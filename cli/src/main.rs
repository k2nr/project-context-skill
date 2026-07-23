mod context;
mod doctor;
mod output;
mod store;

use clap::{Parser, Subcommand, ValueEnum};
use output::{
    OutputFormat, render_configure, render_conflict, render_context, render_doctor, render_event,
    render_init, render_migrate, render_reconstruction, render_reconstruction_check,
    render_validation,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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
    /// Update explicitly supplied project identity and operation commands.
    Configure {
        #[arg(long)]
        project_id: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        build: Vec<String>,
        #[arg(long)]
        test: Vec<String>,
        #[arg(long)]
        lint: Vec<String>,
        #[arg(long)]
        format_command: Vec<String>,
        /// Set an extensible operation as NAME=COMMAND. Repeat for multiple steps.
        #[arg(long)]
        operation: Vec<String>,
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
        occurred_at: Option<String>,
        #[arg(long)]
        rejected: Vec<String>,
        #[arg(long)]
        supersedes: Vec<String>,
        #[arg(long)]
        conditions: Option<String>,
        #[arg(long)]
        evidence: Vec<String>,
        /// Add structured evidence as a JSON object.
        #[arg(long)]
        evidence_detail: Vec<String>,
        /// Add a typed event relation as a JSON object.
        #[arg(long)]
        relation: Vec<String>,
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
        occurred_at: Option<String>,
        #[arg(long)]
        conditions: Option<String>,
        #[arg(long)]
        evidence: Vec<String>,
        #[arg(long)]
        evidence_detail: Vec<String>,
        #[arg(long)]
        relation: Vec<String>,
    },
    /// Atomically migrate a legacy v1 or v2 store to schema v3.
    Migrate,
    /// Atomically apply a validated reconstruction of model and event history.
    ApplyReconstruction {
        #[arg(long)]
        base_model: PathBuf,
        #[arg(long)]
        base_events: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        events: PathBuf,
        #[arg(long)]
        inventory: PathBuf,
    },
    /// Validate a reconstruction without changing the canonical store.
    CheckReconstruction {
        #[arg(long)]
        base_model: PathBuf,
        #[arg(long)]
        base_events: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        events: PathBuf,
        #[arg(long)]
        inventory: PathBuf,
    },
    /// Validate the nearest project-context store.
    Validate {
        /// Treat warnings as validation errors.
        #[arg(long)]
        strict: bool,
    },
    /// Diagnose whether a repository-local installation is complete and usable.
    Doctor {
        /// Check the installed skill, managed AGENTS block, and populated model.
        #[arg(long)]
        installation: bool,
        /// Acknowledge an operation category that intentionally has no commands.
        #[arg(long)]
        allow_empty: Vec<String>,
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
    Doctor(doctor::DoctorReport),
    Conflict(String),
    Environment(String),
}

impl From<StoreError> for AppError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Invalid(report) => Self::Invalid(report),
            StoreError::Conflict(message) => Self::Conflict(message),
            StoreError::Environment(message) => Self::Environment(message),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(output) => write_stdout(&output, 0),
        Err(AppError::Invalid(report)) => write_stdout(&render_validation(&report, cli.format), 1),
        Err(AppError::Doctor(report)) => write_stdout(&render_doctor(&report, cli.format), 1),
        Err(AppError::Conflict(message)) => write_stdout(&render_conflict(&message, cli.format), 3),
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
        Command::Configure {
            project_id,
            description,
            build,
            test,
            lint,
            format_command,
            operation,
        } => {
            validate_optional_value("--project-id", project_id.as_deref())?;
            validate_optional_value("--description", description.as_deref())?;
            validate_commands("--build", build)?;
            validate_commands("--test", test)?;
            validate_commands("--lint", lint)?;
            validate_commands("--format-command", format_command)?;
            let operations = parse_operations(operation)?;
            let root = nearest_root()?;
            let report = store::configure(
                &root,
                store::ConfigureInput {
                    project_id: project_id.clone(),
                    description: description.clone(),
                    build: build.clone(),
                    test: test.clone(),
                    lint: lint.clone(),
                    format: format_command.clone(),
                    operations,
                },
            )?;
            Ok(render_configure(&report, cli.format))
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
            occurred_at,
            rejected,
            supersedes,
            conditions,
            evidence,
            evidence_detail,
            relation,
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
                    occurred_at: occurred_at.clone(),
                    rejected: rejected.clone(),
                    supersedes: supersedes.clone(),
                    conditions: conditions.clone(),
                    evidence: evidence.clone(),
                    evidence_details: parse_json_objects("--evidence-detail", evidence_detail)?,
                    relations: parse_json_objects("--relation", relation)?,
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
            occurred_at,
            conditions,
            evidence,
            evidence_detail,
            relation,
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
                    occurred_at: occurred_at.clone(),
                    conditions: conditions.clone(),
                    evidence: evidence.clone(),
                    evidence_details: parse_json_objects("--evidence-detail", evidence_detail)?,
                    relations: parse_json_objects("--relation", relation)?,
                },
            )?;
            Ok(render_event(&event, cli.format))
        }
        Command::Migrate => {
            let root = nearest_root()?;
            let report = store::migrate(&root)?;
            Ok(render_migrate(&report, cli.format))
        }
        Command::ApplyReconstruction {
            base_model,
            base_events,
            model,
            events,
            inventory,
        } => {
            let root = nearest_root()?;
            let report = store::apply_reconstruction(
                &root,
                store::ReconstructionInput {
                    base_model: base_model.clone(),
                    base_events: base_events.clone(),
                    model: model.clone(),
                    events: events.clone(),
                    inventory: inventory.clone(),
                },
            )?;
            Ok(render_reconstruction(&report, cli.format))
        }
        Command::CheckReconstruction {
            base_model,
            base_events,
            model,
            events,
            inventory,
        } => {
            let root = nearest_root()?;
            let report = store::check_reconstruction(
                &root,
                store::ReconstructionInput {
                    base_model: base_model.clone(),
                    base_events: base_events.clone(),
                    model: model.clone(),
                    events: events.clone(),
                    inventory: inventory.clone(),
                },
            )?;
            Ok(render_reconstruction_check(&report, cli.format))
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
        Command::Doctor {
            installation,
            allow_empty,
        } => {
            if !installation {
                return Err(AppError::Environment(
                    "doctor currently requires --installation".to_owned(),
                ));
            }
            let root = nearest_root()?;
            for operation in allow_empty {
                validate_operation_name(operation)?;
            }
            let allowed = allow_empty.iter().map(String::as_str).collect();
            let report =
                doctor::inspect_installation(&root, &allowed).map_err(AppError::Environment)?;
            if report.ready {
                Ok(render_doctor(&report, cli.format))
            } else {
                Err(AppError::Doctor(report))
            }
        }
    }
}

fn parse_operations(values: &[String]) -> Result<BTreeMap<String, Vec<String>>, AppError> {
    let mut operations = BTreeMap::new();
    for value in values {
        let (name, command) = value
            .split_once('=')
            .ok_or_else(|| AppError::Environment("--operation must use NAME=COMMAND".to_owned()))?;
        validate_operation_name(name)?;
        if command.trim().is_empty() {
            return Err(AppError::Environment(
                "--operation command must not be empty".to_owned(),
            ));
        }
        operations
            .entry(name.to_owned())
            .or_insert_with(Vec::new)
            .push(command.to_owned());
    }
    Ok(operations)
}

fn validate_operation_name(name: &str) -> Result<(), AppError> {
    let valid = !name.is_empty()
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        });
    if !valid {
        return Err(AppError::Environment(format!(
            "operation name '{name}' must match ^[a-z][a-z0-9._-]*$"
        )));
    }
    Ok(())
}

fn parse_json_objects(label: &str, values: &[String]) -> Result<Vec<Value>, AppError> {
    values
        .iter()
        .map(|raw| {
            let value: Value = serde_json::from_str(raw).map_err(|error| {
                AppError::Environment(format!("{label} must be valid JSON: {error}"))
            })?;
            if !value.is_object() {
                return Err(AppError::Environment(format!(
                    "{label} must be a JSON object"
                )));
            }
            Ok(value)
        })
        .collect()
}

fn validate_optional_value(label: &str, value: Option<&str>) -> Result<(), AppError> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        return Err(AppError::Environment(format!("{label} must not be empty")));
    }
    Ok(())
}

fn validate_commands(label: &str, commands: &[String]) -> Result<(), AppError> {
    if commands.iter().any(|command| command.trim().is_empty()) {
        return Err(AppError::Environment(format!(
            "{label} command must not be empty"
        )));
    }
    Ok(())
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
