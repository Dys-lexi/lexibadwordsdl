//! Print pretty JSON match details, chat messages, and death events from an uncompressed demo.
//! cargo run --release --example deadlock-chat-messages -- <demo.dem> [match-start-unix]
use std::fs::File;
use std::io::{self, BufReader, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: deadlock-chat-messages <demo.dem> [match-start-unix]")?;
    let start = args.next().map(|s| s.parse::<f64>()).transpose()?;
    if args.next().is_some() || start.is_some_and(|n| !n.is_finite()) {
        return Err("invalid arguments".into());
    }
    let result = haste::runners::chat_messages::run(BufReader::new(File::open(path)?), start)?;
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &result)?;
    writeln!(stdout)?;
    Ok(())
}
