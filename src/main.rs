#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cache_cab::coding::CodingLab;
use cache_cab::demo::Demo;
use cache_cab::live::{checkpoint_probe, ProbeReport};
use cache_cab::llama::LlamaClient;
use cache_cab::repo_task::RepositoryLab;
use serde::Deserialize;

#[derive(Clone)]
struct App {
    demo: Arc<Mutex<Demo>>,
    live: Option<Arc<tokio::sync::Mutex<LiveLab>>>,
    data_path: PathBuf,
    port: u16,
}

struct LiveLab {
    client: LlamaClient,
    directory: PathBuf,
    report: Option<ProbeReport>,
    coding: Option<CodingLab>,
    repository: Option<RepositoryLab>,
}

enum ApiError {
    BadRequest(&'static str),
    Unavailable(&'static str),
    Conflict(&'static str),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message.to_owned()),
            Self::Unavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, message.to_owned()),
            Self::Conflict(message) => (StatusCode::CONFLICT, message.to_owned()),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

async fn operate(
    app: App,
    operation: impl FnOnce(&mut Demo) -> Result<()> + Send + 'static,
) -> Result<Json<serde_json::Value>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let mut demo = app
            .demo
            .lock()
            .map_err(|_| anyhow::anyhow!("Demo state lock failed"))?;
        operation(&mut demo)?;
        Ok::<_, anyhow::Error>(Json(serde_json::to_value(demo.snapshot()?)?))
    })
    .await
    .map_err(|error| ApiError::Internal(error.to_string()))?
    .map_err(|error| ApiError::Internal(error.to_string()))
}

async fn live_state(State(app): State<App>) -> Json<serde_json::Value> {
    let Some(live) = app.live else {
        return Json(serde_json::json!({"configured":false,"busy":false,"report":null}));
    };
    let response = match live.try_lock() {
        Ok(lab) => Json(serde_json::json!({"configured":true,"busy":false,"report":lab.report})),
        Err(_) => Json(serde_json::json!({"configured":true,"busy":true,"report":null})),
    };
    response
}

async fn run_live(State(app): State<App>) -> Result<Json<ProbeReport>, ApiError> {
    let live = app.live.ok_or(ApiError::Unavailable(
        "No dedicated inference engine is configured",
    ))?;
    let mut lab = live
        .try_lock()
        .map_err(|_| ApiError::Conflict("A live checkpoint test is already running"))?;
    if lab.coding.is_some() || lab.repository.is_some() {
        return Err(ApiError::Conflict(
            "Reset the coding conversation before running an unrelated slot probe",
        ));
    }
    let report = checkpoint_probe(lab.client.clone(), &lab.directory)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    lab.report = Some(report.clone());
    Ok(Json(report))
}

async fn coding_state(State(app): State<App>) -> Json<serde_json::Value> {
    let Some(live) = app.live else {
        return Json(serde_json::json!({"configured":false,"busy":false,"report":null}));
    };
    let response = match live.try_lock() {
        Ok(lab) => Json(
            serde_json::json!({"configured":true,"busy":false,"report":lab.coding.as_ref().map(|coding| &coding.report)}),
        ),
        Err(_) => Json(serde_json::json!({"configured":true,"busy":true,"report":null})),
    };
    response
}

async fn coding_step(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    let live = app.live.ok_or(ApiError::Unavailable(
        "No dedicated coding engine is configured",
    ))?;
    let mut lab = live
        .try_lock()
        .map_err(|_| ApiError::Conflict("A live experiment is already running"))?;
    if lab.repository.is_some() {
        return Err(ApiError::Conflict(
            "Reset repository task before using the same engine slot",
        ));
    }
    if lab.coding.is_none() {
        lab.coding = Some(CodingLab::new(lab.client.clone(), &lab.directory));
    }
    let coding = lab.coding.as_mut().unwrap();
    if let Err(error) = coding.advance().await {
        coding.report.stage = "failed".to_owned();
        let _ = coding.clear().await;
        return Err(ApiError::Internal(error.to_string()));
    }
    Ok(Json(
        serde_json::to_value(&coding.report)
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    ))
}

async fn coding_reset(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    let live = app.live.ok_or(ApiError::Unavailable(
        "No dedicated coding engine is configured",
    ))?;
    let mut lab = live
        .try_lock()
        .map_err(|_| ApiError::Conflict("A live experiment is already running"))?;
    if let Some(coding) = lab.coding.as_mut() {
        coding
            .clear()
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?;
    }
    lab.coding = None;
    Ok(Json(
        serde_json::json!({"configured":true,"busy":false,"report":null}),
    ))
}

async fn snapshot(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    operate(app, |_| Ok(())).await
}

async fn repository_state(State(app): State<App>) -> Json<serde_json::Value> {
    let Some(live) = app.live else {
        return Json(serde_json::json!({"configured":false,"busy":false,"report":null}));
    };
    let response = match live.try_lock() {
        Ok(lab) => Json(
            serde_json::json!({"configured":true,"busy":false,"report":lab.repository.as_ref().map(|task|&task.report)}),
        ),
        Err(_) => Json(serde_json::json!({"configured":true,"busy":true,"report":null})),
    };
    response
}

async fn repository_step(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    let live = app.live.ok_or(ApiError::Unavailable(
        "No dedicated repository-task engine configured",
    ))?;
    let mut lab = live
        .try_lock()
        .map_err(|_| ApiError::Conflict("A live experiment is running"))?;
    if lab.coding.is_some() {
        return Err(ApiError::Conflict(
            "Reset coding conversation before starting repository task",
        ));
    }
    if lab.repository.is_none() {
        lab.repository = Some(
            RepositoryLab::new(lab.client.clone(), &lab.directory)
                .map_err(|error| ApiError::Internal(error.to_string()))?,
        );
    }
    let task = lab.repository.as_mut().unwrap();
    if let Err(error) = task.advance().await {
        task.report.stage = "failed".to_owned();
        let _ = task.clear().await;
        return Err(ApiError::Internal(error.to_string()));
    }
    Ok(Json(
        serde_json::to_value(&task.report)
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    ))
}

async fn repository_reset(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    let live = app.live.ok_or(ApiError::Unavailable(
        "No dedicated repository-task engine configured",
    ))?;
    let mut lab = live
        .try_lock()
        .map_err(|_| ApiError::Conflict("A live experiment is running"))?;
    if let Some(task) = lab.repository.as_mut() {
        task.clear()
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?;
    }
    lab.repository = None;
    Ok(Json(
        serde_json::json!({"configured":true,"busy":false,"report":null}),
    ))
}

async fn step(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    operate(app, |demo| {
        demo.advance()?;
        Ok(())
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reset {
    budget: u32,
}

async fn reset(
    State(app): State<App>,
    Json(request): Json<Reset>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !(6..=12).contains(&request.budget) {
        return Err(ApiError::BadRequest(
            "Budget must be between 6 and 12 modeled units",
        ));
    }
    let data_path = app.data_path.clone();
    operate(app, move |demo| {
        *demo = Demo::new(&data_path, request.budget)?;
        Ok(())
    })
    .await
}

fn allowed_host(host: &str, port: u16) -> bool {
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

async fn local_only(State(app): State<App>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .unwrap_or_default();
    if !allowed_host(host, app.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let permitted = origin
            .to_str()
            .ok()
            .and_then(|origin| origin.strip_prefix("http://"))
            .is_some_and(|origin| allowed_host(origin, app.port));
        if !permitted {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn icon(Path(name): Path<String>) -> Response {
    let source = match name.as_str() {
        "car-taxi-front.svg" => include_str!("../web/icons/car-taxi-front.svg"),
        "play.svg" => include_str!("../web/icons/play.svg"),
        "pause.svg" => include_str!("../web/icons/pause.svg"),
        "rotate-ccw.svg" => include_str!("../web/icons/rotate-ccw.svg"),
        "step-forward.svg" => include_str!("../web/icons/step-forward.svg"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, "image/svg+xml")], source).into_response()
}

fn router(app: App) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/app.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route("/icons/:name", get(icon))
        .route("/api/state", get(snapshot))
        .route("/api/step", post(step))
        .route("/api/reset", post(reset))
        .route("/api/live/state", get(live_state))
        .route("/api/live/probe", post(run_live))
        .route("/api/coding/state", get(coding_state))
        .route("/api/coding/step", post(coding_step))
        .route("/api/coding/reset", post(coding_reset))
        .route("/api/repository/state", get(repository_state))
        .route("/api/repository/step", post(repository_step))
        .route("/api/repository/reset", post(repository_reset))
        .layer(DefaultBodyLimit::max(4_096))
        .layer(middleware::from_fn_with_state(app.clone(), local_only))
        .with_state(app)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut port = 7331_u16;
    let mut data_path = PathBuf::from(".cache-cab");
    let mut engine = None;
    let mut slot_directory = None;
    for argument in std::env::args().skip(1) {
        if let Some(value) = argument.strip_prefix("--port=") {
            port = value.parse().context("Invalid port")?;
        } else if let Some(value) = argument.strip_prefix("--data-dir=") {
            data_path = PathBuf::from(value);
        } else if let Some(value) = argument.strip_prefix("--engine=") {
            engine = Some(value.to_owned());
        } else if let Some(value) = argument.strip_prefix("--slot-dir=") {
            slot_directory = Some(PathBuf::from(value));
        } else if argument == "--help" {
            println!("cache-cab [--port=7331] [--data-dir=PATH] [--engine=http://127.0.0.1:7332 --slot-dir=PATH]\nDefault: simulator. Optional live probe uses and clears slot 0 of a dedicated llama.cpp server.");
            return Ok(());
        } else {
            bail!("Unknown argument: {argument}");
        }
    }
    if port == 0 {
        bail!("Choose a nonzero localhost port");
    }
    let demo = Demo::new(&data_path, 8)?;
    let live = match (engine, slot_directory) {
        (None, None) => None,
        (Some(engine), Some(directory)) => Some(Arc::new(tokio::sync::Mutex::new(LiveLab {
            client: LlamaClient::new(&engine, Duration::from_secs(120))?,
            directory,
            report: None,
            coding: None,
            repository: None,
        }))),
        _ => bail!("--engine and --slot-dir must be supplied together"),
    };
    let app = App {
        demo: Arc::new(Mutex::new(demo)),
        live,
        data_path,
        port,
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("Bind {address}; choose another --port if occupied"))?;
    println!("Cache Cab: http://{address}\nDeterministic simulation. Logical budgets and timings are modeled; JSON checkpoints are real.");
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use tower::ServiceExt;

    fn test_app(directory: &std::path::Path) -> Router {
        router(App {
            demo: Arc::new(Mutex::new(Demo::new(directory, 8).unwrap())),
            live: None,
            data_path: directory.to_owned(),
            port: 7331,
        })
    }

    fn request(method: &str, path: &str, body: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "127.0.0.1:7331")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn api_steps_resets_and_rejects_invalid_budgets_without_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let app = test_app(directory.path());
        let response = app
            .clone()
            .oneshot(request("POST", "/api/step", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let state: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                .unwrap();
        assert_eq!(state["step"], 1);
        assert_eq!(state["mode"], "deterministic_simulation");
        let invalid = app
            .clone()
            .oneshot(request("POST", "/api/reset", r#"{"budget":1}"#))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let response = app
            .clone()
            .oneshot(request("GET", "/api/state", ""))
            .await
            .unwrap();
        assert!(response
            .headers()
            .contains_key(header::CONTENT_SECURITY_POLICY));
        let state: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                .unwrap();
        assert_eq!(state["step"], 1);
        let response = app
            .oneshot(request("POST", "/api/reset", r#"{"budget":10}"#))
            .await
            .unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                .unwrap();
        assert_eq!(state["step"], 0);
        assert_eq!(state["budget"], 10);
    }

    #[tokio::test]
    async fn live_operations_are_unavailable_without_an_explicit_engine() {
        let directory = tempfile::tempdir().unwrap();
        let app = test_app(directory.path());
        let response = app
            .clone()
            .oneshot(request("GET", "/api/live/state", ""))
            .await
            .unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                .unwrap();
        assert_eq!(state["configured"], false);
        for path in [
            "/api/live/probe",
            "/api/coding/step",
            "/api/coding/reset",
            "/api/repository/step",
            "/api/repository/reset",
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(request("POST", path, ""))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }

    #[tokio::test]
    async fn cross_origin_and_rebound_hosts_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let app = test_app(directory.path());
        let mut rebound = request("GET", "/api/state", "");
        rebound.headers_mut().insert(
            header::HOST,
            HeaderValue::from_static("attacker.example:7331"),
        );
        assert_eq!(
            app.clone().oneshot(rebound).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let mut cross_origin = request("POST", "/api/step", "");
        cross_origin.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        assert_eq!(
            app.oneshot(cross_origin).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn only_loopback_hosts_with_the_expected_port_are_accepted() {
        assert!(allowed_host("127.0.0.1:7331", 7331));
        assert!(allowed_host("localhost:7331", 7331));
        assert!(!allowed_host("example.com:7331", 7331));
        assert!(!allowed_host("127.0.0.1:9000", 7331));
        assert!(serde_json::from_str::<Reset>(r#"{"budget":8,"unexpected":true}"#).is_err());
    }
}
