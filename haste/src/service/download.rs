use super::{ApiError, Config};
use axum::http::{StatusCode, header};
use reqwest::{Client, Url};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::{Instant, timeout_at};

static NEXT_DOWNLOAD_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct DownloadLog {
    id: String,
    host: String,
    match_id: Option<u64>,
    started: Instant,
    attempt: usize,
    bytes: usize,
    received_bytes: u64,
    total_bytes: Option<usize>,
    retry_baseline_mbps: Option<f64>,
    original_mbps: Option<f64>,
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
            attempt: 0,
            bytes: 0,
            received_bytes: 0,
            total_bytes: None,
            retry_baseline_mbps: None,
            original_mbps: None,
        }
    }

    pub(super) fn event(&self, event: &str, fields: Value) {
        let mut record = json!({
            "event": event, "download_id": self.id, "match_id": self.match_id,
            "host": self.host,
            "timestamp_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "elapsed_seconds": self.started.elapsed().as_secs_f64(),
            "attempt": self.attempt, "restarts": self.attempt.saturating_sub(1),
            "retained_bytes": self.bytes, "received_bytes": self.received_bytes,
            "total_bytes": self.total_bytes, "original_mbps": self.original_mbps,
        });
        record
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        eprintln!("{record}");
    }

    fn compare_retry(&mut self, mbps: f64, seconds: f64) {
        if let Some(before) = self.retry_baseline_mbps.take() {
            self.event(
                "download_retry_comparison",
                json!({
                    "before_mbps": before, "after_mbps": mbps,
                    "speed_ratio": if before > 0.0 { Some(mbps / before) } else { None },
                    "measurement_seconds": seconds,
                }),
            );
        }
    }
}

fn mbps(bytes: usize, seconds: f64) -> f64 {
    bytes as f64 * 8.0 / seconds.max(0.000001) / 1_000_000.0
}

// Estimate sustained early throughput, not the first buffered burst or a peak.
// With the default 10s decision window: skip 1s, take five 1s samples, use their
// median. Keep the baseline across reconnects so a slow retry cannot lower it.
#[derive(Default)]
struct OriginalSpeed {
    mbps: Option<f64>,
    warmed_up: bool,
    samples: Vec<f64>,
}

impl OriginalSpeed {
    fn new_attempt(&mut self) {
        self.warmed_up = false;
        self.samples.clear();
    }

    fn observe(&mut self, speed: f64) -> Option<f64> {
        if self.mbps.is_some() {
            return None;
        }
        if !self.warmed_up {
            self.warmed_up = true;
            return None;
        }
        self.samples.push(speed);
        if self.samples.len() < 5 {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[2];
        if median > 0.0 {
            self.mbps = Some(median);
            Some(median)
        } else {
            // No usable baseline while stalled. Wait for sustained progress.
            self.samples.remove(0);
            None
        }
    }

    fn slow_reason(&self, speed: f64, minimum: f64, percent: f64) -> Option<&'static str> {
        let absolute = speed < minimum;
        let relative = self
            .mbps
            .is_some_and(|original| speed < original * percent / 100.0);
        match (absolute, relative) {
            (true, true) => Some("below_both_cutoffs"),
            (true, false) => Some("below_minimum_mbps"),
            (false, true) => Some("below_original_speed"),
            (false, false) => None,
        }
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

fn content_range(value: &str) -> Option<(usize, usize, usize)> {
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    (start <= end && end < total).then_some((start, end, total))
}

// Keep the original HTTP request and worker permit throughout all attempts.
// The outer deadline bounds the entire download, including reconnects.
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
            "cutoff_mbps": config.download_min_megabits_per_second,
            "min_original_speed_percent": config.download_min_original_speed_percent,
            "speed_window_seconds": config.download_speed_window_seconds,
            "max_restarts": config.download_max_restarts,
            "timeout_seconds": config.download_timeout_seconds,
            "active_requests": active_requests,
            "max_concurrent_requests": config.max_concurrent_requests,
        }),
    );
    let deadline = Instant::now() + Duration::from_secs(config.download_timeout_seconds);
    let result = timeout_at(deadline, attempts(client, url, config, &mut log))
        .await
        .unwrap_or_else(|_| {
            Err(ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "download_failed",
                "total replay download timeout exceeded",
            ))
        });
    match result {
        Ok(bytes) => {
            log.event("download_completed", json!({
                "effective_mbps": mbps(bytes.len(), log.started.elapsed().as_secs_f64()),
                "transfer_mbps": log.received_bytes as f64 * 8.0 / log.started.elapsed().as_secs_f64().max(0.000001) / 1_000_000.0,
            }));
            Ok((bytes, log))
        }
        Err(error) => {
            log.event("download_failed", json!({"http_status": error.status.as_u16(), "error_code": error.code, "reason": error.message}));
            Err(error)
        }
    }
}

async fn attempts(
    client: &Client,
    url: Url,
    config: &Config,
    log: &mut DownloadLog,
) -> Result<Vec<u8>, ApiError> {
    let window = Duration::from_secs(config.download_speed_window_seconds);
    let mut bytes = Vec::new();
    let mut validator = None;
    let mut total_size = None;
    let mut original = OriginalSpeed::default();
    let sample_interval = window / 10;
    for attempt in 0..=config.download_max_restarts {
        // Without a validator, restart from zero instead of mixing versions.
        if validator.is_none() {
            if !bytes.is_empty() {
                log.event(
                    "download_resume_reset",
                    json!({"discarded_bytes": bytes.len(), "reason": "no_validator"}),
                );
            }
            bytes.clear();
        }
        let offset = bytes.len();
        log.attempt = attempt + 1;
        log.bytes = offset;
        log.event("download_attempt_started", json!({"resume_offset": offset}));
        let attempt_start = Instant::now();
        let received_before = log.received_bytes;
        let mut request = client
            .get(url.clone())
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::CONNECTION, "close");
        if offset > 0 {
            request = request
                .header(header::RANGE, format!("bytes={offset}-"))
                .header(header::IF_RANGE, validator.as_ref().unwrap());
        }
        let mut response = match timeout_at(Instant::now() + window, request.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(ApiError::upstream(error)),
            Err(_) => {
                if attempt == config.download_max_restarts {
                    return Err(failed(
                        "replay response remained stalled after maximum restarts",
                    ));
                }
                log.event(
                    "download_retry",
                    json!({"reason": "stalled_headers", "next_attempt": attempt + 2}),
                );
                continue;
            }
        };
        log.event("download_response", json!({
            "http_status": response.status().as_u16(),
            "response_host": response.url().host_str(),
            "headers_seconds": attempt_start.elapsed().as_secs_f64(),
            "content_length": response.content_length(),
            "content_range": response.headers().get(header::CONTENT_RANGE).and_then(|h| h.to_str().ok()),
            "has_validator": response.headers().contains_key(header::ETAG) || response.headers().contains_key(header::LAST_MODIFIED),
        }));
        let expected_end;
        if response.status() == StatusCode::PARTIAL_CONTENT {
            let (start, end, total) = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|h| h.to_str().ok())
                .and_then(content_range)
                .ok_or_else(|| failed("invalid replay Content-Range"))?;
            if offset == 0 || start != offset || total_size.is_some_and(|size| size != total) {
                return Err(failed(
                    "replay resume range does not match downloaded bytes",
                ));
            }
            if total > config.max_download_bytes {
                return Err(too_large());
            }
            // If the server sends the validator again, it must still match.
            if let Some(current) = response
                .headers()
                .get(header::ETAG)
                .filter(|h| !h.as_bytes().starts_with(b"W/"))
                .or_else(|| response.headers().get(header::LAST_MODIFIED))
                && Some(current) != validator.as_ref()
            {
                return Err(failed("replay changed while resuming download"));
            }
            total_size = Some(total);
            expected_end = Some(end + 1);
        } else if response.status() == StatusCode::OK {
            // A server can ignore Range, or If-Range can detect a changed file.
            if offset > 0 {
                log.event(
                    "download_resume_reset",
                    json!({"discarded_bytes": offset, "reason": "server_returned_full_file"}),
                );
            }
            bytes.clear();
            total_size = response
                .content_length()
                .map(|n| usize::try_from(n).unwrap_or(usize::MAX));
            if total_size.is_some_and(|n| n > config.max_download_bytes) {
                return Err(too_large());
            }
            expected_end = total_size;
            validator = response
                .headers()
                .get(header::ETAG)
                .filter(|h| !h.as_bytes().starts_with(b"W/"))
                .or_else(|| response.headers().get(header::LAST_MODIFIED))
                .cloned();
        } else {
            return Err(failed(format!(
                "replay server returned HTTP {}",
                response.status()
            )));
        }
        log.bytes = bytes.len();
        log.total_bytes = total_size;
        original.new_attempt();
        let mut sample_start = Instant::now();
        let mut sample_bytes = bytes.len();
        let mut window_start = Instant::now();
        let mut window_bytes = bytes.len();
        let mut accepted_slow_transfer = false;
        let reason = loop {
            let next_check = if original.mbps.is_none() {
                (window_start + window).min(sample_start + sample_interval)
            } else {
                window_start + window
            };
            match timeout_at(next_check, response.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    if chunk.len() > config.max_download_bytes.saturating_sub(bytes.len()) {
                        return Err(too_large());
                    }
                    bytes.extend_from_slice(&chunk);
                    log.bytes = bytes.len();
                    log.received_bytes += chunk.len() as u64;
                    if expected_end.is_some_and(|end| bytes.len() > end) {
                        return Err(failed("replay body exceeds its declared range"));
                    }
                }
                Ok(Ok(None)) => {
                    if expected_end.is_some_and(|end| bytes.len() != end) {
                        break "incomplete response";
                    }
                    if total_size.is_none_or(|total| bytes.len() == total) {
                        let seconds = window_start.elapsed().as_secs_f64();
                        log.compare_retry(mbps(bytes.len() - window_bytes, seconds), seconds);
                        log.event(
                            "download_attempt_completed",
                            json!({
                                "attempt_seconds": attempt_start.elapsed().as_secs_f64(),
                                "attempt_received_bytes": log.received_bytes - received_before,
                            }),
                        );
                        return Ok(bytes);
                    }
                    break "partial response";
                }
                Ok(Err(error)) => {
                    log.event(
                        "download_body_error",
                        json!({"reason": error.without_url().to_string()}),
                    );
                    break "interrupted response";
                }
                Err(_) => {}
            }
            if original.mbps.is_none() && sample_start.elapsed() >= sample_interval {
                let sample_seconds = sample_start.elapsed().as_secs_f64();
                if let Some(baseline) =
                    original.observe(mbps(bytes.len() - sample_bytes, sample_seconds))
                {
                    log.original_mbps = Some(baseline);
                    log.event("download_baseline_established", json!({
                        "method": "median_of_five_after_warmup", "sample_count": 5,
                        "sample_interval_seconds": sample_interval.as_secs_f64(),
                        "original_cutoff_mbps": baseline * config.download_min_original_speed_percent / 100.0,
                    }));
                }
                sample_start = Instant::now();
                sample_bytes = bytes.len();
            }
            let elapsed = window_start.elapsed();
            if elapsed >= window {
                let speed = mbps(bytes.len() - window_bytes, elapsed.as_secs_f64());
                log.compare_retry(speed, elapsed.as_secs_f64());
                let slow_reason = original.slow_reason(
                    speed,
                    config.download_min_megabits_per_second,
                    config.download_min_original_speed_percent,
                );
                log.event("download_progress", json!({
                    "window_mbps": speed, "window_seconds": elapsed.as_secs_f64(),
                    "cutoff_mbps": config.download_min_megabits_per_second,
                    "min_original_speed_percent": config.download_min_original_speed_percent,
                    "percent_of_original": original.mbps.map(|baseline| speed / baseline * 100.0),
                    "original_cutoff_mbps": original.mbps.map(|baseline| baseline * config.download_min_original_speed_percent / 100.0),
                    "slow_reason": slow_reason,
                    "remaining_bytes": total_size.map(|total| total.saturating_sub(bytes.len())),
                }));
                if let Some(reason) = slow_reason {
                    if attempt < config.download_max_restarts {
                        log.retry_baseline_mbps = Some(speed);
                        break reason;
                    }
                    if !accepted_slow_transfer {
                        log.event(
                            "download_accepting_slow_transfer",
                            json!({"window_mbps": speed, "reason": reason}),
                        );
                        accepted_slow_transfer = true;
                    }
                }
                window_start = Instant::now();
                window_bytes = bytes.len();
            }
        };
        drop(response); // Close the slow connection before opening another.
        if attempt == config.download_max_restarts {
            return Err(failed(format!(
                "{reason} after maximum replay download restarts"
            )));
        }
        log.event(
            "download_retry",
            json!({
                "reason": reason, "next_attempt": attempt + 2,
                "attempt_seconds": attempt_start.elapsed().as_secs_f64(),
                "attempt_received_bytes": log.received_bytes - received_before,
                "will_resume": validator.is_some(),
            }),
        );
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::OriginalSpeed;

    #[test]
    fn baseline_ignores_startup_burst_and_single_sample_outliers() {
        let mut original = OriginalSpeed::default();
        assert_eq!(original.observe(10_000.0), None); // discarded warmup
        for speed in [100.0, 101.0, 900.0, 99.0] {
            assert_eq!(original.observe(speed), None);
        }
        assert_eq!(original.observe(100.0), Some(100.0));
        assert_eq!(
            original.slow_reason(12.0, 8.0, 25.0),
            Some("below_original_speed")
        );
        assert_eq!(original.slow_reason(26.0, 8.0, 25.0), None);
    }

    #[test]
    fn retry_does_not_lower_original_speed() {
        let mut original = OriginalSpeed::default();
        for _ in 0..6 {
            original.observe(100.0);
        }
        original.new_attempt();
        for _ in 0..10 {
            original.observe(10.0);
        }
        assert_eq!(original.mbps, Some(100.0));
        assert_eq!(
            original.slow_reason(10.0, 8.0, 25.0),
            Some("below_original_speed")
        );
    }

    #[test]
    fn insufficient_or_stalled_samples_do_not_set_a_baseline() {
        let mut original = OriginalSpeed::default();
        for _ in 0..10 {
            assert_eq!(original.observe(0.0), None);
        }
        assert_eq!(original.mbps, None);
        original.new_attempt();
        for _ in 0..4 {
            original.observe(100.0);
        }
        assert_eq!(original.mbps, None);
        original.new_attempt();
        for _ in 0..6 {
            original.observe(20.0);
        }
        assert_eq!(original.mbps, Some(20.0));
    }

    #[test]
    fn cutoffs_are_independent_and_percentage_can_be_disabled() {
        let original = OriginalSpeed {
            mbps: Some(100.0),
            ..Default::default()
        };
        assert_eq!(
            original.slow_reason(6.0, 8.0, 25.0),
            Some("below_both_cutoffs")
        );
        assert_eq!(
            original.slow_reason(6.0, 8.0, 0.0),
            Some("below_minimum_mbps")
        );
        assert_eq!(original.slow_reason(12.0, 8.0, 0.0), None);
        assert_eq!(original.slow_reason(25.0, 8.0, 25.0), None);
        assert_eq!(
            OriginalSpeed::default().slow_reason(6.0, 8.0, 25.0),
            Some("below_minimum_mbps")
        );
    }
}
