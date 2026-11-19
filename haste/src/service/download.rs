use super::{ApiError, Config};
use axum::http::{StatusCode, header};
use futures_util::{StreamExt as _, stream::FuturesUnordered};
use reqwest::{Client, Response, Url};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const RANGE_WORKERS: usize = 8;
const MIN_PARALLEL_BYTES: usize = 8 * 1024 * 1024;
static NEXT_DOWNLOAD_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct DownloadLog {
    id: String,
    host: String,
    match_id: Option<u64>,
    started: Instant,
    received_bytes: AtomicU64,
    total_bytes: Option<usize>,
}

impl DownloadLog {
    fn new(url: &Url) -> Self {
        Self {
            id: format!(
                "{}-{}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros(),
                std::process::id(),
                NEXT_DOWNLOAD_ID.fetch_add(1, Ordering::Relaxed)
            ),
            host: url.host_str().unwrap_or_default().to_owned(),
            match_id: url
                .path_segments()
                .and_then(|mut parts| parts.next_back())
                .and_then(|name| name.split('_').next())
                .and_then(|id| id.parse().ok()),
            started: Instant::now(),
            received_bytes: AtomicU64::new(0),
            total_bytes: None,
        }
    }

    pub(super) fn event(&self, event: &str, fields: Value) {
        let mut record = json!({
            "event": event, "download_id": self.id, "match_id": self.match_id,
            "host": self.host,
            "timestamp_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "elapsed_seconds": self.started.elapsed().as_secs_f64(),
            "received_bytes": self.received_bytes.load(Ordering::Relaxed),
            "total_bytes": self.total_bytes,
        });
        record
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        eprintln!("{record}");
    }
}

fn failed(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, "download_failed", message)
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "download_too_large",
        "compressed demo exceeds MAX_DOWNLOAD_BYTES",
    )
}

fn content_range(response: &Response) -> Option<(usize, usize, usize)> {
    let value = response
        .headers()
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    (start <= end && end < total).then_some((start, end, total))
}

type Validator = (header::HeaderName, header::HeaderValue);

fn validator(response: &Response) -> Option<Validator> {
    let headers = response.headers();
    headers
        .get(header::ETAG)
        .filter(|value| !value.as_bytes().starts_with(b"W/"))
        .map(|value| (header::ETAG, value.clone()))
        .or_else(|| {
            headers
                .get(header::LAST_MODIFIED)
                .map(|value| (header::LAST_MODIFIED, value.clone()))
        })
}

pub(super) async fn download(
    client: &Client,
    url: Url,
    config: &Config,
    active_requests: usize,
) -> Result<(Vec<u8>, DownloadLog), ApiError> {
    let mut log = DownloadLog::new(&url);
    log.event(
        "download_started",
        json!({
            "read_timeout_seconds": config.download_read_timeout_seconds,
            "active_requests": active_requests,
            "max_concurrent_requests": config.max_concurrent_requests,
        }),
    );
    match transfer(client, url, config.max_download_bytes, &mut log).await {
        Ok(bytes) => {
            let seconds = log.started.elapsed().as_secs_f64().max(0.000001);
            let received = log.received_bytes.load(Ordering::Relaxed);
            log.event(
                "download_completed",
                json!({
                    "effective_mbps": bytes.len() as f64 * 8.0 / seconds / 1_000_000.0,
                    "transfer_mbps": received as f64 * 8.0 / seconds / 1_000_000.0,
                }),
            );
            Ok((bytes, log))
        }
        Err(error) => {
            log.event("download_failed", json!({"http_status": error.status.as_u16(), "error_code": error.code, "reason": error.message}));
            Err(error)
        }
    }
}

async fn transfer(
    client: &Client,
    url: Url,
    limit: usize,
    log: &mut DownloadLog,
) -> Result<Vec<u8>, ApiError> {
    match client
        .get(url.clone())
        .header(header::RANGE, "bytes=0-0")
        .send()
        .await
    {
        // An upstream that ignores the probe has already sent the full response.
        Ok(response) if response.status() == StatusCode::OK => {
            return read_sequential(response, limit, log).await;
        }
        Ok(response) if response.status() == StatusCode::PARTIAL_CONTENT => {
            let range = content_range(&response);
            let validator = validator(&response);
            drop(response);
            if let Some((0, 0, size)) = range {
                log.total_bytes = Some(size);
                if size > limit {
                    return Err(too_large());
                }
                if size >= MIN_PARALLEL_BYTES {
                    log.event("download_ranged_started", json!({"ranges": RANGE_WORKERS}));
                    match read_ranged(client, &url, size, validator.as_ref(), log).await {
                        Ok(bytes) => return Ok(bytes),
                        Err(error) => log.event(
                            "download_fallback",
                            json!({"stage": "ranged", "reason": error.message}),
                        ),
                    }
                }
            } else {
                log.event(
                    "download_fallback",
                    json!({"stage": "probe", "reason": "invalid replay Content-Range"}),
                );
            }
        }
        Ok(response) => log.event(
            "download_fallback",
            json!({"stage": "probe", "http_status": response.status().as_u16()}),
        ),
        Err(error) => log.event(
            "download_fallback",
            json!({"stage": "probe", "reason": error.without_url().to_string()}),
        ),
    }
    let response = client.get(url).send().await.map_err(ApiError::upstream)?;
    read_sequential(response, limit, log).await
}

async fn read_ranged(
    client: &Client,
    url: &Url,
    size: usize,
    validator: Option<&Validator>,
    log: &DownloadLog,
) -> Result<Vec<u8>, ApiError> {
    let mut bytes = vec![0; size];
    let chunk_size = size.div_ceil(RANGE_WORKERS);
    // Workers write into disjoint slices, keeping just one file-sized buffer.
    let mut transfers = FuturesUnordered::new();
    for (index, dest) in bytes.chunks_mut(chunk_size).enumerate() {
        transfers.push(async move {
            let start = index * chunk_size;
            let end = start + dest.len() - 1;
            let mut request = client
                .get(url.clone())
                .header(header::RANGE, format!("bytes={start}-{end}"));
            if let Some((_, value)) = validator {
                request = request.header(header::IF_RANGE, value.clone());
            }
            let mut response = request.send().await.map_err(ApiError::upstream)?;
            if response.status() != StatusCode::PARTIAL_CONTENT
                || content_range(&response) != Some((start, end, size))
            {
                return Err(failed("replay server did not honor the requested range"));
            }
            if let Some((name, expected)) = validator
                && response
                    .headers()
                    .get(name)
                    .is_some_and(|value| value != expected)
            {
                return Err(failed("replay changed during ranged download"));
            }
            let mut filled = 0;
            while let Some(chunk) = response.chunk().await.map_err(ApiError::upstream)? {
                log.received_bytes
                    .fetch_add(chunk.len() as u64, Ordering::Relaxed);
                if chunk.len() > dest.len() - filled {
                    return Err(failed("replay body exceeds its declared range"));
                }
                dest[filled..filled + chunk.len()].copy_from_slice(&chunk);
                filled += chunk.len();
            }
            if filled != dest.len() {
                return Err(failed("incomplete replay range"));
            }
            Ok(())
        });
    }
    while let Some(result) = transfers.next().await {
        result?;
    }
    drop(transfers);
    Ok(bytes)
}

async fn read_sequential(
    mut response: Response,
    limit: usize,
    log: &mut DownloadLog,
) -> Result<Vec<u8>, ApiError> {
    if response.status() != StatusCode::OK {
        return Err(failed(format!(
            "replay server returned HTTP {}",
            response.status()
        )));
    }
    let size = response
        .content_length()
        .map(|len| usize::try_from(len).map_err(|_| too_large()))
        .transpose()?;
    log.total_bytes = size;
    if size.is_some_and(|len| len > limit) {
        return Err(too_large());
    }
    log.event("download_sequential_started", json!({}));
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ApiError::upstream)? {
        log.received_bytes
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        if chunk.len() > limit - bytes.len() {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    if size.is_some_and(|len| len != bytes.len()) {
        return Err(failed("incomplete replay response"));
    }
    Ok(bytes)
}
