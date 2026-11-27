//! Outbound coordinator protocol for replay workers.

use std::{env, io, sync::Arc, time::Duration};

use axum::http::{HeaderValue, header};
use reqwest::{Client, Response, Url};
use serde_json::json;
use tokio::{sync::watch, task::JoinSet, time::sleep};

use super::{AppState, Config, ParseRequest, Runner, app_state, process_demo};

type Error = Box<dyn std::error::Error + Send + Sync>;
const POLL_DELAY: Duration = Duration::from_secs(2);

struct Coordinator {
    client: Client,
    get_urls: Url,
    submit_demo: Url,
    count: usize,
}

impl Coordinator {
    fn from_env() -> Result<Self, Error> {
        let endpoint = env::var("COORDINATOR_ENDPOINT")
            .map_err(|_| io::Error::other("COORDINATOR_ENDPOINT is required"))?;
        let base =
            Url::parse(&endpoint).map_err(|_| io::Error::other("invalid COORDINATOR_ENDPOINT"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host().is_none()
            || base.query().is_some()
            || base.fragment().is_some()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err("COORDINATOR_ENDPOINT must be an HTTP(S) base URL".into());
        }
        let route = |name: &str| {
            let mut url = base.clone();
            url.path_segments_mut().unwrap().pop_if_empty().push(name);
            url
        };
        let token = env::var("TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or("TOKEN is required")?;
        let mut token = HeaderValue::from_str(&token)?;
        token.set_sensitive(true);
        let surplus = env::var("DEMO_SURPLUS")
            .unwrap_or_else(|_| "0".into())
            .parse::<usize>()
            .map_err(|_| io::Error::other("DEMO_SURPLUS must be a nonnegative integer"))?;
        Ok(Self {
            client: Client::builder()
                .default_headers(
                    [(header::HeaderName::from_static("token"), token)]
                        .into_iter()
                        .collect(),
                )
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(60))
                .build()?,
            get_urls: route("geturls"),
            submit_demo: route("submitdemo"),
            count: surplus.checked_add(1).ok_or("DEMO_SURPLUS is too large")?,
        })
    }

    async fn post(&self, url: &Url, body: impl Into<reqwest::Body>) -> Result<Response, Error> {
        let response = self
            .client
            .post(url.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(format!("coordinator returned {}", response.status()).into());
        }
        Ok(response)
    }

    async fn get_urls(&self) -> Result<Vec<String>, Error> {
        let response = self
            .post(&self.get_urls, json!({"count": self.count}).to_string())
            .await?;
        Ok(serde_json::from_slice(&response.bytes().await?)?)
    }

    async fn process(&self, state: &AppState, url: String) {
        let request = ParseRequest {
            url,
            run: vec![Runner::ChatMessages],
            match_start_unix: None,
        };
        let body = match process_demo(state, request).await {
            Ok(body) => axum::body::Bytes::from(body),
            Err(error) => {
                eprintln!("replay failed ({}): {}", error.code, error.message);
                return;
            }
        };
        let mut delay = POLL_DELAY;
        while let Err(error) = self.post(&self.submit_demo, body.clone()).await {
            eprintln!("submitdemo failed, retrying in {delay:?}: {error}");
            sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(60));
        }
    }

    async fn worker(&self, state: AppState, mut stop: watch::Receiver<bool>) {
        while !*stop.borrow() {
            match self.get_urls().await {
                Ok(urls) if !urls.is_empty() => {
                    for url in urls {
                        self.process(&state, url).await;
                    }
                    continue;
                }
                Ok(_) => {}
                Err(error) => eprintln!("geturls failed: {error}"),
            }
            tokio::select! {
                _ = sleep(POLL_DELAY) => {},
                _ = stop.changed() => return,
            }
        }
    }
}

pub async fn run(config: Config, shutdown: impl Future<Output = ()>) -> Result<(), Error> {
    let coordinator = Arc::new(Coordinator::from_env()?);
    let state = app_state(config)?;
    let mut workers = JoinSet::new();
    let (stop, _) = watch::channel(false);
    for _ in 0..state.config.max_concurrent_requests {
        let coordinator = Arc::clone(&coordinator);
        let state = state.clone();
        let receiver = stop.subscribe();
        workers.spawn(async move { coordinator.worker(state, receiver).await });
    }
    tokio::select! {
        _ = shutdown => {},
        Some(worker) = workers.join_next() => {
            worker?;
            return Err("coordinator worker exited unexpectedly".into());
        },
    }
    stop.send_replace(true);
    while let Some(worker) = workers.join_next().await {
        worker?;
    }
    Ok(())
}
