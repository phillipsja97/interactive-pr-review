//! Command-line entry point.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::git::Repo;
use crate::model::FindingBatch;
use crate::result::ReviewResult;
use crate::server::{self, ServeEnd};
use crate::session::{Session, SessionStatus, Store, Target};
use crate::validate;

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
    /// Validate Findings and open a Review session in the browser. Blocks until the Reviewer
    /// finishes, then prints the result JSON. Run it as a background task.
    Review(ReviewArgs),
    /// Print the JSON Schema of the findings file.
    Schema,
}

#[derive(Args)]
struct ReviewArgs {
    /// Findings JSON file, or `-` for stdin.
    #[arg(long)]
    findings: PathBuf,
    /// Base of the change, e.g. `main`.
    #[arg(long)]
    base: String,
    /// Head of the change.
    #[arg(long, default_value = "HEAD")]
    head: String,
    /// Print the URL instead of opening a browser.
    #[arg(long)]
    no_open: bool,
    /// Also write the composed Review as markdown to this path.
    #[arg(long)]
    export_md: Option<PathBuf>,
    /// Abandon an open session for the same change and start a new one.
    #[arg(long)]
    replace: bool,
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
        Command::Review(args) => review(args),
        Command::Schema => {
            let schema = schemars::schema_for!(FindingBatch);
            println!("{}", serde_json::to_string_pretty(&schema)?);
            Ok(exit::OK)
        }
    }
}

fn review(args: ReviewArgs) -> Result<i32> {
    let repo = Repo::discover(&std::env::current_dir()?)?;
    let head_sha = repo.rev_parse(&args.head)?;
    let base_sha = repo.merge_base(&repo.rev_parse(&args.base)?, &head_sha)?;
    let json = if args.findings.as_os_str() == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(&args.findings).with_context(|| format!("reading {}", args.findings.display()))?
    };
    let entries = match validate::parse_batch(&json).and_then(|b| validate::validate(b, &repo, &head_sha)) {
        Ok(entries) => entries,
        Err(errors) => {
            eprintln!("{}", serde_json::to_string_pretty(&serde_json::json!({ "errors": errors }))?);
            return Ok(exit::INVALID_FINDINGS);
        }
    };
    let export_md = args.export_md.map(std::path::absolute).transpose()?;
    let store = Store::from_env()?;
    let session = Session::new(
        repo.root().to_path_buf(),
        Target::Local { base_ref: args.base, head_ref: args.head },
        base_sha,
        head_sha,
        entries,
        export_md,
    );
    if let Some(mut existing) = store.find_open(&session)? {
        if !args.replace {
            eprintln!(
                "prr: session {id} is already open for this change; resume it with `prr wait {id}` or pass --replace",
                id = existing.id
            );
            return Ok(exit::SESSION_EXISTS);
        }
        let _old_lock = store
            .try_lock(&existing.id)?
            .with_context(|| format!("session {} is being served by another prr process; finish it there first", existing.id))?;
        existing.status = SessionStatus::Abandoned;
        store.save(&existing)?;
    }
    store.save(&session)?;
    let _lock = store.try_lock(&session.id)?.context("could not lock the new session")?;
    run_server(session, store, repo, !args.no_open)
}

/// Serves the session until it ends, then prints the result and returns the exit code.
pub(crate) fn run_server(session: Session, store: Store, repo: Repo, open_browser: bool) -> Result<i32> {
    let runtime = tokio::runtime::Runtime::new()?;
    match runtime.block_on(server::serve_session(session, store, repo, open_browser))? {
        ServeEnd::Interrupted(s) => {
            eprintln!("prr: interrupted; session {id} is still open — resume with `prr wait {id}`", id = s.id);
            Ok(exit::INTERRUPTED)
        }
        ServeEnd::Finished(s) => print_result(&s),
    }
}

pub(crate) fn print_result(session: &Session) -> Result<i32> {
    println!("{}", serde_json::to_string_pretty(&ReviewResult::from_session(session))?);
    Ok(match session.status {
        SessionStatus::Abandoned => exit::ABANDONED,
        _ => exit::OK,
    })
}
