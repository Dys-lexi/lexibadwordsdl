//! HTTP download, in-memory decompression, and named demo analysis.

use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, State, rejection::JsonRejection};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

use crate::runners::{self, Runner};

mod download;

#[derive(Clone, Debug)]
pub struct Config {
    pub max_download_bytes: usize,
    pub max_download_bytes_per_second: u64,
    pub max_demo_bytes: usize,
    pub max_concurrent_requests: usize,
    pub download_read_timeout_seconds: u64,
    /// Empty means only replay<number>.valve.net. Overrides are exact hostnames.
    pub allowed_replay_hosts: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_download_bytes: 512 * 1024 * 1024,
            max_download_bytes_per_second: 0,
            max_demo_bytes: 2 * 1024 * 1024 * 1024,
            max_concurrent_requests: 1,
            download_read_timeout_seconds: 30,
            allowed_replay_hosts: Vec::new(),
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, io::Error> {
        fn positive(name: &str, default: usize) -> Result<usize, io::Error> {
            match std::env::var(name) {
                Ok(value) => value
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| io::Error::other(format!("{name} must be a positive integer"))),
                Err(std::env::VarError::NotPresent) => Ok(default),
                Err(error) => Err(io::Error::other(error)),
            }
        }
        let defaults = Self::default();
        let max_download_bytes_per_second = match std::env::var("MAX_DOWNLOAD_SPEED") {
            Ok(value) if !value.is_empty() => value
                .parse::<f64>()
                .ok()
                .filter(|speed| speed.is_finite() && *speed >= 0.0)
                .map(|speed| (speed * 125_000.0).ceil() as u64)
                .ok_or_else(|| io::Error::other("MAX_DOWNLOAD_SPEED must be nonnegative Mbps"))?,
            Ok(_) | Err(std::env::VarError::NotPresent) => defaults.max_download_bytes_per_second,
            Err(error) => return Err(io::Error::other(error)),
        };
        Ok(Self {
            max_download_bytes: positive("MAX_DOWNLOAD_BYTES", defaults.max_download_bytes)?,
            max_download_bytes_per_second,
            max_demo_bytes: positive("MAX_DEMO_BYTES", defaults.max_demo_bytes)?,
            max_concurrent_requests: positive(
                "MAX_CONCURRENT_REQUESTS",
                defaults.max_concurrent_requests,
            )?,
            download_read_timeout_seconds: positive(
                "DOWNLOAD_READ_TIMEOUT_SECONDS",
                defaults.download_read_timeout_seconds as usize,
            )? as u64,
            allowed_replay_hosts: std::env::var("ALLOWED_REPLAY_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase)
                .collect(),
        })
    }
}

#[derive(Clone)]
struct AppState {
    replay_client: Arc<download::ReplayClient>,
    config: Arc<Config>,
    slots: Arc<Semaphore>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParseRequest {
    url: String,
    run: Vec<Runner>,
    /// Optional override for the match start timestamp; otherwise read from the demo.
    match_start_unix: Option<f64>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn upstream(error: reqwest::Error) -> Self {
        let status = if error.is_timeout() {
            StatusCode::GATEWAY_TIMEOUT
        } else {
            StatusCode::BAD_GATEWAY
        };
        Self::new(status, "download_failed", error.without_url().to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": {"code": self.code, "message": self.message}})),
        )
            .into_response()
    }
}

fn url_allowed(url: &Url, overrides: &[String]) -> bool {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if !overrides.is_empty() {
        return overrides
            .iter()
            .any(|allowed| host.eq_ignore_ascii_case(allowed));
    }
    if url.port().is_some_and(|port| port != 80 && port != 443) {
        return false;
    }
    host.strip_prefix("replay")
        .and_then(|h| h.strip_suffix(".valve.net"))
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

pub fn router(config: Config) -> Result<Router, reqwest::Error> {
    let hosts = config.allowed_replay_hosts.clone();
    let client = Client::builder()
        .user_agent("haste-demo-service/0.1")
        .connect_timeout(Duration::from_secs(5))
        .retry(reqwest::retry::never())
        .default_headers(
            [(
                header::ACCEPT_ENCODING,
                header::HeaderValue::from_static("identity"),
            )]
            .into_iter()
            .collect(),
        )
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many replay redirects")
            } else if url_allowed(attempt.url(), &hosts) {
                attempt.follow()
            } else {
                attempt.error("redirect destination is not an allowed replay host")
            }
        }))
        .build()?;
    let state = AppState {
        replay_client: Arc::new(download::ReplayClient::new(client, &config)),
        slots: Arc::new(Semaphore::new(config.max_concurrent_requests)),
        config: Arc::new(config),
    };
    Ok(Router::new()
        .route("/", post(parse))
        .route("/parse", post(parse))
        .route(
            "/health",
            get(|State(state): State<AppState>| async move {
                Json(json!({
                    "status": "ok",
                    "runners": [Runner::ChatMessages, Runner::DemoCheck],
                    "max_concurrent_requests": state.config.max_concurrent_requests,
                }))
            }),
        )
        .fallback(|| async {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "use POST /parse or GET /health",
            )
        })
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state))
}

async fn parse(
    State(state): State<AppState>,
    request: Result<Json<ParseRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            error.body_text(),
        )
    })?;
    if request.run.is_empty()
        || request.run.len() > 2
        || request
            .run
            .iter()
            .enumerate()
            .any(|(i, r)| request.run[..i].contains(r))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_run",
            "run must contain one or two distinct supported runner names",
        ));
    }
    if request
        .match_start_unix
        .is_some_and(|n| !n.is_finite() || n < 0.0)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_timestamp",
            "match_start_unix must be finite, nonnegative Unix seconds",
        ));
    }
    let url = Url::parse(&request.url).map_err(|error| {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_url", error.to_string())
    })?;
    if !url_allowed(&url, &state.config.allowed_replay_hosts) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_url",
            "URL must use HTTP(S) and an allowed replay host (default: replay<number>.valve.net)",
        ));
    }
    let permit = Arc::clone(&state.slots).try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "all demo workers are busy; retry later",
        )
    })?;
    let (compressed, download_log) = download::download(
        &state.replay_client,
        url,
        &state.config,
        state.config.max_concurrent_requests - state.slots.available_permits(),
    )
    .await?;
    let config = Arc::clone(&state.config);
    // The permit stays with the actual worker even if its HTTP client disconnects.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let processing_start = std::time::Instant::now();
        download_log.event("processing_started", json!({}));
        let result: Result<Response, ApiError> = (|| {
            let demo = decompress(compressed, config.max_demo_bytes)?;
            download_log.event("decompression_completed", json!({
                "decompressed_bytes": demo.len(),
                "decompression_seconds": processing_start.elapsed().as_secs_f64(),
            }));
            let parse_start = std::time::Instant::now();
            let results =
                runners::analyze(&demo, &request.run, request.match_start_unix).map_err(|error| {
                    ApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "parse_failed",
                        error.to_string(),
                    )
                })?;
            download_log.event("parsing_completed", json!({"parse_seconds": parse_start.elapsed().as_secs_f64()}));
            let body = serde_json::to_string_pretty(&json!({"results": results})).map_err(|error| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "serialization_failed",
                    error.to_string(),
                )
            })?;
            Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
        })();
        match &result {
            Ok(_) => download_log.event("request_completed", json!({"processing_seconds": processing_start.elapsed().as_secs_f64()})),
            Err(error) => download_log.event("processing_failed", json!({
                "http_status": error.status.as_u16(), "error_code": error.code, "reason": error.message,
                "processing_seconds": processing_start.elapsed().as_secs_f64(),
            })),
        }
        result
    })
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "worker_failed",
            error.to_string(),
        )
    })?
}

fn decompress(bytes: Vec<u8>, limit: usize) -> Result<Vec<u8>, ApiError> {
    let invalid = |error: io::Error| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "decompression_failed",
            error.to_string(),
        )
    };
    let demo = if bytes.starts_with(b"PBDEMS2\0") {
        bytes
    } else {
        let reader: Box<dyn Read + '_> = if bytes.starts_with(b"BZh") {
            Box::new(bzip2::read::MultiBzDecoder::new(bytes.as_slice()))
        } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            let mut decoder =
                zstd::stream::read::Decoder::new(bytes.as_slice()).map_err(invalid)?;
            decoder.window_log_max(27).map_err(invalid)?;
            Box::new(decoder)
        } else {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_compression",
                "expected bzip2, Zstandard, or an uncompressed Source 2 demo",
            ));
        };
        let mut demo = Vec::new();
        reader
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut demo)
            .map_err(invalid)?;
        demo
    };
    if demo.len() > limit {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "demo_too_large",
            "extracted demo exceeds MAX_DEMO_BYTES",
        ));
    }
    if !demo.starts_with(b"PBDEMS2\0") {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_demo",
            "extracted data is not a Source 2 demo",
        ));
    }
    Ok(demo)
}
