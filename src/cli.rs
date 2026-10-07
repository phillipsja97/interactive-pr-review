//! Command-line entry point.

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::model::FindingBatch;

pub mod exit {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const INVALID_FINDINGS: i32 = 10;
    pub const ABANDONED: i32 = 11;
    pub const SESSION_EXISTS: i32 = 12;
    pub const INTERRUPTED: i32 = 130;
}

#[derive(Parser)]
#[command(
    name = "prr",
    version,
    about = "Human-in-the-loop adjudication of agent-proposed pull request review findings"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the JSON Schema of the findings file.
    Schema,
}

pub fn run() -> i32 {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("prr: error: {e:#}");
            exit::ERROR
        }
    }
}

fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Schema => {
            let schema = schemars::schema_for!(FindingBatch);
            println!("{}", serde_json::to_string_pretty(&schema)?);
            Ok(exit::OK)
        }
    }
}
