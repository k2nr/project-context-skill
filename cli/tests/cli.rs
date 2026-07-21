use fs2::FileExt;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
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
    let reconstruction = root.join(".agents/skills/reconstruct-project-context");
    copy_tree(
        &repository.join("reconstruct-project-context"),
        &reconstruction,
    );
    fs::copy(repository.join("LICENSE"), reconstruction.join("LICENSE"))
        .expect("copy reconstruction license");
    fs::copy(
        destination.join("assets/install/AGENTS.fragment.md"),
        root.join("AGENTS.md"),
    )
    .expect("copy managed AGENTS block");
}

fn reconstruction_inventory(
    root: &Path,
    decision_coverage: &str,
    conversations: &[&str],
    tracked: &[&str],
) -> PathBuf {
    let inventory = root.join("inventory");
    fs::create_dir(&inventory).expect("create reconstruction inventory");
    fs::write(inventory.join("decision-coverage.jsonl"), decision_coverage)
        .expect("write decision coverage");
    fs::write(inventory.join("commits.jsonl"), "").expect("write commits inventory");
    fs::write(inventory.join("commit-coverage.jsonl"), "").expect("write commit coverage");
    let conversation_coverage = conversations
        .iter()
        .map(|source| serde_json::json!({"source": source, "status": "analyzed"}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        inventory.join("conversation-coverage.jsonl"),
        if conversation_coverage.is_empty() {
            conversation_coverage
        } else {
            conversation_coverage + "\n"
        },
    )
    .expect("write conversation coverage");
    fs::write(
        inventory.join("tracked-paths.json"),
        serde_json::to_string(tracked).expect("serialize tracked paths"),
    )
    .expect("write tracked paths");
    fs::write(
        inventory.join("coverage-sources.json"),
        serde_json::to_string(&serde_json::json!({
            "version": 1,
            "sources": {
                "commit-coverage.jsonl": [],
                "conversation-coverage.jsonl": conversations,
                "decision-coverage.jsonl": decision_coverage
                    .lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter_map(|record| record.get("source").and_then(Value::as_str).map(str::to_owned))
                    .collect::<Vec<_>>()
            }
        }))
        .expect("serialize coverage sources"),
    )
    .expect("write coverage sources");
    fs::write(
        inventory.join("summary.json"),
        serde_json::to_string(&serde_json::json!({
            "selected": {"non_ignored_untracked": false},
            "counts": {
                "commits": 0,
                "conversation_records": conversations.len(),
                "decision_signals": decision_coverage.lines().count(),
                "untracked_coverage": 0
            }
        }))
        .expect("serialize inventory summary"),
    )
    .expect("write inventory summary");
    inventory
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
fn migration_upgrades_v1_provenance_and_enables_custom_operations() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let context = directory.path().join(".project-context");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::copy(
        manifest.join("src/schemas/model-v1.json"),
        context.join("schemas/model.schema.json"),
    )
    .expect("copy v1 model schema");
    fs::copy(
        manifest.join("src/schemas/event-v1.json"),
        context.join("schemas/event.schema.json"),
    )
    .expect("copy v1 event schema");
    fs::write(
        context.join("model.yaml"),
        concat!(
            "schema_version: 1\n",
            "project:\n  id: legacy\n",
            "principles:\n",
            "  - id: durable\n    statement: Preserve rationale.\n    related_events: [D-1]\n",
            "architecture: []\nbehaviors: []\nconstraints: []\n",
            "operations:\n  build: [cargo build]\n  test: []\n  lint: []\n  format: []\n",
            "extensions: {}\n"
        ),
    )
    .expect("write v1 model");
    fs::write(
        context.join("events.jsonl"),
        "{\"schema_version\":1,\"type\":\"decision\",\"id\":\"D-1\",\"date\":\"2026-07-18\",\"subject\":\"legacy\",\"decision\":\"Preserve it.\",\"reason\":\"It matters.\",\"evidence\":[\"file:README.md\"],\"supersedes\":[]}\n",
    )
    .expect("write v1 event");

    assert!(cli(directory.path(), &["validate"]).status.success());
    let migrated = cli(directory.path(), &["migrate", "--format", "json"]);
    assert!(migrated.status.success(), "{}", stdout(&migrated));
    let report: Value = serde_json::from_slice(&migrated.stdout).expect("migration JSON");
    assert_eq!(report["model_migrated"], true);
    assert_eq!(report["events_migrated"], 1);
    let model: Value = serde_yaml_ng::from_str(
        &fs::read_to_string(context.join("model.yaml")).expect("migrated model"),
    )
    .expect("model YAML");
    assert_eq!(model["schema_version"], 2);
    assert_eq!(model["operations"]["build"][0]["command"], "cargo build");
    assert_eq!(
        model["principles"][0]["event_relations"][0],
        serde_json::json!({"event":"D-1","kind":"related"})
    );
    let event: Value = serde_json::from_str(
        fs::read_to_string(context.join("events.jsonl"))
            .expect("migrated events")
            .trim(),
    )
    .expect("event JSON");
    assert_eq!(
        event["evidence"][0],
        serde_json::json!({"ref":"file:README.md"})
    );

    let configured = cli(
        directory.path(),
        &["configure", "--operation", "deploy=cargo run -- deploy"],
    );
    assert!(configured.status.success(), "{}", stdout(&configured));
    assert!(
        cli(directory.path(), &["validate", "--strict"])
            .status
            .success()
    );
    let second = cli(directory.path(), &["migrate", "--format", "json"]);
    let second: Value = serde_json::from_slice(&second.stdout).expect("second migration JSON");
    assert_eq!(second["no_op"], true);
}

#[test]
fn typed_partial_supersession_requires_scope_and_preserves_the_store_atomically() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    assert!(
        cli(
            directory.path(),
            &[
                "add-decision",
                "--id",
                "D-1",
                "--subject",
                "baseline",
                "--decision",
                "Use the baseline.",
                "--reason",
                "It covers the default case."
            ]
        )
        .status
        .success()
    );
    let events = directory.path().join(".project-context/events.jsonl");
    let before = fs::read(&events).expect("events before invalid relation");
    let invalid = cli(
        directory.path(),
        &[
            "add-decision",
            "--subject",
            "scoped revision",
            "--decision",
            "Use a revision in one scope.",
            "--reason",
            "The scope differs.",
            "--relation",
            "{\"event\":\"D-1\",\"kind\":\"partially_supersedes\"}",
        ],
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(fs::read(&events).expect("events after rejection"), before);
    let valid = cli(
        directory.path(),
        &[
            "add-decision",
            "--subject",
            "scoped revision",
            "--decision",
            "Use a revision in one scope.",
            "--reason",
            "The scope differs.",
            "--relation",
            "{\"event\":\"D-1\",\"kind\":\"partially_supersedes\",\"scope\":\"remote execution only\"}",
        ],
    );
    assert!(valid.status.success(), "{}", stdout(&valid));
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
fn reconstruction_applies_atomically_and_is_idempotent() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let context = directory.path().join(".project-context");
    let base_model = directory.path().join("base-model.yaml");
    let base_events = directory.path().join("base-events.jsonl");
    let proposed_model = directory.path().join("proposed-model.yaml");
    let proposed_events = directory.path().join("proposed-events.jsonl");
    fs::copy(context.join("model.yaml"), &base_model).expect("copy base model");
    fs::copy(context.join("events.jsonl"), &base_events).expect("copy base events");
    let model = fs::read_to_string(&base_model)
        .expect("read base model")
        .replace(
            "architecture: []",
            concat!(
                "architecture:\n",
                "  - id: reconstructed-history\n",
                "    statement: Preserve reconstructed repository intent.\n",
                "    evidence:\n",
                "      - ref: file:src/history.rs\n",
                "        role: implementation\n",
                "    event_relations:\n",
                "      - event: candidate:decision\n",
                "        kind: origin"
            ),
        );
    fs::write(&proposed_model, model).expect("write proposed model");
    fs::write(
        &proposed_events,
        concat!(
            "{\"schema_version\":2,\"type\":\"decision\",\"id\":\"candidate:decision\",",
            "\"date\":\"2026-07-19\",\"subject\":\"history reconstruction\",",
            "\"decision\":\"Preserve repository-linked local history.\",",
            "\"reason\":\"It contains durable project intent.\",",
            "\"evidence\":[{\"ref\":\"conversation:codex:fixture#4\",\"role\":\"choice\"}]}\n"
        ),
    )
    .expect("write proposed events");
    let inventory = reconstruction_inventory(
        directory.path(),
        concat!(
            "{\"source\":\"conversation:codex:fixture#4\",\"status\":\"decision\",",
            "\"topic\":\"history reconstruction\",\"candidate\":\"candidate:decision\",",
            "\"rationale\":\"It contains durable project intent.\"}\n"
        ),
        &["conversation:codex:fixture#4"],
        &["src/history.rs"],
    );

    let arguments = [
        "apply-reconstruction",
        "--base-model",
        base_model.to_str().expect("base model path"),
        "--base-events",
        base_events.to_str().expect("base events path"),
        "--model",
        proposed_model.to_str().expect("proposed model path"),
        "--events",
        proposed_events.to_str().expect("proposed events path"),
        "--inventory",
        inventory.to_str().expect("inventory path"),
        "--format",
        "json",
    ];
    let check_arguments = [
        "check-reconstruction",
        "--base-model",
        base_model.to_str().expect("base model path"),
        "--base-events",
        base_events.to_str().expect("base events path"),
        "--model",
        proposed_model.to_str().expect("proposed model path"),
        "--events",
        proposed_events.to_str().expect("proposed events path"),
        "--inventory",
        inventory.to_str().expect("inventory path"),
        "--format",
        "json",
    ];
    let model_before_check = fs::read(context.join("model.yaml")).expect("model before check");
    let events_before_check = fs::read(context.join("events.jsonl")).expect("events before check");
    let checked = cli(directory.path(), &check_arguments);
    assert!(checked.status.success(), "{}", stdout(&checked));
    let check_report: Value = serde_json::from_slice(&checked.stdout).expect("check report JSON");
    assert_eq!(check_report["valid"], true);
    assert_eq!(
        fs::read(context.join("model.yaml")).unwrap(),
        model_before_check
    );
    assert_eq!(
        fs::read(context.join("events.jsonl")).unwrap(),
        events_before_check
    );
    let first = cli(directory.path(), &arguments);
    assert!(first.status.success(), "{}", stdout(&first));
    let report: Value = serde_json::from_slice(&first.stdout).expect("report JSON");
    assert_eq!(report["model_changed"], true);
    assert_eq!(report["events_added"], 1);
    assert_eq!(report["duplicates_skipped"], 0);
    assert_eq!(report["no_op"], false);
    assert_eq!(report["model_changed"], check_report["model_changed"]);
    assert_eq!(report["events_added"], check_report["events_added"]);
    assert_eq!(
        report["duplicates_skipped"],
        check_report["duplicates_skipped"]
    );
    let original_events = fs::read(&base_events).expect("original events");
    let applied_events = fs::read(context.join("events.jsonl")).expect("applied events");
    assert!(applied_events.starts_with(&original_events));

    fs::copy(context.join("model.yaml"), &base_model).expect("refresh base model");
    fs::copy(context.join("events.jsonl"), &base_events).expect("refresh base events");
    fs::copy(&base_model, &proposed_model).expect("reuse applied model");
    let second = cli(directory.path(), &arguments);
    assert!(second.status.success(), "{}", stdout(&second));
    let report: Value = serde_json::from_slice(&second.stdout).expect("report JSON");
    assert_eq!(report["events_added"], 0);
    assert_eq!(report["duplicates_skipped"], 1);
    assert_eq!(report["no_op"], true);
}

#[test]
fn reconstruction_requires_exact_model_signal_evidence() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let context = directory.path().join(".project-context");
    let base_model = directory.path().join("base-model.yaml");
    let base_events = directory.path().join("base-events.jsonl");
    let proposed_model = directory.path().join("proposed-model.yaml");
    let proposed_events = directory.path().join("proposed-events.jsonl");
    fs::copy(context.join("model.yaml"), &base_model).expect("copy base model");
    fs::copy(context.join("events.jsonl"), &base_events).expect("copy base events");
    let source = "conversation:codex:model-fixture#7";
    let model = fs::read_to_string(&base_model)
        .expect("read base model")
        .replace(
            "architecture: []",
            concat!(
                "architecture:\n",
                "  - id: model-signal\n",
                "    statement: Preserve model-only intent.\n",
                "    evidence:\n",
                "      - ref: conversation:codex:model-fixture#7\n",
                "        role: choice"
            ),
        );
    fs::write(&proposed_model, model).expect("write proposed model");
    fs::write(&proposed_events, "").expect("write proposed events");
    let inventory = reconstruction_inventory(
        directory.path(),
        concat!(
            "{\"source\":\"conversation:codex:model-fixture#7\",\"status\":\"model\",",
            "\"topic\":\"model-only intent\",\"candidate\":\"architecture:model-signal\"}\n"
        ),
        &[source],
        &[],
    );
    let arguments = [
        "check-reconstruction",
        "--base-model",
        base_model.to_str().expect("base model path"),
        "--base-events",
        base_events.to_str().expect("base events path"),
        "--model",
        proposed_model.to_str().expect("proposed model path"),
        "--events",
        proposed_events.to_str().expect("proposed events path"),
        "--inventory",
        inventory.to_str().expect("inventory path"),
        "--format",
        "json",
    ];
    let valid = cli(directory.path(), &arguments);
    assert!(valid.status.success(), "{}", stdout(&valid));

    let invalid_model = fs::read_to_string(&proposed_model)
        .expect("read proposed model")
        .replace(source, "conversation:codex:model-fixture#8");
    fs::write(&proposed_model, invalid_model).expect("write invalid proposed model");
    let invalid = cli(directory.path(), &arguments);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(stdout(&invalid).contains("absent from 'architecture:model-signal' evidence"));
}

#[test]
fn reconstruction_writes_a_stable_event_timeline() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let context = directory.path().join(".project-context");
    fs::write(
        context.join("events.jsonl"),
        concat!(
            "{\"schema_version\":2,\"type\":\"decision\",\"id\":\"D-1\",\"date\":\"2026-07-20\",\"subject\":\"late existing\",\"decision\":\"late\",\"reason\":\"late\"}\n",
            "{\"schema_version\":2,\"type\":\"decision\",\"id\":\"D-2\",\"date\":\"2026-07-18\",\"subject\":\"early existing\",\"decision\":\"early\",\"reason\":\"early\"}\n",
        ),
    )
    .expect("write base events");
    let base_model = directory.path().join("base-model.yaml");
    let base_events = directory.path().join("base-events.jsonl");
    let proposed_model = directory.path().join("proposed-model.yaml");
    let proposed_events = directory.path().join("proposed-events.jsonl");
    fs::copy(context.join("model.yaml"), &base_model).expect("copy base model");
    fs::copy(context.join("events.jsonl"), &base_events).expect("copy base events");
    fs::copy(&base_model, &proposed_model).expect("copy proposed model");
    fs::write(
        &proposed_events,
        concat!(
            "{\"schema_version\":2,\"type\":\"attempt\",\"id\":\"candidate:middle-attempt\",\"date\":\"2026-07-19\",\"occurred_at\":\"2026-07-19T10:00:00Z\",\"subject\":\"middle attempt\",\"approach\":\"try\",\"result\":\"failed\",\"finding\":\"finding\",\"evidence\":[{\"ref\":\"conversation:codex:fixture#2\",\"role\":\"outcome\",\"observed_at\":\"2026-07-19T10:00:00Z\"}]}\n",
            "{\"schema_version\":2,\"type\":\"decision\",\"id\":\"candidate:middle-decision\",\"date\":\"2026-07-19\",\"occurred_at\":\"2026-07-19T09:00:00Z\",\"subject\":\"middle decision\",\"decision\":\"middle\",\"reason\":\"middle\",\"evidence\":[{\"ref\":\"conversation:codex:fixture#1\",\"role\":\"choice\",\"observed_at\":\"2026-07-19T09:00:00Z\"}]}\n",
        ),
    )
    .expect("write proposed events");
    let inventory = reconstruction_inventory(
        directory.path(),
        "",
        &[
            "conversation:codex:fixture#1",
            "conversation:codex:fixture#2",
        ],
        &[],
    );

    let output = cli(
        directory.path(),
        &[
            "apply-reconstruction",
            "--base-model",
            base_model.to_str().expect("base model path"),
            "--base-events",
            base_events.to_str().expect("base events path"),
            "--model",
            proposed_model.to_str().expect("proposed model path"),
            "--events",
            proposed_events.to_str().expect("proposed events path"),
            "--inventory",
            inventory.to_str().expect("inventory path"),
            "--format",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", stdout(&output));
    let report: Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    assert_eq!(report["events_added"], 2);
    assert_eq!(report["no_op"], false);
    let stored = fs::read_to_string(context.join("events.jsonl")).expect("read events");
    let subjects: Vec<String> = stored
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("event JSON")["subject"]
                .as_str()
                .expect("subject")
                .to_owned()
        })
        .collect();
    assert_eq!(
        subjects,
        [
            "early existing",
            "middle decision",
            "middle attempt",
            "late existing"
        ]
    );
}

#[test]
fn reconstruction_reports_base_conflicts_without_mutation() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let context = directory.path().join(".project-context");
    let base_model = directory.path().join("base-model.yaml");
    let base_events = directory.path().join("base-events.jsonl");
    let proposed_model = directory.path().join("proposed-model.yaml");
    let proposed_events = directory.path().join("proposed-events.jsonl");
    fs::copy(context.join("model.yaml"), &base_model).expect("copy base model");
    fs::copy(context.join("events.jsonl"), &base_events).expect("copy base events");
    fs::copy(&base_model, &proposed_model).expect("copy proposed model");
    fs::write(&proposed_events, "").expect("empty proposed events");
    let inventory = reconstruction_inventory(directory.path(), "", &[], &[]);
    assert!(
        cli(
            directory.path(),
            &[
                "add-attempt",
                "--subject",
                "concurrent mutation",
                "--approach",
                "Change the canonical event log.",
                "--result",
                "succeeded",
                "--finding",
                "The base snapshot is now stale.",
            ],
        )
        .status
        .success()
    );
    let before_model = fs::read(context.join("model.yaml")).expect("model before conflict");
    let before_events = fs::read(context.join("events.jsonl")).expect("events before conflict");

    let output = cli(
        directory.path(),
        &[
            "apply-reconstruction",
            "--base-model",
            base_model.to_str().expect("base model path"),
            "--base-events",
            base_events.to_str().expect("base events path"),
            "--model",
            proposed_model.to_str().expect("proposed model path"),
            "--events",
            proposed_events.to_str().expect("proposed events path"),
            "--inventory",
            inventory.to_str().expect("inventory path"),
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(fs::read(context.join("model.yaml")).unwrap(), before_model);
    assert_eq!(
        fs::read(context.join("events.jsonl")).unwrap(),
        before_events
    );
}

#[test]
fn reconstruction_reports_invalid_proposed_data_as_exit_one() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(cli(directory.path(), &["init"]).status.success());
    let context = directory.path().join(".project-context");
    let base_model = directory.path().join("base-model.yaml");
    let base_events = directory.path().join("base-events.jsonl");
    let proposed_model = directory.path().join("proposed-model.yaml");
    let proposed_events = directory.path().join("proposed-events.jsonl");
    fs::copy(context.join("model.yaml"), &base_model).expect("copy base model");
    fs::copy(context.join("events.jsonl"), &base_events).expect("copy base events");
    fs::write(&proposed_model, "project: [invalid\n").expect("write invalid model");
    fs::write(&proposed_events, "").expect("empty proposed events");
    let inventory = reconstruction_inventory(directory.path(), "", &[], &[]);

    let output = cli(
        directory.path(),
        &[
            "apply-reconstruction",
            "--base-model",
            base_model.to_str().expect("base model path"),
            "--base-events",
            base_events.to_str().expect("base events path"),
            "--model",
            proposed_model.to_str().expect("proposed model path"),
            "--events",
            proposed_events.to_str().expect("proposed events path"),
            "--inventory",
            inventory.to_str().expect("inventory path"),
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).expect("validation JSON");
    assert_eq!(report["valid"], false);
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
            "--occurred-at",
            "2026-07-17T10:30:00+09:00",
            "--evidence-detail",
            "{\"ref\":\"conversation:codex:fixture#9\",\"role\":\"rationale\",\"observed_at\":\"2026-07-17T10:29:00+09:00\"}",
            "--format",
            "json",
        ],
    );
    assert!(second.status.success(), "{}", stdout(&second));
    let event: Value = serde_json::from_slice(&second.stdout).expect("decision JSON");
    assert_eq!(event["id"], "D-42");
    assert_eq!(event["occurred_at"], "2026-07-17T10:30:00+09:00");
    assert_eq!(
        event["relations"],
        serde_json::json!([{"event":"D-41","kind":"supersedes"}])
    );
    assert_eq!(
        event["rejected"],
        serde_json::json!(["Keep the old boundary.", "Duplicate ownership."])
    );
    assert_eq!(event["conditions"], "While the frontend owns the session.");
    assert_eq!(
        event["evidence"],
        serde_json::json!([
            {"ref":"file:src/session.rs"},
            {"ref":"conversation:codex:fixture#9","role":"rationale","observed_at":"2026-07-17T10:29:00+09:00"}
        ])
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
