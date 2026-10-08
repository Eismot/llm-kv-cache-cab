use crate::live::{Identity, LiveSession, Manifest, Observation};
use crate::llama::{Generation, LlamaClient, Message, RestoredSlot, Role};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use syn::{Expr, FnArg, Pat, ReturnType, Stmt, Type};

#[derive(Clone, Debug, Serialize)]
pub struct RepositoryReport {
    pub stage: String,
    pub mode: &'static str,
    pub context: SourceContext,
    pub task: String,
    pub identity: Option<Identity>,
    pub proposal: Option<Observation<Generation>>,
    pub initial_check: Option<ProjectCheck>,
    pub checkpoint: Option<Observation<Manifest>>,
    pub helper: Option<Observation<Generation>>,
    pub restore: Option<Observation<RestoredSlot>>,
    pub revision: Option<Observation<Generation>>,
    pub final_check: Option<ProjectCheck>,
    pub feedback: Option<String>,
    pub slot_cleared: bool,
    pub boundary_tokens_omitted: usize,
}

pub struct RepositoryLab {
    client: LlamaClient,
    directory: PathBuf,
    root: PathBuf,
    session: Option<LiveSession>,
    messages: Vec<Message>,
    continuation: Vec<u32>,
    pub report: RepositoryReport,
}

impl RepositoryLab {
    pub fn new(client: LlamaClient, directory: &Path) -> anyhow::Result<Self> {
        let root = project_root();
        let context = read_context(&root)?;
        let task=format!("Repository context from {} / {}:\n{}\n\nImplement the status policy for a new GET /api/coding/report JSON download endpoint. The harness will wire the handler and test it in a repository snapshot. Return ONLY one Rust function, no tests or explanation: fn report_status(configured: bool, busy: bool, has_report: bool) -> u16. Return 503 when not configured, otherwise 409 when busy, otherwise 404 when no report, otherwise 200. Use only if/else, boolean arguments, !, return, and numeric status literals. No imports, calls, macros, or loops.",context.path,context.symbol,context.excerpt);
        let messages=vec![Message{role:Role::System,content:"You are a precise Rust coding assistant working on the provided repository excerpt.".to_owned()},Message{role:Role::User,content:task.clone()}];
        Ok(Self {
            client,
            directory: directory.to_owned(),
            root,
            session: None,
            messages,
            continuation: Vec::new(),
            report: RepositoryReport {
                stage: "ready".to_owned(),
                mode: "real_repository_task",
                context,
                task,
                identity: None,
                proposal: None,
                initial_check: None,
                checkpoint: None,
                helper: None,
                restore: None,
                revision: None,
                final_check: None,
                feedback: None,
                slot_cleared: false,
                boundary_tokens_omitted: 0,
            },
        })
    }

    async fn generate(
        &self,
        tokens: &[u32],
        limit: u32,
    ) -> anyhow::Result<Observation<Generation>> {
        let context = self
            .report
            .identity
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No engine identity"))?
            .context_tokens;
        anyhow::ensure!(
            tokens.len() + (limit as usize) < context as usize,
            "Repository task exceeds context budget"
        );
        let started = std::time::Instant::now();
        let engine = self.client.generate(tokens, limit).await?;
        Ok(Observation {
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            engine,
        })
    }

    pub async fn advance(&mut self) -> anyhow::Result<()> {
        match self.report.stage.as_str() {
            "ready" => {
                let mut session = LiveSession::attach(self.client.clone(), &self.directory).await?;
                anyhow::ensure!(
                    session.identity().model_sha256 == crate::coding::MODEL_SHA256,
                    "Repository demo requires approved Qwen model"
                );
                session.erase().await?;
                self.report.identity = Some(session.identity().clone());
                let tokens = self.client.chat_tokens(&self.messages).await?;
                let proposal = self.generate(&tokens, 256).await?;
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: proposal.engine.content.clone(),
                });
                self.report.proposal = Some(proposal);
                self.session = Some(session);
                self.report.stage = "proposed".to_owned();
            }
            "proposed" => {
                let proposal = &self.report.proposal.as_ref().unwrap().engine.content;
                let check = check_project(&self.root, proposal).await;
                let tail: String = check.output.chars().take(2_000).collect();
                let feedback=format!("Actual temporary-project check: accepted={}, passed={}.\n{}\nReturn the final report_status function with the same signature and policy. Fix any failure; if all tests passed retain the correct implementation. Return only the function, no explanation or tests.",check.accepted,check.passed,tail);
                self.report.feedback = Some(feedback.clone());
                self.report.initial_check = Some(check);
                let completed = self.client.chat_tokens(&self.messages).await?;
                self.messages.push(Message {
                    role: Role::User,
                    content: feedback,
                });
                self.continuation = self.client.chat_tokens(&self.messages).await?;
                let length = crate::coding::common_prefix(&completed, &self.continuation);
                anyhow::ensure!(length > 0, "No compatible conversation prefix");
                self.report.boundary_tokens_omitted = completed.len() - length;
                let session = self.session.as_mut().unwrap();
                session.prefill(&completed[..length]).await?;
                self.report.checkpoint = Some(
                    session
                        .save(&crate::SessionId::try_from("repository-task".to_owned())?)
                        .await?,
                );
                session.erase().await?;
                self.report.slot_cleared = true;
                self.report.stage = "parked".to_owned();
            }
            "parked" => {
                let messages=[Message{role:Role::User,content:"In one short sentence, distinguish HTTP 409 Conflict from 503 Service Unavailable.".to_owned()}];
                let tokens = self.client.chat_tokens(&messages).await?;
                self.report.helper = Some(self.generate(&tokens, 96).await?);
                self.session.as_mut().unwrap().erase().await?;
                self.report.stage = "helper_done".to_owned();
            }
            "helper_done" => {
                let session = self.session.as_mut().unwrap();
                let checkpoint = &self.report.checkpoint.as_ref().unwrap().engine;
                self.report.restore = Some(session.restore(checkpoint, &self.continuation).await?);
                let revision = self.generate(&self.continuation, 256).await?;
                anyhow::ensure!(
                    revision.engine.timings.cache_n > 0 && revision.engine.timings.prompt_n > 0,
                    "Restored revision did not report prefix reuse and new input"
                );
                self.report.final_check =
                    Some(check_project(&self.root, &revision.engine.content).await);
                self.report.revision = Some(revision);
                self.session.as_mut().unwrap().erase().await?;
                self.report.stage = "complete".to_owned();
            }
            _ => anyhow::bail!("Reset the repository task before another run"),
        }
        Ok(())
    }

    pub async fn clear(&mut self) -> anyhow::Result<()> {
        self.client.erase().await?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceContext {
    pub path: &'static str,
    pub sha256: String,
    pub symbol: &'static str,
    pub excerpt: String,
}

pub fn read_context(root: &Path) -> anyhow::Result<SourceContext> {
    let filename = root.join("src/main.rs");
    anyhow::ensure!(
        !std::fs::symlink_metadata(&filename)?.is_symlink(),
        "Source symlinks are not permitted"
    );
    let source = std::fs::read_to_string(filename)?;
    anyhow::ensure!(source.len() <= 100_000, "Source file exceeds read budget");
    syn::parse_file(&source)?;
    let start = source
        .find("async fn coding_state(")
        .ok_or_else(|| anyhow::anyhow!("Missing context symbol"))?;
    let end = source[start..]
        .find("\nasync fn coding_step(")
        .ok_or_else(|| anyhow::anyhow!("Missing context boundary"))?
        + start;
    let excerpt = source[start..end].to_owned();
    anyhow::ensure!(excerpt.len() <= 4_096, "Selected context exceeds budget");
    Ok(SourceContext {
        path: "src/main.rs",
        sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
        symbol: "coding_state",
        excerpt,
    })
}

fn scalar(kind: &Type, name: &str) -> bool {
    matches!(kind, Type::Path(path) if path.qself.is_none() && path.path.is_ident(name))
}

fn pure(expression: &Expr, depth: usize) -> bool {
    if depth > 12 {
        return false;
    }
    let next = |expression: &Expr| pure(expression, depth + 1);
    match expression {
        Expr::Path(path) => {
            path.attrs.is_empty()
                && path.qself.is_none()
                && ["configured", "busy", "has_report"]
                    .iter()
                    .any(|name| path.path.is_ident(name))
        }
        Expr::Lit(literal) => {
            literal.attrs.is_empty()
                && matches!(&literal.lit, syn::Lit::Int(value) if matches!(value.base10_parse::<u16>().ok(), Some(200 | 404 | 409 | 503)) && matches!(value.suffix(), "" | "u16"))
        }
        Expr::Unary(unary) => {
            unary.attrs.is_empty() && matches!(unary.op, syn::UnOp::Not(_)) && next(&unary.expr)
        }
        Expr::Paren(parenthesized) => parenthesized.attrs.is_empty() && next(&parenthesized.expr),
        Expr::Binary(binary) => {
            binary.attrs.is_empty()
                && matches!(binary.op, syn::BinOp::And(_) | syn::BinOp::Or(_))
                && next(&binary.left)
                && next(&binary.right)
        }
        Expr::Return(returned) => {
            returned.attrs.is_empty()
                && returned
                    .expr
                    .as_ref()
                    .is_some_and(|expression| next(expression))
        }
        Expr::If(branch) => {
            branch.attrs.is_empty()
                && next(&branch.cond)
                && pure_block(&branch.then_branch, depth + 1)
                && branch
                    .else_branch
                    .as_ref()
                    .map_or(true, |(_, expression)| next(expression))
        }
        Expr::Block(block) => {
            block.attrs.is_empty() && block.label.is_none() && pure_block(&block.block, depth + 1)
        }
        _ => false,
    }
}

fn pure_block(block: &syn::Block, depth: usize) -> bool {
    depth <= 12
        && block.stmts.len() <= 8
        && block.stmts.iter().all(
            |statement| matches!(statement,Stmt::Expr(expression,_) if pure(expression,depth+1)),
        )
}

pub fn reviewed_policy(output: &str) -> anyhow::Result<String> {
    anyhow::ensure!(output.len() <= 4_096, "Proposal exceeds size budget");
    let trimmed = output.trim();
    let source = if let Some(code) = trimmed
        .strip_prefix("```rust\n")
        .or_else(|| trimmed.strip_prefix("```\n"))
    {
        code.strip_suffix("```")
            .ok_or_else(|| anyhow::anyhow!("Unclosed code fence"))?
            .trim()
    } else {
        trimmed
    };
    let function: syn::ItemFn = syn::parse_str(source)?;
    let signature = &function.sig;
    anyhow::ensure!(
        function.attrs.is_empty()
            && signature.ident == "report_status"
            && signature.inputs.len() == 3
            && signature.generics.params.is_empty()
            && signature.generics.where_clause.is_none()
            && signature.constness.is_none()
            && signature.asyncness.is_none()
            && signature.unsafety.is_none()
            && signature.abi.is_none()
            && signature.variadic.is_none()
            && matches!(
                function.vis,
                syn::Visibility::Inherited | syn::Visibility::Public(_)
            ),
        "Unsupported function declaration"
    );
    for (argument, name) in signature
        .inputs
        .iter()
        .zip(["configured", "busy", "has_report"])
    {
        let FnArg::Typed(input) = argument else {
            anyhow::bail!("Unsupported parameter");
        };
        let Pat::Ident(binding) = input.pat.as_ref() else {
            anyhow::bail!("Unsupported parameter");
        };
        anyhow::ensure!(
            input.attrs.is_empty()
                && scalar(&input.ty, "bool")
                && binding.ident == name
                && binding.attrs.is_empty()
                && binding.by_ref.is_none()
                && binding.mutability.is_none()
                && binding.subpat.is_none(),
            "Unsupported parameter"
        );
    }
    anyhow::ensure!(
        matches!(&signature.output,ReturnType::Type(_,kind) if scalar(kind,"u16"))
            && pure_block(&function.block, 0),
        "Proposal failed pure-code review; nothing executed"
    );
    Ok(source.to_owned())
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectCheck {
    pub accepted: bool,
    pub passed: bool,
    pub output: String,
    pub patch: String,
    pub execution: &'static str,
}

const PLUMBING: &str = r#"
fn repository_download_response(configured: bool, busy: bool, report: Option<serde_json::Value>) -> Response {
    let status = StatusCode::from_u16(report_status(configured,busy,report.is_some())).unwrap();
    if status != StatusCode::OK { return (status,Json(serde_json::json!({"error":"Report unavailable"}))).into_response(); }
    ([(header::CONTENT_TYPE,"application/json"),(header::CONTENT_DISPOSITION,"attachment; filename=cache-cab-report.json")],Json(report.unwrap())).into_response()
}

async fn repository_download(State(app): State<App>) -> Response {
    let Some(live) = app.live else { return repository_download_response(false,false,None); };
    let response = match live.try_lock() {
        Ok(lab) => repository_download_response(true,false,lab.coding.as_ref().and_then(|coding|serde_json::to_value(&coding.report).ok())),
        Err(_) => repository_download_response(true,true,None),
    };
    response
}

#[cfg(test)]
mod repository_feature_tests {
    use super::*;
    use axum::body::to_bytes;
    #[test]
    fn repo_export_status_matrix() {
        for configured in [false,true] { for busy in [false,true] { for has_report in [false,true] {
            let expected = if !configured {503} else if busy {409} else if !has_report {404} else {200};
            assert_eq!(report_status(configured,busy,has_report),expected,"configured={configured}, busy={busy}, has_report={has_report}");
        } } }
    }
    #[tokio::test]
    async fn repo_export_attachment_preserves_json() {
        let report = serde_json::json!({"stage":"complete","tokens":260});
        let response = repository_download_response(true,false,Some(report.clone()));
        assert_eq!(response.status(),StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE],"application/json");
        assert_eq!(response.headers()[header::CONTENT_DISPOSITION],"attachment; filename=cache-cab-report.json");
        let body=to_bytes(response.into_body(),4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(),report);
    }
    #[tokio::test]
    async fn repo_export_http_route_is_wired() {
        use tower::ServiceExt;
        let directory=tempfile::tempdir().unwrap();
        let app=App { demo:Arc::new(Mutex::new(Demo::new(directory.path(),8).unwrap())),live:None,data_path:directory.path().to_owned(),port:7331 };
        let request=axum::http::Request::builder().uri("/api/coding/report").header(header::HOST,"127.0.0.1:7331").body(axum::body::Body::empty()).unwrap();
        assert_eq!(router(app).oneshot(request).await.unwrap().status(),StatusCode::SERVICE_UNAVAILABLE);
    }
}
"#;

fn copy_tree(source: &Path, target: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        anyhow::ensure!(!kind.is_symlink(), "Snapshot symlinks are not permitted");
        if kind.is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

pub fn redact_paths(output: &str, roots: &[(&Path, &str)]) -> String {
    let mut redacted = output.to_owned();
    for (path, replacement) in roots {
        let spelling = path.to_string_lossy();
        if spelling.is_empty() {
            continue;
        }
        for variant in [
            spelling.to_string(),
            spelling.replace('\\', "/"),
            spelling.replace('/', "\\"),
        ] {
            redacted = redacted.replace(&variant, replacement);
        }
    }
    redacted
}

pub async fn check_project(root: &Path, output: &str) -> ProjectCheck {
    let policy = match reviewed_policy(output) {
        Ok(policy) => policy,
        Err(error) => {
            return ProjectCheck {
                accepted: false,
                passed: false,
                output: error.to_string(),
                patch: String::new(),
                execution: "Rejected; no generated code executed",
            }
        }
    };
    let patch = format!(
        "MODEL / src/main.rs addition\n{policy}\n\nHARNESS / trusted HTTP wiring and 3 fixed API tests\n{PLUMBING}"
    );
    let result = async {
        let temporary = tempfile::tempdir()?;
        for name in ["Cargo.toml", "Cargo.lock"] {
            std::fs::copy(root.join(name), temporary.path().join(name))?;
        }
        for name in ["src", "web"] {
            copy_tree(&root.join(name), &temporary.path().join(name))?;
        }
        let filename = temporary.path().join("src/main.rs");
        let source = std::fs::read_to_string(&filename)?;
        let anchor = ".route(\"/api/coding/state\", get(coding_state))";
        anyhow::ensure!(
            source.matches(anchor).count() == 1,
            "Route patch anchor changed; refusing mutation"
        );
        let source = source.replacen(
            anchor,
            &format!("{anchor}\n.route(\"/api/coding/report\", get(repository_download))"),
            1,
        );
        std::fs::write(filename, format!("{source}\n{policy}\n{PLUMBING}"))?;
        let target = root.join(".cache-cab/repository-target");
        let command = tokio::process::Command::new("cargo")
            .args([
                "test",
                "--offline",
                "--locked",
                "--bin",
                "cache-cab",
                "repo_export_",
            ])
            .env("CARGO_TARGET_DIR", &target)
            .env("CARGO_BUILD_JOBS", "2")
            .current_dir(temporary.path())
            .kill_on_drop(true)
            .output();
        let executed = tokio::time::timeout(std::time::Duration::from_secs(180), command)
            .await
            .map_err(|_| anyhow::anyhow!("Temporary project checks timed out"))??;
        let output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&executed.stdout),
            String::from_utf8_lossy(&executed.stderr)
        );
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let output = redact_paths(
            &output,
            &[
                (temporary.path(), "<temporary-project>"),
                (&target, "<build-cache>"),
                (root, "<project>"),
                (&home, "<home>"),
            ],
        );
        anyhow::ensure!(output.len() < 100_000, "Compiler output exceeds budget");
        Ok::<_, anyhow::Error>((executed.status.success(), output))
    }
    .await;
    let (passed, output) = result.unwrap_or_else(|error| (false, error.to_string()));
    ProjectCheck {
        accepted: true,
        passed,
        output,
        patch,
        execution: "Pure generated policy reviewed; actual repository snapshot tested offline in a temporary folder. Not an OS sandbox.",
    }
}

pub fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_compiler_output_redacts_native_and_mixed_paths() {
        let root = Path::new("C:\\Users\\example\\project");
        let output="Compiling (C:\\Users\\example\\project) C:/Users/example/project/target/debug/test.exe";
        assert_eq!(
            redact_paths(output, &[(root, "<project>")]),
            "Compiling (<project>) <project>/target/debug/test.exe"
        );
        assert_eq!(
            redact_paths("unchanged", &[(Path::new(""), "<home>")]),
            "unchanged"
        );
    }
    #[test]
    fn only_boolean_status_policy_can_be_executed() {
        let source="fn report_status(configured:bool,busy:bool,has_report:bool)->u16{if !configured{return 503;}if busy{return 409;}if !has_report{return 404;}200}";
        reviewed_policy(source).unwrap();
        for bad in [
            source.replace("200}", "std::process::exit(0)}"),
            source.replace("200}", "loop{}}"),
            source.replace("200}", "panic!()}"),
            format!("{source} fn main(){{}}"),
            format!("#[test] {source}"),
        ] {
            assert!(reviewed_policy(&bad).is_err());
        }
    }
    #[test]
    fn context_comes_from_the_actual_repository() {
        let context = read_context(&project_root()).unwrap();
        assert_eq!(context.path, "src/main.rs");
        assert!(context.excerpt.contains("lab.coding"));
        assert_eq!(context.sha256.len(), 64);
    }
}
