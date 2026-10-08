use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::llama::{EngineError, LlamaClient, Prefill, RestoredSlot};
use crate::SessionId;

#[derive(Debug, Error)]
pub enum LiveError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("live checkpoint I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("live checkpoint JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("live checkpoint persistence: {0}")]
    Persist(#[from] tempfile::PersistError),
    #[error("server must report an absolute local model path and a nonempty engine build")]
    InvalidIdentity,
    #[error("checkpoint belongs to another model, engine build, or context")]
    IncompatibleIdentity,
    #[error("checkpoint prompt is not an exact token prefix of the requested prompt")]
    PrefixMismatch,
    #[error("checkpoint is missing, oversized, corrupted, or has inconsistent accounting")]
    InvalidCheckpoint,
    #[error("no prefilled prompt is available to checkpoint")]
    NoPrompt,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub model_sha256: String,
    pub engine_build: String,
    pub context_tokens: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    pub session: SessionId,
    pub identity: Identity,
    pub tokens: Vec<u32>,
    pub filename: String,
    pub bytes: u64,
    pub sha256: String,
    pub saved_tokens: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Observation<T> {
    pub elapsed_ms: f64,
    pub engine: T,
}

pub struct LiveSession {
    client: LlamaClient,
    directory: PathBuf,
    identity: Identity,
    active_tokens: Vec<u32>,
}

impl LiveSession {
    pub async fn attach(client: LlamaClient, directory: &Path) -> Result<Self, LiveError> {
        let info = client.info().await?;
        let slot = client.exclusive_slot().await?;
        let model = Path::new(&info.model_path);
        if !model.is_absolute() || info.build_info.is_empty() {
            return Err(LiveError::InvalidIdentity);
        }
        let model_sha256 = digest_file(model)?;
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let identity = Identity {
            model_sha256,
            engine_build: info.build_info,
            context_tokens: slot.n_ctx,
        };
        Ok(Self {
            client,
            directory,
            identity,
            active_tokens: Vec::new(),
        })
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub async fn tokenize(&self, prompt: &str) -> Result<Vec<u32>, LiveError> {
        Ok(self.client.tokenize(prompt).await?)
    }

    pub async fn tokenize_fragment(&self, prompt: &str) -> Result<Vec<u32>, LiveError> {
        Ok(self.client.tokenize_fragment(prompt).await?)
    }

    pub async fn prefill(&mut self, tokens: &[u32]) -> Result<Observation<Prefill>, LiveError> {
        if tokens.len() > self.identity.context_tokens as usize {
            return Err(LiveError::InvalidCheckpoint);
        }
        let start = Instant::now();
        let engine = self.client.prefill(tokens).await?;
        let elapsed_ms = milliseconds(start.elapsed());
        self.active_tokens = tokens.to_vec();
        Ok(Observation { elapsed_ms, engine })
    }

    pub async fn erase(&mut self) -> Result<(), LiveError> {
        self.client.erase().await?;
        self.active_tokens.clear();
        Ok(())
    }

    pub async fn save(&self, session: &SessionId) -> Result<Observation<Manifest>, LiveError> {
        if self.active_tokens.is_empty() {
            return Err(LiveError::NoPrompt);
        }
        let key = checkpoint_key(session, &self.identity, &self.active_tokens)?;
        let filename = format!("cc-{key}.bin");
        let binary = self.directory.join(&filename);
        if let Ok(metadata) = fs::symlink_metadata(&binary) {
            if !metadata.is_file() || metadata.is_symlink() {
                return Err(LiveError::InvalidCheckpoint);
            }
        }
        let start = Instant::now();
        let saved = self.client.save(&filename).await?;
        let metadata = fs::symlink_metadata(&binary)?;
        if metadata.is_symlink()
            || !metadata.is_file()
            || metadata.len() != saved.n_written
            || metadata.len() > 2_147_483_648
            || saved.n_saved as usize > self.active_tokens.len()
        {
            return Err(LiveError::InvalidCheckpoint);
        }
        let manifest = Manifest {
            version: 1,
            session: session.clone(),
            identity: self.identity.clone(),
            tokens: self.active_tokens.clone(),
            filename,
            bytes: metadata.len(),
            sha256: digest_file(&binary)?,
            saved_tokens: saved.n_saved,
        };
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        temporary.write_all(&serde_json::to_vec(&manifest)?)?;
        temporary.as_file().sync_all()?;
        temporary.persist(self.directory.join(format!("cc-{key}.json")))?;
        Ok(Observation {
            elapsed_ms: milliseconds(start.elapsed()),
            engine: manifest,
        })
    }

    pub async fn restore(
        &mut self,
        manifest: &Manifest,
        requested: &[u32],
    ) -> Result<Observation<RestoredSlot>, LiveError> {
        let start = Instant::now();
        validate_manifest(manifest, &self.identity, requested)?;
        let binary = self.directory.join(&manifest.filename);
        let metadata = fs::symlink_metadata(&binary)?;
        if !metadata.is_file()
            || metadata.is_symlink()
            || metadata.len() != manifest.bytes
            || digest_file(&binary)? != manifest.sha256
        {
            return Err(LiveError::InvalidCheckpoint);
        }
        let engine = self.client.restore(&manifest.filename).await?;
        if engine.n_restored != manifest.saved_tokens || engine.n_read != manifest.bytes {
            return Err(LiveError::InvalidCheckpoint);
        }
        self.active_tokens = manifest.tokens.clone();
        Ok(Observation {
            elapsed_ms: milliseconds(start.elapsed()),
            engine,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeReport {
    pub mode: &'static str,
    pub scope: &'static str,
    pub identity: Identity,
    pub prompt_tokens: usize,
    pub cold_prefill: Observation<Prefill>,
    pub save: Observation<Manifest>,
    pub restore: Observation<RestoredSlot>,
    pub reused_prefill: Observation<Prefill>,
    pub continuation_tokens: usize,
    pub continued_prefill: Observation<Prefill>,
    pub verified: bool,
}

pub async fn checkpoint_probe(
    client: LlamaClient,
    directory: &Path,
) -> Result<ProbeReport, LiveError> {
    let mut session = LiveSession::attach(client, directory).await?;
    let tokens = session.tokenize("Once upon a time, there was a small yellow car. The car carried its passengers safely through the city. One day, the driver learned that keeping a familiar route ready made the next journey easier.").await?;
    let id = SessionId::try_from("checkpoint-smoke-test".to_owned())
        .map_err(|_| LiveError::InvalidIdentity)?;
    let result = async {
        session.erase().await?;
        let cold_prefill = session.prefill(&tokens).await?;
        let save = session.save(&id).await?;
        session.erase().await?;
        let restore = session.restore(&save.engine, &tokens).await?;
        let reused_prefill = session.prefill(&tokens).await?;
        if reused_prefill.engine.timings.cache_n == 0 || reused_prefill.engine.timings.prompt_n >= cold_prefill.engine.timings.prompt_n {
            return Err(LiveError::Engine(EngineError::Protocol("restoration did not reduce prompt-token evaluation")));
        }
        let mut continuation = tokens.clone();
        continuation.extend(session.tokenize_fragment(" The driver continued the journey with a new passenger.").await?);
        validate_manifest(&save.engine, session.identity(), &continuation)?;
        let continued_prefill = session.prefill(&continuation).await?;
        if continued_prefill.engine.timings.cache_n == 0 || continued_prefill.engine.timings.prompt_n == 0 {
            return Err(LiveError::Engine(EngineError::Protocol("continuation did not reuse a prefix and evaluate new tokens")));
        }
        Ok(ProbeReport {
            mode: "real_llama_cpp_checkpoint_smoke_test",
            scope: "Model already resident; fresh versus restored KV state; logical checkpoint bytes, not physical SSD reads; not a coding-quality or policy-speedup benchmark",
            identity: session.identity().clone(), prompt_tokens: tokens.len(), cold_prefill, save, restore, reused_prefill,
            continuation_tokens: continuation.len(), continued_prefill, verified: true,
        })
    }.await;
    let cleanup = session.erase().await;
    match result {
        Ok(report) => {
            cleanup?;
            Ok(report)
        }
        Err(error) => Err(error),
    }
}

pub fn validate_manifest(
    manifest: &Manifest,
    identity: &Identity,
    requested: &[u32],
) -> Result<(), LiveError> {
    if manifest.identity != *identity {
        return Err(LiveError::IncompatibleIdentity);
    }
    if manifest.version != 1
        || manifest.tokens.is_empty()
        || manifest.tokens.len() > identity.context_tokens as usize
        || manifest.tokens.len() > 65_536
        || manifest.saved_tokens == 0
        || manifest.saved_tokens as usize > manifest.tokens.len()
        || manifest.bytes == 0
        || manifest.bytes > 2_147_483_648
        || manifest.sha256.len() != 64
        || !manifest
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(LiveError::InvalidCheckpoint);
    }
    let expected = format!(
        "cc-{}.bin",
        checkpoint_key(&manifest.session, identity, &manifest.tokens)?
    );
    if manifest.filename != expected {
        return Err(LiveError::InvalidCheckpoint);
    }
    if requested.len() > identity.context_tokens as usize
        || !requested.starts_with(&manifest.tokens)
    {
        return Err(LiveError::PrefixMismatch);
    }
    Ok(())
}

fn checkpoint_key(
    session: &SessionId,
    identity: &Identity,
    tokens: &[u32],
) -> Result<String, LiveError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(session, identity, tokens))?)
    ))
}

fn digest_file(path: &Path) -> Result<String, LiveError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Query, State},
        routing::{get, post},
        Json, Router,
    };
    use std::collections::BTreeMap;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Clone)]
    struct FixtureEngine {
        model: PathBuf,
        slots: PathBuf,
        restores: Arc<AtomicUsize>,
    }

    #[tokio::test]
    async fn real_binary_integrity_is_checked_before_the_restore_request() {
        let directory = tempfile::tempdir().unwrap();
        let model = directory.path().join("model.gguf");
        fs::write(&model, b"protocol fixture model").unwrap();
        let slots = directory.path().join("slots");
        fs::create_dir(&slots).unwrap();
        let restores = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/props", get(|State(state): State<FixtureEngine>| async move {
                Json(serde_json::json!({"model_path":state.model,"build_info":"fixture-build"}))
            }))
            .route("/slots", get(|| async { Json(serde_json::json!([{"id":0,"n_ctx":16,"is_processing":false}])) }))
            .route("/completion", post(|| async { Json(serde_json::json!({"tokens_cached":0,"tokens_evaluated":3,"timings":{"cache_n":0,"prompt_n":3,"prompt_ms":1.0}})) }))
            .route("/slots/0", post(|State(state): State<FixtureEngine>, Query(query): Query<BTreeMap<String,String>>, Json(body): Json<serde_json::Value>| async move {
                Json(match query.get("action").map(String::as_str) {
                    Some("save") => {
                        let filename = body["filename"].as_str().unwrap();
                        fs::write(state.slots.join(filename), b"fixture-kv").unwrap();
                        serde_json::json!({"id_slot":0,"n_saved":3,"n_written":10})
                    },
                    Some("restore") => {
                        state.restores.fetch_add(1, Ordering::SeqCst);
                        serde_json::json!({"id_slot":0,"n_restored":3,"n_read":10})
                    },
                    _ => serde_json::json!({"id_slot":0}),
                })
            })).with_state(FixtureEngine { model, slots:slots.clone(), restores:restores.clone() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client =
            LlamaClient::new(&format!("http://{address}"), Duration::from_secs(2)).unwrap();
        let mut session = LiveSession::attach(client, &slots).await.unwrap();
        session.prefill(&[1, 2, 3]).await.unwrap();
        let id = SessionId::try_from("fixture".to_owned()).unwrap();
        let saved = session.save(&id).await.unwrap();
        session.erase().await.unwrap();
        session.restore(&saved.engine, &[1, 2, 3, 4]).await.unwrap();
        assert_eq!(restores.load(Ordering::SeqCst), 1);
        assert!(matches!(
            session.restore(&saved.engine, &[9, 2, 3]).await,
            Err(LiveError::PrefixMismatch)
        ));
        fs::write(slots.join(&saved.engine.filename), b"tamperedkv").unwrap();
        assert!(matches!(
            session.restore(&saved.engine, &[1, 2, 3]).await,
            Err(LiveError::InvalidCheckpoint)
        ));
        assert_eq!(restores.load(Ordering::SeqCst), 1);
        server.abort();
    }

    fn fixture() -> Manifest {
        let identity = Identity {
            model_sha256: "a".repeat(64),
            engine_build: "build-1".to_owned(),
            context_tokens: 2048,
        };
        let session = SessionId::try_from("refactor".to_owned()).unwrap();
        let tokens = vec![1, 2, 3];
        let filename = format!(
            "cc-{}.bin",
            checkpoint_key(&session, &identity, &tokens).unwrap()
        );
        Manifest {
            version: 1,
            session,
            identity,
            tokens,
            filename,
            bytes: 128,
            sha256: "b".repeat(64),
            saved_tokens: 3,
        }
    }

    #[test]
    fn only_exact_token_prefixes_can_reuse_checkpoint_state() {
        let manifest = fixture();
        validate_manifest(&manifest, &manifest.identity, &[1, 2, 3]).unwrap();
        validate_manifest(&manifest, &manifest.identity, &[1, 2, 3, 4]).unwrap();
        for tokens in [vec![1, 2], vec![1, 9, 3], vec![9, 1, 2, 3]] {
            assert!(matches!(
                validate_manifest(&manifest, &manifest.identity, &tokens),
                Err(LiveError::PrefixMismatch)
            ));
        }
    }

    #[test]
    fn model_build_context_and_filename_mismatches_are_rejected() {
        let manifest = fixture();
        let mut identity = manifest.identity.clone();
        identity.engine_build = "build-2".to_owned();
        assert!(matches!(
            validate_manifest(&manifest, &identity, &manifest.tokens),
            Err(LiveError::IncompatibleIdentity)
        ));
        let mut manifest = fixture();
        manifest.filename = "../escape.bin".to_owned();
        assert!(matches!(
            validate_manifest(&manifest, &manifest.identity, &manifest.tokens),
            Err(LiveError::InvalidCheckpoint)
        ));
        let mut manifest = fixture();
        manifest.saved_tokens = 4;
        assert!(validate_manifest(&manifest, &manifest.identity, &manifest.tokens).is_err());
    }

    #[test]
    fn binary_digest_detects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("fixture.bin");
        fs::write(&filename, b"original state").unwrap();
        let first = digest_file(&filename).unwrap();
        fs::write(&filename, b"tampered state").unwrap();
        assert_ne!(first, digest_file(&filename).unwrap());
    }
}
