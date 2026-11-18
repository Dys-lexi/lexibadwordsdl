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

#[derive(Clone, Debug)]
pub struct Config {
    pub max_download_bytes: usize,
    pub max_demo_bytes: usize,
    pub max_concurrent_requests: usize,
    pub download_timeout_seconds: u64,
    /// Empty means only replay<number>.valve.net. Overrides are exact hostnames.
    pub allowed_replay_hosts: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_download_bytes: 512 * 1024 * 1024,
            max_demo_bytes: 2 * 1024 * 1024 * 1024,
            max_concurrent_requests: 1,
            download_timeout_seconds: 300,
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
        Ok(Self {
            max_download_bytes: positive("MAX_DOWNLOAD_BYTES", defaults.max_download_bytes)?,
            max_demo_bytes: positive("MAX_DEMO_BYTES", defaults.max_demo_bytes)?,
            max_concurrent_requests: positive(
                "MAX_CONCURRENT_REQUESTS",
                defaults.max_concurrent_requests,
            )?,
            download_timeout_seconds: positive("DOWNLOAD_TIMEOUT_SECONDS", 300)? as u64,
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
    client: Client,
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
        Self::new(status, "download_failed", error.to_string())
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
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(config.download_timeout_seconds))
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
        client,
        slots: Arc::new(Semaphore::new(config.max_concurrent_requests)),
        config: Arc::new(config),
    };
    Ok(Router::new()
        .route("/", post(parse))
        .route("/parse", post(parse))
        .route(
            "/health",
            get(|| async {
                Json(json!({
                    "status": "ok", "runners": [Runner::ChatMessages, Runner::DemoCheck]
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
    let mut response = state
        .client
        .get(url)
        .header(header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(ApiError::upstream)?;
    if !response.status().is_success() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "download_failed",
            format!("replay server returned HTTP {}", response.status()),
        ));
    }
    let limit = state.config.max_download_bytes;
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "download_too_large",
            "compressed demo exceeds MAX_DOWNLOAD_BYTES",
        ));
    }
    let mut compressed = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ApiError::upstream)? {
        if chunk.len() > limit.saturating_sub(compressed.len()) {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "download_too_large",
                "compressed demo exceeds MAX_DOWNLOAD_BYTES",
            ));
        }
        compressed.extend_from_slice(&chunk);
    }
    let config = Arc::clone(&state.config);
    // The permit stays with the actual worker even if its HTTP client disconnects.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let demo = decompress(compressed, config.max_demo_bytes)?;
        let results =
            runners::analyze(&demo, &request.run, request.match_start_unix).map_err(|error| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "parse_failed",
                    error.to_string(),
                )
            })?;
        let body = serde_json::to_string_pretty(&json!({"results": results})).map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serialization_failed",
                error.to_string(),
            )
        })?;
        Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
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
