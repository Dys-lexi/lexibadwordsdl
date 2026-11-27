//! Named demo analyses shared by the HTTP service and command line examples.

use std::io::{self, Cursor};

use serde_json::{Map, Value};

pub mod chat_messages;
mod death_events;
#[cfg(feature = "demo-check")]
pub mod demo_check;
mod hero_snapshots;
mod match_details;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "http-service", derive(serde::Deserialize, serde::Serialize))]
pub enum Runner {
    #[cfg_attr(feature = "http-service", serde(rename = "deadlock-chat-messages"))]
    ChatMessages,
    #[cfg(feature = "demo-check")]
    #[cfg_attr(feature = "http-service", serde(rename = "deadlock-demo-check"))]
    DemoCheck,
}

impl Runner {
    pub const fn name(self) -> &'static str {
        match self {
            Self::ChatMessages => "deadlock-chat-messages",
            #[cfg(feature = "demo-check")]
            Self::DemoCheck => "deadlock-demo-check",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    DemoHeader(#[from] crate::demofile::DemoHeaderError),
    #[error(transparent)]
    Parser(#[from] crate::parser::ParserError<io::Error>),
}

pub fn analyze(
    demo: &[u8],
    runs: &[Runner],
    match_start_unix: Option<f64>,
) -> Result<Map<String, Value>, RunError> {
    let mut results = Map::new();
    for runner in runs {
        let reader = Cursor::new(demo);
        let value = match runner {
            Runner::ChatMessages => chat_messages::run(reader, match_start_unix)?,
            #[cfg(feature = "demo-check")]
            Runner::DemoCheck => demo_check::run(reader)?,
        };
        results.insert(runner.name().to_owned(), value);
    }
    Ok(results)
}
