//! Named demo analyses shared by the HTTP service and command line examples.

use std::io::{self, Cursor};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub mod chat_messages;
mod death_events;
pub mod demo_check;
mod hero_snapshots;
mod match_details;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Runner {
    #[serde(rename = "deadlock-chat-messages")]
    ChatMessages,
    #[serde(rename = "deadlock-demo-check")]
    DemoCheck,
}

impl Runner {
    pub const fn name(self) -> &'static str {
        match self {
            Self::ChatMessages => "deadlock-chat-messages",
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
            Runner::DemoCheck => demo_check::run(reader)?,
        };
        results.insert(runner.name().to_owned(), value);
    }
    Ok(results)
}
