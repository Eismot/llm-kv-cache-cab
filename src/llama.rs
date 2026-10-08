use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, Uri};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("use an HTTP loopback IP address with an explicit port and no path")]
    InvalidEndpoint,
    #[error("invalid checkpoint filename")]
    InvalidFilename,
    #[error("prompt tokens must contain 1-65536 token IDs")]
    InvalidTokens,
    #[error("engine operation timed out")]
    Timeout,
    #[error("engine transport: {0}")]
    Transport(String),
    #[error("engine returned HTTP {0}")]
    Status(u16),
    #[error("engine JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("engine protocol: {0}")]
    Protocol(&'static str),
}

#[derive(Debug, Deserialize)]
pub struct ServerInfo {
    pub model_path: String,
    pub build_info: String,
}

#[derive(Debug, Deserialize)]
pub struct Slot {
    pub id: u32,
    pub n_ctx: u32,
    pub is_processing: bool,
}

#[derive(Debug, Deserialize)]
struct Tokenization {
    tokens: Vec<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Deserialize)]
struct Template {
    prompt: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PrefillTimings {
    pub cache_n: u32,
    pub prompt_n: u32,
    pub prompt_ms: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Prefill {
    pub tokens_cached: u32,
    pub tokens_evaluated: u32,
    pub timings: PrefillTimings,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GenerationTimings {
    pub cache_n: u32,
    pub prompt_n: u32,
    pub prompt_ms: f64,
    pub predicted_n: u32,
    pub predicted_ms: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Generation {
    pub content: String,
    pub tokens: Vec<u32>,
    pub stop_type: String,
    pub timings: GenerationTimings,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SavedSlot {
    pub id_slot: u32,
    pub n_saved: u32,
    pub n_written: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestoredSlot {
    pub id_slot: u32,
    pub n_restored: u32,
    pub n_read: u64,
}

#[derive(Debug, Deserialize)]
struct ErasedSlot {
    id_slot: u32,
}

#[derive(Clone)]
pub struct LlamaClient {
    base: String,
    http: Client<HttpConnector, Full<Bytes>>,
    timeout: Duration,
}

impl LlamaClient {
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self, EngineError> {
        let uri: Uri = endpoint.parse().map_err(|_| EngineError::InvalidEndpoint)?;
        let host = uri.host().ok_or(EngineError::InvalidEndpoint)?;
        let address: IpAddr = host
            .trim_matches(['[', ']'])
            .parse()
            .map_err(|_| EngineError::InvalidEndpoint)?;
        if uri.scheme_str() != Some("http")
            || !address.is_loopback()
            || uri.port_u16().map_or(true, |port| port == 0)
            || !matches!(uri.path(), "" | "/")
            || uri.query().is_some()
            || timeout.is_zero()
        {
            return Err(EngineError::InvalidEndpoint);
        }
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(Duration::from_secs(3)));
        let http = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(0)
            .build(connector);
        Ok(Self {
            base: endpoint.trim_end_matches('/').to_owned(),
            http,
            timeout,
        })
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, EngineError> {
        let uri: Uri = format!("{}{path}", self.base)
            .parse()
            .map_err(|_| EngineError::InvalidEndpoint)?;
        let bytes = body
            .map(|body| serde_json::to_vec(&body))
            .transpose()?
            .unwrap_or_default();
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(bytes)))
            .map_err(|error| EngineError::Transport(error.to_string()))?;
        tokio::time::timeout(self.timeout, async {
            let response = self
                .http
                .request(request)
                .await
                .map_err(|error| EngineError::Transport(error.to_string()))?;
            if !response.status().is_success() {
                return Err(EngineError::Status(response.status().as_u16()));
            }
            let bytes = Limited::new(response.into_body(), 8 * 1024 * 1024)
                .collect()
                .await
                .map_err(|error| EngineError::Transport(error.to_string()))?
                .to_bytes();
            Ok(serde_json::from_slice(&bytes)?)
        })
        .await
        .map_err(|_| EngineError::Timeout)?
    }

    pub async fn info(&self) -> Result<ServerInfo, EngineError> {
        self.json(Method::GET, "/props", None).await
    }

    pub async fn exclusive_slot(&self) -> Result<Slot, EngineError> {
        let mut slots: Vec<Slot> = self.json(Method::GET, "/slots", None).await?;
        if slots.len() != 1 {
            return Err(EngineError::Protocol(
                "a dedicated single-slot engine is required",
            ));
        }
        let slot = slots.pop().ok_or(EngineError::Protocol("slot missing"))?;
        if slot.id != 0 || slot.is_processing || slot.n_ctx == 0 {
            return Err(EngineError::Protocol(
                "slot 0 must be idle and have a context budget",
            ));
        }
        Ok(slot)
    }

    pub async fn tokenize(&self, prompt: &str) -> Result<Vec<u32>, EngineError> {
        self.tokenize_with_special_tokens(prompt, true).await
    }

    pub async fn tokenize_fragment(&self, prompt: &str) -> Result<Vec<u32>, EngineError> {
        self.tokenize_with_special_tokens(prompt, false).await
    }

    pub async fn chat_tokens(&self, messages: &[Message]) -> Result<Vec<u32>, EngineError> {
        if messages.is_empty()
            || messages.len() > 16
            || messages
                .iter()
                .any(|message| message.content.len() > 65_536)
        {
            return Err(EngineError::InvalidTokens);
        }
        let template: Template = self
            .json(
                Method::POST,
                "/apply-template",
                Some(serde_json::json!({"messages":messages})),
            )
            .await?;
        let tokens: Tokenization = self.json(Method::POST, "/tokenize", Some(serde_json::json!({"content":template.prompt,"add_special":false,"parse_special":true,"with_pieces":false}))).await?;
        validate_tokens(&tokens.tokens)?;
        Ok(tokens.tokens)
    }

    async fn tokenize_with_special_tokens(
        &self,
        prompt: &str,
        add_special: bool,
    ) -> Result<Vec<u32>, EngineError> {
        if prompt.is_empty() || prompt.len() > 1_048_576 {
            return Err(EngineError::InvalidTokens);
        }
        let tokens: Tokenization = self.json(Method::POST, "/tokenize", Some(serde_json::json!({ "content": prompt, "add_special": add_special, "parse_special": false, "with_pieces": false }))).await?;
        validate_tokens(&tokens.tokens)?;
        Ok(tokens.tokens)
    }

    pub async fn prefill(&self, tokens: &[u32]) -> Result<Prefill, EngineError> {
        validate_tokens(tokens)?;
        let result: Prefill = self.json(Method::POST, "/completion", Some(serde_json::json!({ "prompt": tokens, "id_slot": 0, "n_predict": 0, "cache_prompt": true, "return_tokens": true }))).await?;
        if result.tokens_cached as usize > tokens.len()
            || u64::from(result.timings.cache_n) + u64::from(result.timings.prompt_n)
                > tokens.len() as u64
            || !result.timings.prompt_ms.is_finite()
            || result.timings.prompt_ms < 0.0
        {
            return Err(EngineError::Protocol("invalid prefill accounting"));
        }
        Ok(result)
    }

    pub async fn save(&self, filename: &str) -> Result<SavedSlot, EngineError> {
        validate_filename(filename)?;
        let result: SavedSlot = self
            .json(
                Method::POST,
                "/slots/0?action=save",
                Some(serde_json::json!({ "filename": filename })),
            )
            .await?;
        if result.id_slot != 0 || result.n_saved == 0 || result.n_written == 0 {
            return Err(EngineError::Protocol("invalid saved-slot accounting"));
        }
        Ok(result)
    }

    pub async fn generate(
        &self,
        tokens: &[u32],
        max_tokens: u32,
    ) -> Result<Generation, EngineError> {
        validate_tokens(tokens)?;
        if !(1..=256).contains(&max_tokens) {
            return Err(EngineError::InvalidTokens);
        }
        let result: Generation = self.json(Method::POST, "/completion", Some(serde_json::json!({
            "prompt":tokens,"id_slot":0,"n_predict":max_tokens,"cache_prompt":true,"return_tokens":true,
            "temperature":0.0,"seed":1,"stream":false
        }))).await?;
        if result.tokens.is_empty()
            || result.tokens.len() > max_tokens as usize + 1
            || result.content.len() > 1_048_576
            || !result.timings.prompt_ms.is_finite()
            || result.timings.prompt_ms < 0.0
            || !result.timings.predicted_ms.is_finite()
            || result.timings.predicted_ms < 0.0
        {
            return Err(EngineError::Protocol("invalid bounded generation response"));
        }
        Ok(result)
    }

    pub async fn restore(&self, filename: &str) -> Result<RestoredSlot, EngineError> {
        validate_filename(filename)?;
        let result: RestoredSlot = self
            .json(
                Method::POST,
                "/slots/0?action=restore",
                Some(serde_json::json!({ "filename": filename })),
            )
            .await?;
        if result.id_slot != 0 || result.n_restored == 0 || result.n_read == 0 {
            return Err(EngineError::Protocol("invalid restored-slot accounting"));
        }
        Ok(result)
    }

    pub async fn erase(&self) -> Result<(), EngineError> {
        let result: ErasedSlot = self
            .json(
                Method::POST,
                "/slots/0?action=erase",
                Some(serde_json::json!({})),
            )
            .await?;
        if result.id_slot != 0 {
            return Err(EngineError::Protocol("unexpected erased slot"));
        }
        Ok(())
    }
}

fn validate_tokens(tokens: &[u32]) -> Result<(), EngineError> {
    if tokens.is_empty() || tokens.len() > 65_536 {
        return Err(EngineError::InvalidTokens);
    }
    Ok(())
}

fn validate_filename(filename: &str) -> Result<(), EngineError> {
    if filename.len() < 5
        || filename.len() > 128
        || !filename.ends_with(".bin")
        || filename.starts_with('.')
        || !filename.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
        })
    {
        return Err(EngineError::InvalidFilename);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        routing::{get, post},
        Json, Router,
    };

    #[test]
    fn endpoints_and_names_are_restricted_to_the_local_protocol() {
        for endpoint in [
            "http://example.com:8080",
            "https://127.0.0.1:8080",
            "http://127.0.0.1",
            "http://127.0.0.1:8080/proxy",
            "http://127.0.0.1:8080/?query=true",
        ] {
            assert!(LlamaClient::new(endpoint, Duration::from_secs(1)).is_err());
        }
        assert!(LlamaClient::new("http://127.0.0.1:8080", Duration::from_secs(1)).is_ok());
        for filename in [
            "../escape.bin",
            "/absolute.bin",
            "Bad.bin",
            ".hidden.bin",
            "state.json",
        ] {
            assert!(validate_filename(filename).is_err());
        }
        assert!(validate_filename("checkpoint-01.bin").is_ok());
    }

    #[tokio::test]
    async fn generation_is_bounded_and_returns_actual_token_ids() {
        let app = Router::new().route("/completion", post(|Json(body): Json<serde_json::Value>| async move {
            assert_eq!(body["n_predict"], 64);
            assert_eq!(body["return_tokens"], true);
            assert_eq!(body["temperature"], 0.0);
            Json(serde_json::json!({"content":"value.checked_add(1)","tokens":[10,11,12],"stop_type":"eos","timings":{"cache_n":0,"prompt_n":3,"prompt_ms":1.0,"predicted_n":3,"predicted_ms":2.0}}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client =
            LlamaClient::new(&format!("http://{address}"), Duration::from_secs(2)).unwrap();
        assert!(client.generate(&[1, 2, 3], 0).await.is_err());
        assert!(client.generate(&[1, 2, 3], 257).await.is_err());
        let generated = client.generate(&[1, 2, 3], 64).await.unwrap();
        assert_eq!(generated.tokens, [10, 11, 12]);
        assert_eq!(generated.content, "value.checked_add(1)");
        server.abort();
    }

    #[tokio::test]
    async fn client_speaks_the_actual_slot_and_prefill_contract() {
        let app = Router::new()
            .route("/props", get(|| async { Json(serde_json::json!({"model_path":"fixture.gguf","build_info":"test-build"})) }))
            .route("/slots", get(|| async { Json(serde_json::json!([{"id":0,"n_ctx":2048,"is_processing":false}])) }))
            .route("/tokenize", post(|| async { Json(serde_json::json!({"tokens":[1,2,3]})) }))
            .route("/completion", post(|Json(body): Json<serde_json::Value>| async move {
                assert_eq!(body["n_predict"], 0);
                assert_eq!(body["prompt"], serde_json::json!([1,2,3]));
                Json(serde_json::json!({"tokens_cached":2,"tokens_evaluated":3,"timings":{"cache_n":2,"prompt_n":1,"prompt_ms":1.5}}))
            }))
            .route("/slots/0", post(|axum::extract::Query(query): axum::extract::Query<BTreeMap<String,String>>| async move {
                Json(match query.get("action").map(String::as_str) {
                    Some("save") => serde_json::json!({"id_slot":0,"n_saved":3,"n_written":128}),
                    Some("restore") => serde_json::json!({"id_slot":0,"n_restored":3,"n_read":128}),
                    _ => serde_json::json!({"id_slot":0,"n_erased":3}),
                })
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client =
            LlamaClient::new(&format!("http://{address}"), Duration::from_secs(2)).unwrap();
        assert_eq!(client.info().await.unwrap().build_info, "test-build");
        assert_eq!(client.exclusive_slot().await.unwrap().n_ctx, 2048);
        let tokens = client.tokenize("a prompt").await.unwrap();
        assert_eq!(client.prefill(&tokens).await.unwrap().tokens_cached, 2);
        assert_eq!(client.save("checkpoint.bin").await.unwrap().n_saved, 3);
        client.erase().await.unwrap();
        assert_eq!(
            client.restore("checkpoint.bin").await.unwrap().n_restored,
            3
        );
        server.abort();
    }

    use std::collections::BTreeMap;
}
