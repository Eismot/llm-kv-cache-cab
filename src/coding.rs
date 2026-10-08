use crate::live::{Identity, LiveSession, Manifest, Observation};
use crate::llama::{Generation, LlamaClient, Message, Prefill, RestoredSlot, Role};
use serde::Serialize;
use std::path::{Path, PathBuf};
use syn::{Expr, FnArg, GenericArgument, Lit, Pat, PathArguments, ReturnType, Stmt, Type};
use thiserror::Error;

pub const MODEL_SHA256: &str = "cc324af070c2ecbfd324a30884d2f951a7ff756aba85cb811a6ec436933bb046";
pub const ORIGINAL: &str = "fn invoice_total(unit_price: u64, quantity: u64, discount_percent: u64) -> u64 {\n    let gross = unit_price * quantity;\n    let discount = gross * discount_percent / 100;\n    gross - discount\n}";
pub const QUESTION: &str = "Fix this Rust invoice function. Keep invoice_total and the three u64 parameters in the same order. Return Option<u64>. Reject discount_percent > 100. Use checked_mul for gross and gross * discount_percent, checked_div(100) to round the discount down, and checked_sub to deduct it. Propagate any intermediate overflow as None using ?. Return only the function in one rust code fence; no comments, tests, imports, or explanation.\n\nfn invoice_total(unit_price: u64, quantity: u64, discount_percent: u64) -> u64 {\n    let gross = unit_price * quantity;\n    let discount = gross * discount_percent / 100;\n    gross - discount\n}";
pub const FOLLOWUP: &str = "For your invoice_total, compare (101, 3, 10), (u64::MAX, 1, 0), and (u64::MAX, 1, 50). Give each result and explain rounding versus intermediate overflow, in at most four short sentences. Do not write code.";
pub const FOLLOWUP_REFERENCE: &str = "Test oracle: (101, 3, 10) -> Some(273); (u64::MAX, 1, 0) -> Some(u64::MAX); (u64::MAX, 1, 50) -> None. Code tests do not validate the model's explanation.";
const OUTPUT_LIMIT: u32 = 256;

#[derive(Clone, Debug, Serialize)]
pub struct CodingReport {
    pub stage: String,
    pub mode: &'static str,
    pub model: &'static str,
    pub original: &'static str,
    pub question: &'static str,
    pub followup_question: &'static str,
    pub followup_reference: &'static str,
    pub output_limit: u32,
    pub identity: Option<Identity>,
    pub first: Option<Observation<Generation>>,
    pub validation: Option<SampleValidation>,
    pub prefix_prefill: Option<Observation<Prefill>>,
    pub canonical_prefix_tokens: usize,
    pub boundary_tokens_omitted: usize,
    pub save: Option<Observation<Manifest>>,
    pub slot_cleared: bool,
    pub restore: Option<Observation<RestoredSlot>>,
    pub followup: Option<Observation<Generation>>,
}

pub struct CodingLab {
    client: LlamaClient,
    directory: PathBuf,
    session: Option<LiveSession>,
    prefix: Vec<u32>,
    continuation: Vec<u32>,
    pub report: CodingReport,
}

pub fn common_prefix(first: &[u32], second: &[u32]) -> usize {
    first
        .iter()
        .zip(second)
        .take_while(|(left, right)| left == right)
        .count()
}

impl CodingLab {
    pub fn new(client: LlamaClient, directory: &Path) -> Self {
        Self {
            client,
            directory: directory.to_owned(),
            session: None,
            prefix: Vec::new(),
            continuation: Vec::new(),
            report: CodingReport {
                stage: "ready".to_owned(),
                mode: "real_coding_conversation",
                model: "Qwen2.5-Coder-1.5B-Instruct Q4_K_M",
                original: ORIGINAL,
                question: QUESTION,
                followup_question: FOLLOWUP,
                followup_reference: FOLLOWUP_REFERENCE,
                output_limit: OUTPUT_LIMIT,
                identity: None,
                first: None,
                validation: None,
                prefix_prefill: None,
                canonical_prefix_tokens: 0,
                boundary_tokens_omitted: 0,
                save: None,
                slot_cleared: false,
                restore: None,
                followup: None,
            },
        }
    }

    async fn generate(
        &self,
        tokens: &[u32],
        context: u32,
    ) -> anyhow::Result<Observation<Generation>> {
        anyhow::ensure!(
            tokens.len() + (OUTPUT_LIMIT as usize) < context as usize,
            "Conversation exceeds bounded slot context"
        );
        let start = std::time::Instant::now();
        let engine = self.client.generate(tokens, OUTPUT_LIMIT).await?;
        Ok(Observation {
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            engine,
        })
    }

    pub async fn advance(&mut self) -> anyhow::Result<()> {
        match self.report.stage.as_str() {
            "ready" => {
                let mut session = LiveSession::attach(self.client.clone(), &self.directory).await?;
                anyhow::ensure!(
                    session.identity().model_sha256 == MODEL_SHA256,
                    "Coding demo requires the approved checksum-verified Qwen model"
                );
                session.erase().await?;
                let mut messages = vec![
                    Message {
                        role: Role::System,
                        content: "You are a precise Rust coding assistant.".to_owned(),
                    },
                    Message {
                        role: Role::User,
                        content: QUESTION.to_owned(),
                    },
                ];
                let tokens = self.client.chat_tokens(&messages).await?;
                let first = self
                    .generate(&tokens, session.identity().context_tokens)
                    .await?;
                messages.push(Message {
                    role: Role::Assistant,
                    content: first.engine.content.clone(),
                });
                let completed = self.client.chat_tokens(&messages).await?;
                messages.push(Message {
                    role: Role::User,
                    content: FOLLOWUP.to_owned(),
                });
                self.continuation = self.client.chat_tokens(&messages).await?;
                let length = common_prefix(&completed, &self.continuation);
                anyhow::ensure!(
                    length >= tokens.len(),
                    "Rendered conversation does not preserve the initial prompt token prefix"
                );
                self.prefix = completed[..length].to_vec();
                self.report.canonical_prefix_tokens = length;
                self.report.boundary_tokens_omitted = completed.len() - length;
                self.report.validation = Some(validate_and_test(&first.engine.content).await);
                self.report.identity = Some(session.identity().clone());
                self.report.first = Some(first);
                self.report.stage = "answered".to_owned();
                self.session = Some(session);
            }
            "answered" => {
                let session = self
                    .session
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("Missing live session"))?;
                self.report.prefix_prefill = Some(session.prefill(&self.prefix).await?);
                let saved = session
                    .save(&crate::SessionId::try_from("coding-demo".to_owned())?)
                    .await?;
                session.erase().await?;
                self.report.save = Some(saved);
                self.report.slot_cleared = true;
                self.report.stage = "checkpointed".to_owned();
            }
            "checkpointed" => {
                let session = self
                    .session
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("Missing live session"))?;
                let manifest = &self
                    .report
                    .save
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Missing checkpoint"))?
                    .engine;
                let restored = session.restore(manifest, &self.continuation).await?;
                let context = session.identity().context_tokens;
                self.report.restore = Some(restored);
                let followup = self.generate(&self.continuation, context).await?;
                anyhow::ensure!(
                    followup.engine.timings.cache_n > 0 && followup.engine.timings.prompt_n > 0,
                    "Follow-up did not report both prefix reuse and new input processing"
                );
                self.report.followup = Some(followup);
                self.session.as_mut().unwrap().erase().await?;
                self.report.stage = "complete".to_owned();
            }
            _ => anyhow::bail!("Coding demo is complete; reset before another run"),
        }
        Ok(())
    }

    pub async fn clear(&mut self) -> anyhow::Result<()> {
        self.client.erase().await?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum CodingError {
    #[error("model output does not match the narrowly allowed pure Rust function")]
    UnsafeSample,
    #[error("sample I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("sample compilation or test execution timed out")]
    TestTimeout,
    #[error("sample tool failed: {0}")]
    Tool(String),
}

fn named_type(kind: &Type, expected: &str) -> bool {
    matches!(kind, Type::Path(path) if path.qself.is_none() && path.path.leading_colon.is_none() && path.path.segments.len() == 1 && path.path.segments[0].ident == expected && matches!(path.path.segments[0].arguments, PathArguments::None))
}

fn option_u64(kind: &Type) -> bool {
    let Type::Path(path) = kind else {
        return false;
    };
    if path.qself.is_some()
        || path.path.leading_colon.is_some()
        || path.path.segments.len() != 1
        || path.path.segments[0].ident != "Option"
    {
        return false;
    }
    let PathArguments::AngleBracketed(arguments) = &path.path.segments[0].arguments else {
        return false;
    };
    matches!(arguments.args.first(), Some(GenericArgument::Type(kind)) if arguments.args.len() == 1 && named_type(kind, "u64"))
}

fn allowed_expression(expression: &Expr, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    let allowed = |expression: &Expr| allowed_expression(expression, depth + 1);
    match expression {
        Expr::Path(path) => {
            path.attrs.is_empty() && path.qself.is_none() && path.path.get_ident().is_some()
        }
        Expr::Lit(literal) => {
            literal.attrs.is_empty()
                && matches!(&literal.lit, Lit::Int(value) if value.base10_parse::<u64>().is_ok() && matches!(value.suffix(), "" | "u64"))
        }
        Expr::MethodCall(call) => {
            call.attrs.is_empty()
                && matches!(
                    call.method.to_string().as_str(),
                    "checked_mul" | "checked_div" | "checked_sub"
                )
                && call.turbofish.is_none()
                && call.args.len() == 1
                && allowed(&call.receiver)
                && call.args.iter().all(allowed)
        }
        Expr::Try(tried) => tried.attrs.is_empty() && allowed(&tried.expr),
        Expr::Paren(parenthesized) => {
            parenthesized.attrs.is_empty() && allowed(&parenthesized.expr)
        }
        Expr::Binary(binary) => {
            binary.attrs.is_empty()
                && matches!(
                    binary.op,
                    syn::BinOp::Gt(_)
                        | syn::BinOp::Ge(_)
                        | syn::BinOp::Lt(_)
                        | syn::BinOp::Le(_)
                        | syn::BinOp::Eq(_)
                        | syn::BinOp::Ne(_)
                )
                && allowed(&binary.left)
                && allowed(&binary.right)
        }
        Expr::Return(returned) => {
            returned.attrs.is_empty()
                && returned
                    .expr
                    .as_ref()
                    .is_some_and(|expression| allowed(expression))
        }
        Expr::Call(call) => {
            call.attrs.is_empty()
                && matches!(call.func.as_ref(), Expr::Path(path) if path.attrs.is_empty() && path.qself.is_none() && path.path.is_ident("Some"))
                && call.args.len() == 1
                && call.args.iter().all(allowed)
        }
        Expr::If(branch) => {
            branch.attrs.is_empty()
                && allowed(&branch.cond)
                && allowed_block(&branch.then_branch, depth + 1)
                && branch
                    .else_branch
                    .as_ref()
                    .map_or(true, |(_, expression)| allowed(expression))
        }
        Expr::Block(block) => {
            block.attrs.is_empty()
                && block.label.is_none()
                && allowed_block(&block.block, depth + 1)
        }
        _ => false,
    }
}

fn allowed_block(block: &syn::Block, depth: usize) -> bool {
    depth <= 32 && block.stmts.len() <= 12 && block.stmts.iter().all(|statement| match statement {
        Stmt::Expr(expression, _) => allowed_expression(expression, depth + 1),
        Stmt::Local(local) => local.attrs.is_empty() && matches!(&local.pat, Pat::Ident(binding) if binding.attrs.is_empty() && binding.by_ref.is_none() && binding.mutability.is_none() && binding.subpat.is_none()) && local.init.as_ref().is_some_and(|init| init.diverge.is_none() && allowed_expression(&init.expr, depth + 1)),
        _ => false,
    })
}

pub fn validated_sample(output: &str) -> Result<String, CodingError> {
    let output = output.trim();
    let source = if let Some(code) = output
        .strip_prefix("```rust\n")
        .or_else(|| output.strip_prefix("```\n"))
    {
        code.strip_suffix("```")
            .ok_or(CodingError::UnsafeSample)?
            .trim()
    } else {
        output
    };
    if source.len() > 4096 {
        return Err(CodingError::UnsafeSample);
    }
    let function: syn::ItemFn = syn::parse_str(source).map_err(|_| CodingError::UnsafeSample)?;
    let signature = &function.sig;
    if !function.attrs.is_empty()
        || signature.ident != "invoice_total"
        || signature.constness.is_some()
        || signature.asyncness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.variadic.is_some()
        || !signature.generics.params.is_empty()
        || signature.generics.where_clause.is_some()
        || signature.inputs.len() != 3
        || !matches!(
            function.vis,
            syn::Visibility::Inherited | syn::Visibility::Public(_)
        )
    {
        return Err(CodingError::UnsafeSample);
    }
    for (argument, expected) in
        signature
            .inputs
            .iter()
            .zip(["unit_price", "quantity", "discount_percent"])
    {
        let FnArg::Typed(input) = argument else {
            return Err(CodingError::UnsafeSample);
        };
        let Pat::Ident(parameter) = input.pat.as_ref() else {
            return Err(CodingError::UnsafeSample);
        };
        if !input.attrs.is_empty()
            || parameter.ident != expected
            || parameter.by_ref.is_some()
            || parameter.mutability.is_some()
            || parameter.subpat.is_some()
            || !parameter.attrs.is_empty()
            || !named_type(&input.ty, "u64")
        {
            return Err(CodingError::UnsafeSample);
        }
    }
    let ReturnType::Type(_, output_type) = &signature.output else {
        return Err(CodingError::UnsafeSample);
    };
    if !option_u64(output_type) {
        return Err(CodingError::UnsafeSample);
    }
    if !allowed_block(&function.block, 0) {
        return Err(CodingError::UnsafeSample);
    }
    Ok(source.to_owned())
}

#[derive(Clone, Debug, Serialize)]
pub struct SampleValidation {
    pub accepted: bool,
    pub passed: bool,
    pub cases: usize,
    pub details: String,
}

pub async fn validate_and_test(output: &str) -> SampleValidation {
    let source = match validated_sample(output) {
        Ok(source) => source,
        Err(error) => {
            return SampleValidation {
                accepted: false,
                passed: false,
                cases: 0,
                details: error.to_string(),
            }
        }
    };
    let result = async {
        let directory = tempfile::tempdir()?;
        let filename = directory.path().join("sample.rs");
        let executable = directory.path().join(if cfg!(windows) { "sample-tests.exe" } else { "sample-tests" });
        let tests = "\n#[test]\nfn invoice_cases() { for (price, quantity, percent, expected) in [(100,3,10,Some(270)),(101,3,10,Some(273)),(0,5,50,Some(0)),(100,0,50,Some(0)),(100,3,0,Some(300)),(100,3,100,Some(0)),(100,3,101,None),(0,0,101,None),(u64::MAX,2,0,None),(u64::MAX,1,0,Some(u64::MAX)),(u64::MAX,1,50,None),(1,1,99,Some(1))] { assert_eq!(invoice_total(price,quantity,percent),expected,\"input: {price}, {quantity}, {percent}\"); } }\n";
        std::fs::write(&filename, format!("#![forbid(unsafe_code)]\n{source}{tests}"))?;
        let compiler = tokio::process::Command::new("rustc").arg("--test").arg("--edition=2021").arg(&filename).arg("-o").arg(&executable)
            .current_dir(directory.path()).kill_on_drop(true).output();
        let compiled = tokio::time::timeout(std::time::Duration::from_secs(30), compiler).await.map_err(|_| CodingError::TestTimeout)??;
        if !compiled.status.success() { return Err(CodingError::Tool(crate::repo_task::redact_paths(&String::from_utf8_lossy(&compiled.stderr), &[(directory.path(), "<temporary-project>")]))); }
        let executed = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::process::Command::new(&executable).current_dir(directory.path()).kill_on_drop(true).output()).await.map_err(|_| CodingError::TestTimeout)??;
        if !executed.status.success() { return Err(CodingError::Tool(crate::repo_task::redact_paths(&String::from_utf8_lossy(&executed.stdout), &[(directory.path(), "<temporary-project>")]))); }
        Ok::<_,CodingError>(crate::repo_task::redact_paths(&String::from_utf8_lossy(&executed.stdout), &[(directory.path(), "<temporary-project>")]))
    }.await;
    match result {
        Ok(details) => SampleValidation {
            accepted: true,
            passed: true,
            cases: 12,
            details,
        },
        Err(error) => SampleValidation {
            accepted: true,
            passed: false,
            cases: 12,
            details: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bounded_pure_invoice_code_is_accepted() {
        validated_sample(SAMPLE).unwrap();
        for output in [
            SAMPLE.replace(
                "gross.checked_sub(discount)",
                "std::fs::remove_file(\"important\").ok(); None",
            ),
            format!("#[test] {SAMPLE}"),
            format!("{SAMPLE} fn main(){{}}"),
            SAMPLE.replace("fn invoice_total", "unsafe fn invoice_total"),
            SAMPLE.replace("gross.checked_sub(discount)", "loop {}"),
            SAMPLE.replace("gross.checked_sub(discount)", "panic!(\"no\")"),
            SAMPLE.replace("gross.checked_sub(discount)", "Some(unit_price * quantity)"),
        ] {
            assert!(validated_sample(&output).is_err(), "{output}");
        }
    }

    const SAMPLE: &str = "fn invoice_total(unit_price:u64,quantity:u64,discount_percent:u64)->Option<u64>{if discount_percent>100{return None;}let gross=unit_price.checked_mul(quantity)?;let discount=gross.checked_mul(discount_percent)?.checked_div(100)?;gross.checked_sub(discount)}";

    #[test]
    fn checkpoint_prefix_excludes_changed_token_boundary() {
        assert_eq!(common_prefix(&[1, 2, 3, 4], &[1, 2, 3, 5, 6]), 3);
        assert_eq!(common_prefix(&[1, 2], &[1, 2, 3]), 2);
        assert_eq!(common_prefix(&[9], &[1]), 0);
    }

    #[tokio::test]
    async fn accepted_function_passes_isolated_boundary_tests() {
        let validation = validate_and_test(SAMPLE).await;
        assert!(validation.accepted);
        assert!(validation.passed, "{}", validation.details);
        assert!(!validate_and_test("fn main(){panic!()}").await.accepted);
        let incorrect =
            validate_and_test(&SAMPLE.replace("gross.checked_sub(discount)", "Some(gross)")).await;
        assert!(incorrect.accepted);
        assert!(!incorrect.passed);
    }
}
