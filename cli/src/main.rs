mod context;
mod output;
mod store;

use clap::{Parser, Subcommand, ValueEnum};
use output::{OutputFormat, render_context, render_event, render_init, render_validation};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use store::{AttemptInput, DecisionInput, StoreError};

#[derive(Parser)]
#[command(name = "project-context", version, about)]
struct Cli {
    #[arg(long, value_enum, default_value_t = OutputFormat::Text, global = true)]
    format: OutputFormat,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the minimal project-context store in the current directory.
    Init {
        /// Replace existing project-context files.
        #[arg(long)]
        force: bool,
    },
    /// Return relevant durable context for paths, symbols, phrases, or topics.
    Context {
        /// Paths, symbols, error phrases, or topics to retrieve.
        #[arg(required = true, num_args = 1..)]
        queries: Vec<String>,
        /// Approximate maximum output size in tokens.
        #[arg(long, default_value_t = 4000)]
        max_tokens: usize,
    },
    /// Append a durable architectural or product decision.
    AddDecision {
        #[arg(long)]
        subject: String,
        #[arg(long)]
        decision: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        rejected: Vec<String>,
        #[arg(long)]
        supersedes: Vec<String>,
        #[arg(long)]
        conditions: Option<String>,
        #[arg(long)]
        evidence: Vec<String>,
    },
    /// Append a costly or non-obvious experimental result.
    AddAttempt {
        #[arg(long)]
        subject: String,
        #[arg(long)]
        approach: String,
        #[arg(long, value_enum)]
        result: AttemptResult,
        #[arg(long)]
        finding: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        conditions: Option<String>,
        #[arg(long)]
        evidence: Vec<String>,
    },
    /// Validate the nearest project-context store.
    Validate {
        /// Treat warnings as validation errors.
        #[arg(long)]
        strict: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AttemptResult {
    Succeeded,
    Failed,
    Partial,
    Inconclusive,
}

impl AttemptResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Partial => "partial",
            Self::Inconclusive => "inconclusive",
        }
    }
}

enum AppError {
    Invalid(store::ValidationReport),
    Environment(String),
}

impl From<StoreError> for AppError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Invalid(report) => Self::Invalid(report),
            StoreError::Environment(message) => Self::Environment(message),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(output) => write_stdout(&output, 0),
        Err(AppError::Invalid(report)) => write_stdout(&render_validation(&report, cli.format), 1),
        Err(AppError::Environment(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<String, AppError> {
    match &cli.command {
        Command::Init { force } => {
            let root = current_directory()?;
            let report = store::initialize(&root, *force).map_err(AppError::Environment)?;
            Ok(render_init(&report, cli.format))
        }
        Command::Context {
            queries,
            max_tokens,
        } => {
            if *max_tokens == 0 {
                return Err(AppError::Environment(
                    "--max-tokens must be greater than zero".to_owned(),
                ));
            }
            if queries.len() > 32 {
                return Err(AppError::Environment(
                    "context accepts at most 32 query arguments".to_owned(),
                ));
            }
            if let Some(query) = queries
                .iter()
                .find(|query| query.trim().is_empty() || query.len() > 1024)
            {
                let reason = if query.trim().is_empty() {
                    "must not be empty"
                } else {
                    "must not exceed 1024 UTF-8 bytes"
                };
                return Err(AppError::Environment(format!("context query {reason}")));
            }
            let root = nearest_root()?;
            let data = store::load_valid_repository(&root)?;
            let packet = context::build_context(&root, &data, queries, *max_tokens);
            render_context(&packet, cli.format, *max_tokens).map_err(AppError::Environment)
        }
        Command::AddDecision {
            subject,
            decision,
            reason,
            id,
            date,
            rejected,
            supersedes,
            conditions,
            evidence,
        } => {
            let root = nearest_root()?;
            let event = store::add_decision(
                &root,
                DecisionInput {
                    subject: subject.clone(),
                    decision: decision.clone(),
                    reason: reason.clone(),
                    id: id.clone(),
                    date: date.clone(),
                    rejected: rejected.clone(),
                    supersedes: supersedes.clone(),
                    conditions: conditions.clone(),
                    evidence: evidence.clone(),
                },
            )?;
            Ok(render_event(&event, cli.format))
        }
        Command::AddAttempt {
            subject,
            approach,
            result,
            finding,
            id,
            date,
            conditions,
            evidence,
        } => {
            let root = nearest_root()?;
            let event = store::add_attempt(
                &root,
                AttemptInput {
                    subject: subject.clone(),
                    approach: approach.clone(),
                    result: result.as_str().to_owned(),
                    finding: finding.clone(),
                    id: id.clone(),
                    date: date.clone(),
                    conditions: conditions.clone(),
                    evidence: evidence.clone(),
                },
            )?;
            Ok(render_event(&event, cli.format))
        }
        Command::Validate { strict } => {
            let root = nearest_root()?;
            let mut report = store::validate_repository(&root)?;
            if *strict && !report.warnings.is_empty() {
                report.errors.extend(
                    report
                        .warnings
                        .iter()
                        .map(|warning| format!("strict warning: {warning}")),
                );
            }
            report.normalize();
            if report.valid {
                Ok(render_validation(&report, cli.format))
            } else {
                Err(AppError::Invalid(report))
            }
        }
    }
}

fn write_stdout(output: &str, successful_write_code: u8) -> ExitCode {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    match writeln!(handle, "{output}") {
        Ok(()) => ExitCode::from(successful_write_code),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: cannot write output: {error}");
            ExitCode::from(2)
        }
    }
}

fn current_directory() -> Result<std::path::PathBuf, AppError> {
    std::env::current_dir().map_err(|error| AppError::Environment(error.to_string()))
}

fn nearest_root() -> Result<std::path::PathBuf, AppError> {
    let start = current_directory()?;
    store::discover_root(Path::new(&start)).ok_or_else(|| {
        AppError::Environment(format!(
            "no .project-context directory found from {} or its ancestors",
            start.display()
        ))
    })
}
