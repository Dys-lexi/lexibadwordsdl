//! Outbound coordinator protocol for replay workers.

use std::{collections::VecDeque, env, io, num::NonZeroUsize, time::Duration};

use reqwest::{
    Client, Response, Url,
    header::{self, HeaderValue},
};
use serde_json::json;
use tokio::{
    task::JoinSet,
    time::{Instant, sleep, sleep_until},
};

use super::{Config, ParseRequest, Runner, app_state, process_demo};

type Error = Box<dyn std::error::Error + Send + Sync>;
const EMPTY_QUEUE_DELAY: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_secs(2);

const MAX_SUBMISSION_SIZE: usize = 50;

struct Coordinator {
    client: Client,
    get_urls: Url,
    submit_demo: Url,
    surplus: usize,
    max_finished: NonZeroUsize,
}

impl Coordinator {
    fn from_env() -> Result<Self, Error> {
        fn setting(name: &str, default: &str) -> Result<String, io::Error> {
            match env::var(name) {
                Ok(value) if !value.is_empty() => Ok(value),
                Ok(_) | Err(env::VarError::NotPresent) => Ok(default.to_owned()),
                Err(env::VarError::NotUnicode(_)) => {
                    Err(io::Error::other(format!("{name} must be valid UTF-8")))
                }
            }
        }
        let endpoint = setting("COORDINATOR_ENDPOINT", "https://deadlock.lexi.tf:443/haste")?;
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
        let token = setting("TOKEN", "unknown")?;
        if token.trim().is_empty() {
            return Err("TOKEN must contain a non-whitespace character".into());
        }
        let mut token = HeaderValue::from_str(&token)?;
        token.set_sensitive(true);
        let surplus = setting("DEMO_SURPLUS", "10")?
            .parse::<usize>()
            .map_err(|_| io::Error::other("DEMO_SURPLUS must be a nonnegative integer"))?;
        let max_finished = setting("MAX_FINISHED_DEMOS", "50")?
            .parse::<NonZeroUsize>()
            .map_err(|_| io::Error::other("MAX_FINISHED_DEMOS must be a positive integer"))?;
        Ok(Self {
            client: Client::builder()
                .default_headers(
                    [(header::HeaderName::from_static("token"), token)]
                        .into_iter()
                        .collect(),
                )
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(24 * 60 * 60))
                .build()?,
            get_urls: route("geturls"),
            submit_demo: route("submitdemo"),
            surplus,
            max_finished,
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

    async fn get_urls(
        &self,
        requested_count: usize,
        finished_count: usize,
    ) -> Result<Vec<String>, Error> {
        let response = self
            .post(
                &self.get_urls,
                json!({"count": requested_count, "finished_count": finished_count}).to_string(),
            )
            .await?;
        let body = response.bytes().await?;
        if body.is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_slice(&body)?)
    }

    async fn submit_batch(&self, body: String) {
        let mut delay = RETRY_DELAY;
        loop {
            match self.post(&self.submit_demo, body.clone()).await {
                Ok(_) => return,
                Err(error) => {
                    eprintln!("submitdemo failed, retrying in {delay:?}: {error}");
                    sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(60));
                }
            }
        }
    }
}

pub async fn run(config: Config, shutdown: impl Future<Output = ()>) -> Result<(), Error> {
    let coordinator = Coordinator::from_env()?;
    let capacity = config.max_concurrent_requests;
    let target = capacity
        .checked_add(coordinator.surplus)
        .ok_or("DEMO_SURPLUS is too large")?;
    let state = app_state(config)?;
    let mut workers = JoinSet::new();
    let mut waiting = VecDeque::new();
    // The front of the queue stays here until the coordinator acknowledges its upload.
    let mut finished: VecDeque<String> = VecDeque::new();
    let mut submission = None;
    let mut next_poll = Instant::now();
    let mut refill = None;
    let mut stopping = false;
    tokio::pin!(shutdown);
    loop {
        if submission.is_none() && !finished.is_empty() {
            let count = finished.len().min(MAX_SUBMISSION_SIZE);
            let mut body = String::from("[");
            for (index, demo) in finished.iter().take(count).enumerate() {
                if index > 0 {
                    body.push(',');
                }
                body.push_str(demo);
            }
            body.push(']');
            submission = Some((count, Box::pin(coordinator.submit_batch(body))));
        }
        // Reserve room for each active parse, even if every worker finishes together.
        let parse_slots = coordinator.max_finished.get() - finished.len();
        while workers.len() < capacity.min(parse_slots) {
            let Some(url) = waiting.pop_front() else {
                break;
            };
            let state = state.clone();
            workers.spawn(async move {
                let request = ParseRequest {
                    url,
                    run: vec![Runner::ChatMessages],
                    match_start_unix: None,
                };
                process_demo(&state, request).await
            });
        }
        if stopping && workers.is_empty() && refill.is_none() && finished.is_empty() {
            return Ok(());
        }
        let missing = (target - workers.len()).saturating_sub(waiting.len());
        tokio::select! {
            biased;
            _ = &mut shutdown, if !stopping => stopping = true,
            _ = async { submission.as_mut().unwrap().1.as_mut().await }, if submission.is_some() => {
                let (count, _) = submission.take().unwrap();
                finished.drain(..count);
            }
            Some(worker) = workers.join_next(), if !workers.is_empty() => {
                match worker? {
                    Ok(body) => finished.push_back(body),
                    Err(error) => eprintln!("replay failed ({}): {}", error.code, error.message),
                }
            }
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
                refill = Some(Box::pin(coordinator.get_urls(missing, finished.len())));
            }
        }
    }
}
