use fs2::FileExt;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn cli(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_project-context"))
        .current_dir(root)
        .args(arguments)
        .output()
        .expect("run project-context")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create destination directory");
    for entry in fs::read_dir(source).expect("read source directory") {
        let entry = entry.expect("source entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("entry type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy source file");
        }
    }
}

fn install_skill_fixture(root: &Path) {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root");
    let destination = root.join(".agents/skills/project-context");
    copy_tree(&repository.join("project-context"), &destination);
    fs::copy(repository.join("LICENSE"), destination.join("LICENSE")).expect("copy license");
    fs::copy(
        destination.join("assets/install/AGENTS.fragment.md"),
        root.join("AGENTS.md"),
    )
    .expect("copy managed AGENTS block");
}

#[test]
fn all_phase_four_commands_work_end_to_end() {
    let directory = TempDir::new().expect("temporary directory");

    let init = cli(directory.path(), &["init", "--format", "json"]);
    assert!(init.status.success(), "{}", stdout(&init));
    let init_json: Value = serde_json::from_slice(&init.stdout).expect("init JSON");
    let initialized_files = init_json["files"].as_array().expect("files");
    assert_eq!(initialized_files.len(), 4);
    assert!(initialized_files.iter().all(|path| {
        path.as_str()
            .is_some_and(|path| path.starts_with(".project-context/"))
    }));

    let decision = cli(
        directory.path(),
        &[
            "add-decision",
            "--subject",
            "candidate ownership",
            "--decision",
            "Generate candidates in the frontend.",
            "--reason",
            "The frontend owns session state.",
            "--rejected",
            "Generate candidates in the backend.",
            "--evidence",
            "file:src/candidates.rs",
            "--date",
            "2026-07-17",
            "--format",
            "json",
        ],
    );
    assert!(decision.status.success(), "{}", stdout(&decision));
    let decision_json: Value = serde_json::from_slice(&decision.stdout).expect("decision JSON");
    assert_eq!(decision_json["id"], "D-0001");

    let attempt = cli(
        directory.path(),
        &[
            "add-attempt",
            "--subject",
            "backend candidate generation",
            "--approach",
            "Move generation into the backend.",
            "--result",
            "failed",
            "--finding",
            "Session state was duplicated.",
            "--date",
            "2026-07-17",
            "--format",
            "yaml",
        ],
    );
    assert!(attempt.status.success(), "{}", stdout(&attempt));
    assert!(stdout(&attempt).contains("id: A-0001"));

    let context = cli(
        directory.path(),
        &["context", "candidate ownership", "--format", "json"],
    );
    assert!(context.status.success(), "{}", stdout(&context));
    let context_json: Value = serde_json::from_slice(&context.stdout).expect("context JSON");
    assert_eq!(context_json["history"]["decisions"][0]["id"], "D-0001");

    let validate = cli(directory.path(), &["validate", "--format", "text"]);
    assert!(validate.status.success(), "{}", stdout(&validate));
    assert_eq!(stdout(&validate), "valid\n");
}

#[test]
fn configure_updates_only_explicit_model_fields_and_preserves_history() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    assert!(
        cli(
            directory.path(),
            &[
                "add-attempt",
                "--subject",
                "preserved history",
                "--approach",
                "Record before configuration.",
                "--result",
                "succeeded",
                "--finding",
                "Configuration preserves the event log.",
            ],
        )
        .status
        .success()
    );
    let events_path = directory.path().join(".project-context/events.jsonl");
    let events_before = fs::read(&events_path).expect("events before configuration");

    let configured = cli(
        directory.path(),
        &[
            "configure",
            "--project-id",
            "configured-project",
            "--description",
            "Configured project fixture.",
            "--build",
            "cargo build",
            "--test",
            "cargo test",
            "--format",
            "json",
        ],
    );
    assert!(configured.status.success(), "{}", stdout(&configured));
    let report: Value = serde_json::from_slice(&configured.stdout).expect("configure JSON");
    assert_eq!(report["updated"].as_array().expect("updated").len(), 4);
    assert_eq!(
        fs::read(events_path).expect("events after configuration"),
        events_before
    );
    let model = fs::read_to_string(directory.path().join(".project-context/model.yaml"))
        .expect("configured model");
    assert!(model.contains("id: configured-project"));
    assert!(model.contains("description: Configured project fixture."));
    assert!(model.contains("cargo build"));
    assert!(model.contains("cargo test"));
    assert!(
        cli(directory.path(), &["validate", "--strict"])
            .status
            .success()
    );
}

#[test]
fn installation_doctor_requires_complete_model_or_explicit_empty_acknowledgement() {
    let directory = TempDir::new().expect("temporary directory");
    install_skill_fixture(directory.path());
    assert!(cli(directory.path(), &["init"]).status.success());
    assert!(
        cli(
            directory.path(),
            &[
                "configure",
                "--description",
                "Doctor fixture.",
                "--build",
                "cargo build",
            ],
        )
        .status
        .success()
    );

    let incomplete = cli(
        directory.path(),
        &["doctor", "--installation", "--format", "json"],
    );
    assert_eq!(incomplete.status.code(), Some(1));
    let incomplete_report: Value =
        serde_json::from_slice(&incomplete.stdout).expect("incomplete doctor JSON");
    assert_eq!(incomplete_report["ready"], false);
    assert!(
        incomplete_report["errors"]
            .as_array()
            .expect("doctor errors")
            .iter()
            .any(|error| error
                .as_str()
                .is_some_and(|error| error.contains("operations.test")))
    );

    let ready = cli(
        directory.path(),
        &[
            "doctor",
            "--installation",
            "--allow-empty",
            "test",
            "--allow-empty",
            "lint",
            "--allow-empty",
            "format",
            "--format",
            "json",
        ],
    );
    assert!(ready.status.success(), "{}", stdout(&ready));
    let ready_report: Value = serde_json::from_slice(&ready.stdout).expect("ready doctor JSON");
    assert_eq!(ready_report["ready"], true);
    assert_eq!(ready_report["checks"]["skill.package"], "valid");
    assert_eq!(ready_report["checks"]["agents.managed_block"], "current");
}

#[test]
fn strict_validation_rejects_unverifiable_commit_evidence() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    assert!(
        cli(
            directory.path(),
            &[
                "add-decision",
                "--subject",
                "evidence validation",
                "--decision",
                "Record the commit.",
                "--reason",
                "The change is durable.",
                "--evidence",
                "commit:0123456789abcdef0123456789abcdef01234567",
            ],
        )
        .status
        .success()
    );

    let ordinary = cli(directory.path(), &["validate", "--format", "json"]);
    assert!(ordinary.status.success(), "{}", stdout(&ordinary));
    let report: Value = serde_json::from_slice(&ordinary.stdout).expect("validation JSON");
    assert_eq!(report["valid"], true);
    assert!(!report["warnings"].as_array().expect("warnings").is_empty());

    let strict = cli(
        directory.path(),
        &["validate", "--strict", "--format", "json"],
    );
    assert_eq!(strict.status.code(), Some(1));
}

#[test]
fn command_exit_codes_distinguish_invalid_data_and_environment_errors() {
    let missing = TempDir::new().expect("temporary directory");
    let no_project = cli(missing.path(), &["validate"]);
    assert_eq!(no_project.status.code(), Some(2));

    let invalid = TempDir::new().expect("temporary directory");
    assert!(cli(invalid.path(), &["init"]).status.success());
    fs::write(
        invalid.path().join(".project-context/events.jsonl"),
        "invalid\n",
    )
    .expect("write invalid event log");
    let invalid_data = cli(invalid.path(), &["validate", "--format", "json"]);
    assert_eq!(invalid_data.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&invalid_data.stdout).expect("validation JSON");
    assert_eq!(report["valid"], false);

    let incomplete = TempDir::new().expect("temporary directory");
    assert!(cli(incomplete.path(), &["init"]).status.success());
    fs::remove_file(incomplete.path().join(".project-context/model.yaml")).expect("remove model");
    let missing_canonical = cli(incomplete.path(), &["validate", "--format", "json"]);
    assert_eq!(missing_canonical.status.code(), Some(1));
}

#[test]
fn max_tokens_must_be_positive() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let output = cli(
        directory.path(),
        &["context", "candidate", "--max-tokens", "0"],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn decision_flags_are_persisted_and_invalid_supersession_is_atomic() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let first = cli(
        directory.path(),
        &[
            "add-decision",
            "--id",
            "D-41",
            "--date",
            "2026-07-16",
            "--subject",
            "old boundary",
            "--decision",
            "Use the old boundary.",
            "--reason",
            "It was available.",
        ],
    );
    assert!(first.status.success(), "{}", stdout(&first));
    let second = cli(
        directory.path(),
        &[
            "add-decision",
            "--id",
            "D-42",
            "--date",
            "2026-07-17",
            "--subject",
            "current boundary",
            "--decision",
            "Use the current boundary.",
            "--reason",
            "It preserves ownership.",
            "--rejected",
            "Keep the old boundary.",
            "--rejected",
            "Duplicate ownership.",
            "--supersedes",
            "D-41",
            "--conditions",
            "While the frontend owns the session.",
            "--evidence",
            "file:src/session.rs",
            "--format",
            "json",
        ],
    );
    assert!(second.status.success(), "{}", stdout(&second));
    let event: Value = serde_json::from_slice(&second.stdout).expect("decision JSON");
    assert_eq!(event["id"], "D-42");
    assert_eq!(event["supersedes"], serde_json::json!(["D-41"]));
    assert_eq!(
        event["rejected"],
        serde_json::json!(["Keep the old boundary.", "Duplicate ownership."])
    );
    assert_eq!(event["conditions"], "While the frontend owns the session.");
    assert_eq!(
        event["evidence"],
        serde_json::json!(["file:src/session.rs"])
    );

    let path = directory.path().join(".project-context/events.jsonl");
    let before = fs::read(&path).expect("events before rejected append");
    let invalid = cli(
        directory.path(),
        &[
            "add-decision",
            "--subject",
            "invalid supersession",
            "--decision",
            "Do not persist this.",
            "--reason",
            "The reference is missing.",
            "--supersedes",
            "D-404",
        ],
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(
        fs::read(path).expect("events after rejected append"),
        before
    );
}

#[test]
fn context_enforces_query_count_and_utf8_byte_limits() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let arguments: Vec<String> = std::iter::once("context".to_owned())
        .chain((0..33).map(|index| format!("q{index}")))
        .collect();
    let references: Vec<&str> = arguments.iter().map(String::as_str).collect();
    assert_eq!(cli(directory.path(), &references).status.code(), Some(2));

    let oversized = "日".repeat(342);
    assert!(oversized.len() > 1024);
    assert_eq!(
        cli(directory.path(), &["context", &oversized])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn a_separate_process_cannot_mutate_while_the_store_is_locked() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let event_path = directory.path().join(".project-context/events.jsonl");
    let before = fs::read(&event_path).expect("events before contention");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.path().join(".project-context/.lock"))
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold exclusive lock");

    let contender = cli(
        directory.path(),
        &[
            "add-attempt",
            "--subject",
            "contended mutation",
            "--approach",
            "Write while locked.",
            "--result",
            "failed",
            "--finding",
            "The lock rejected it.",
        ],
    );
    assert_eq!(contender.status.code(), Some(2));
    assert_eq!(
        fs::read(event_path).expect("events after contention"),
        before
    );
}

#[test]
fn broken_stdout_after_a_mutation_is_not_reported_as_a_failed_commit() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let mut child = Command::new(env!("CARGO_BIN_EXE_project-context"))
        .current_dir(directory.path())
        .args([
            "add-attempt",
            "--subject",
            "broken output",
            "--approach",
            "Write after the reader closes.",
            "--result",
            "failed",
            "--finding",
            "The event still committed.",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn mutation");
    drop(child.stdout.take());
    assert!(child.wait().expect("wait for mutation").success());
    let events = fs::read_to_string(directory.path().join(".project-context/events.jsonl"))
        .expect("committed events");
    assert!(events.contains("broken output"));
}
