//! Outbound coordinator protocol for replay workers.

use std::{collections::VecDeque, env, io, time::Duration};

use axum::http::{HeaderValue, header};
use reqwest::{Client, Response, Url};
use serde_json::json;
use tokio::{
    sync::mpsc,
    task::JoinSet,
    time::{Instant, sleep, sleep_until},
};

use super::{Config, ParseRequest, Runner, app_state, process_demo};

type Error = Box<dyn std::error::Error + Send + Sync>;
const EMPTY_QUEUE_DELAY: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_secs(2);

struct Coordinator {
    client: Client,
    get_urls: Url,
    submit_demo: Url,
    surplus: usize,
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
            surplus,
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

    async fn get_urls(&self, count: usize) -> Result<Vec<String>, Error> {
        let response = self
            .post(&self.get_urls, json!({"count": count}).to_string())
            .await?;
        let body = response.bytes().await?;
        if body.is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_slice(&body)?)
    }

    async fn submit_completed(&self, mut completed: mpsc::UnboundedReceiver<String>) {
        while let Some(first) = completed.recv().await {
            let mut batch = String::from("[");
            batch.push_str(&first);
            drop(first);
            let mut delay = RETRY_DELAY;
            loop {
                // Each worker sends one complete, already serialized JSON object.
                while let Ok(result) = completed.try_recv() {
                    batch.push(',');
                    batch.push_str(&result);
                }
                batch.push(']');
                match self.post(&self.submit_demo, batch.clone()).await {
                    Ok(_) => break,
                    Err(error) => {
                        batch.pop();
                        eprintln!("submitdemo failed, retrying in {delay:?}: {error}");
                        sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(60));
                    }
                }
            }
        }
    }
}

pub async fn run(config: Config, shutdown: impl Future<Output = ()>) -> Result<(), Error> {
    let coordinator = Coordinator::from_env()?;
    let (completed, receiver) = mpsc::unbounded_channel();
    let (result, ()) = tokio::join!(
        process_urls(config, &coordinator, completed, shutdown),
        coordinator.submit_completed(receiver),
    );
    result
}

async fn process_urls(
    config: Config,
    coordinator: &Coordinator,
    completed: mpsc::UnboundedSender<String>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    let capacity = config.max_concurrent_requests;
    let target = capacity
        .checked_add(coordinator.surplus)
        .ok_or("DEMO_SURPLUS is too large")?;
    let state = app_state(config)?;
    let mut workers = JoinSet::new();
    let mut waiting = VecDeque::new();
    let mut next_poll = Instant::now();
    let mut refill = None;
    let mut stopping = false;
    tokio::pin!(shutdown);
    loop {
        while let Some(worker) = workers.try_join_next() {
            worker??;
        }
        while workers.len() < capacity {
            let Some(url) = waiting.pop_front() else {
                break;
            };
            let completed = completed.clone();
            let state = state.clone();
            workers.spawn(async move {
                let request = ParseRequest {
                    url,
                    run: vec![Runner::ChatMessages],
                    match_start_unix: None,
                };
                match process_demo(&state, request).await {
                    Ok(body) => completed
                        .send(body)
                        .map_err(|_| "submission queue closed")?,
                    Err(error) => eprintln!("replay failed ({}): {}", error.code, error.message),
                }
                Ok::<(), Error>(())
            });
        }
        if stopping && workers.is_empty() && refill.is_none() {
            return Ok(());
        }
        let missing = (target - workers.len()).saturating_sub(waiting.len());
        tokio::select! {
            biased;
            _ = &mut shutdown, if !stopping => stopping = true,
            Some(worker) = workers.join_next(), if !workers.is_empty() => worker??,
            result = async { refill.as_mut().unwrap().await }, if refill.is_some() => {
                let result: Result<Vec<String>, Error> = result;
                refill = None;
                match result {
                    Ok(urls) if !urls.is_empty() => waiting.extend(urls),
                    Ok(_) => next_poll = Instant::now() + EMPTY_QUEUE_DELAY,
                    Err(error) => {
                        eprintln!("geturls failed: {error}");
                        next_poll = Instant::now() + RETRY_DELAY;
                    }
                }
            }
            _ = sleep_until(next_poll), if !stopping && missing > 0 && refill.is_none() => {
                refill = Some(Box::pin(coordinator.get_urls(missing)));
            }
        }
    }
}
