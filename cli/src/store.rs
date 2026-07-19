use fs2::FileExt;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use time::{
    Date, OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description,
};

const MODEL_TEMPLATE: &str = include_str!("../../project-context/assets/init/model.yaml");
const MODEL_SCHEMA: &str = include_str!("../../project-context/assets/init/model.schema.json");
const EVENT_SCHEMA: &str = include_str!("../../project-context/assets/init/event.schema.json");
const MODEL_SCHEMA_V1: &str = include_str!("schemas/model-v1.json");
const EVENT_SCHEMA_V1: &str = include_str!("schemas/event-v1.json");

const INIT_PATHS: [&str; 4] = [
    ".project-context/model.yaml",
    ".project-context/events.jsonl",
    ".project-context/schemas/model.schema.json",
    ".project-context/schemas/event.schema.json",
];
const TRANSACTION_DIRECTORY: &str = ".project-context/.init-transaction";

#[derive(Debug, Serialize)]
pub struct InitReport {
    pub initialized: bool,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ConfigureReport {
    pub updated: Vec<String>,
}

pub struct ConfigureInput {
    pub project_id: Option<String>,
    pub description: Option<String>,
    pub build: Vec<String>,
    pub test: Vec<String>,
    pub lint: Vec<String>,
    pub format: Vec<String>,
    pub operations: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct MigrateReport {
    pub model_migrated: bool,
    pub events_migrated: usize,
    pub schemas_updated: bool,
    pub no_op: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn normalize(&mut self) {
        self.errors.sort();
        self.errors.dedup();
        self.warnings.sort();
        self.warnings.dedup();
        self.valid = self.errors.is_empty();
    }
}

#[derive(Debug)]
pub enum StoreError {
    Invalid(ValidationReport),
    Conflict(String),
    Environment(String),
}

#[derive(Debug)]
pub struct RepositoryData {
    pub model: Value,
    pub events: Vec<Value>,
}

pub struct DecisionInput {
    pub subject: String,
    pub decision: String,
    pub reason: String,
    pub id: Option<String>,
    pub date: Option<String>,
    pub occurred_at: Option<String>,
    pub rejected: Vec<String>,
    pub supersedes: Vec<String>,
    pub conditions: Option<String>,
    pub evidence: Vec<String>,
    pub evidence_details: Vec<Value>,
    pub relations: Vec<Value>,
}

pub struct AttemptInput {
    pub subject: String,
    pub approach: String,
    pub result: String,
    pub finding: String,
    pub id: Option<String>,
    pub date: Option<String>,
    pub occurred_at: Option<String>,
    pub conditions: Option<String>,
    pub evidence: Vec<String>,
    pub evidence_details: Vec<Value>,
    pub relations: Vec<Value>,
}

pub struct ReconstructionInput {
    pub base_model: PathBuf,
    pub base_events: PathBuf,
    pub model: PathBuf,
    pub events: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ReconstructionReport {
    pub model_changed: bool,
    pub events_added: usize,
    pub duplicates_skipped: usize,
    pub no_op: bool,
}

#[derive(Clone, Copy)]
enum TransactionKind {
    Init,
    Migration,
    Reconstruction,
}

impl TransactionKind {
    fn name(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Migration => "migration",
            Self::Reconstruction => "reconstruction",
        }
    }

    fn paths(self) -> &'static [&'static str] {
        match self {
            Self::Init => &INIT_PATHS,
            Self::Migration => &INIT_PATHS,
            Self::Reconstruction => &RECONSTRUCTION_PATHS,
        }
    }
}

struct RepositoryDocuments {
    model_schema: String,
    event_schema: String,
    model: String,
    events: String,
}

const RECONSTRUCTION_PATHS: [&str; 2] = [
    ".project-context/model.yaml",
    ".project-context/events.jsonl",
];

pub fn initialize(root: &Path, force: bool) -> Result<InitReport, String> {
    let project_directory = root.join(".project-context");
    fs::create_dir_all(&project_directory)
        .map_err(|error| format!("cannot create {}: {error}", project_directory.display()))?;
    let lock = open_lock(root, true)?;
    recover_transaction(root)?;
    cleanup_stale_temporary_files(root);
    let result = initialize_locked(root, force);
    let _ = lock.unlock();
    result
}

fn initialize_locked(root: &Path, force: bool) -> Result<InitReport, String> {
    let mut existing = Vec::new();
    for path in INIT_PATHS {
        match fs::symlink_metadata(root.join(path)) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!(
                        "refusing to replace symbolic link at {path}; remove it manually"
                    ));
                }
                existing.push(path.to_owned());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect {path}: {error}")),
        }
    }
    if !force && !existing.is_empty() {
        return Err(format!(
            "refusing to overwrite existing files: {} (use --force to replace them)",
            existing.join(", ")
        ));
    }

    let model = initial_model(root)?;
    let (template_report, _) = inspect_documents(MODEL_SCHEMA, EVENT_SCHEMA, &model, "");
    if !template_report.valid {
        return Err(format!(
            "embedded initialization templates are invalid: {}",
            template_report.errors.join("; ")
        ));
    }

    let contents = [
        model.into_bytes(),
        Vec::new(),
        MODEL_SCHEMA.as_bytes().to_vec(),
        EVENT_SCHEMA.as_bytes().to_vec(),
    ];
    transactional_replace(root, TransactionKind::Init, &contents)?;
    Ok(InitReport {
        initialized: true,
        files: INIT_PATHS.iter().map(|path| (*path).to_owned()).collect(),
    })
}

fn initial_model(root: &Path) -> Result<String, String> {
    let mut model: serde_yaml_ng::Value = serde_yaml_ng::from_str(MODEL_TEMPLATE)
        .map_err(|error| format!("embedded model template is invalid: {error}"))?;
    let id = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project-context");
    let project = model
        .get_mut("project")
        .and_then(serde_yaml_ng::Value::as_mapping_mut)
        .ok_or_else(|| "embedded model template has no project object".to_owned())?;
    project.insert(
        serde_yaml_ng::Value::String("id".to_owned()),
        serde_yaml_ng::Value::String(id.to_owned()),
    );
    project.remove(serde_yaml_ng::Value::String("description".to_owned()));
    serde_yaml_ng::to_string(&model)
        .map_err(|error| format!("cannot render embedded model template: {error}"))
}

fn transactional_replace(
    root: &Path,
    kind: TransactionKind,
    contents: &[Vec<u8>],
) -> Result<(), String> {
    let paths = kind.paths();
    if paths.len() != contents.len() {
        return Err("transaction contents do not match the allowed path set".to_owned());
    }
    let transaction = root.join(TRANSACTION_DIRECTORY);
    if transaction.exists() {
        return Err("a project-context transaction is already present".to_owned());
    }
    fs::create_dir(&transaction)
        .map_err(|error| format!("cannot create project-context transaction: {error}"))?;
    let staged_root = transaction.join("staged");
    let backup_root = transaction.join("backup");
    write_synced_file(
        &transaction.join("kind"),
        format!("{}\n", kind.name()).as_bytes(),
    )?;
    File::open(&transaction)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync transaction kind marker: {error}"))?;
    fs::create_dir(&staged_root)
        .map_err(|error| format!("cannot create transaction staging area: {error}"))?;
    fs::create_dir(&backup_root)
        .map_err(|error| format!("cannot create transaction backup area: {error}"))?;

    let result = (|| -> Result<(), String> {
        for (relative, content) in paths.iter().zip(contents) {
            let inner = relative.trim_start_matches(".project-context/");
            let staged = staged_root.join(inner);
            if let Some(parent) = staged.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            write_synced_file(&staged, content)?;
            let destination = root.join(relative);
            match fs::symlink_metadata(&destination) {
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return Err(format!(
                        "refusing to replace non-regular transaction target {relative}"
                    ));
                }
                Ok(_) => {
                    for warning in copy_metadata(&destination, &staged)? {
                        eprintln!("warning: {warning}");
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("cannot inspect {relative}: {error}")),
            }
        }
        File::open(&staged_root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("cannot sync transaction staging area: {error}"))?;
        write_synced_file(&transaction.join("prepared"), b"prepared\n")?;
        File::open(&transaction)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("cannot sync prepared transaction: {error}"))?;

        for relative in paths {
            let inner = relative.trim_start_matches(".project-context/");
            let destination = root.join(relative);
            let staged = staged_root.join(inner);
            let backup = backup_root.join(inner);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            if fs::symlink_metadata(&destination).is_ok() {
                if let Some(parent) = backup.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
                }
                fs::rename(&destination, &backup).map_err(|error| {
                    format!("cannot stage existing {relative} for replacement: {error}")
                })?;
            }
            fs::rename(&staged, &destination)
                .map_err(|error| format!("cannot install {relative}: {error}"))?;
        }
        write_synced_file(&transaction.join("committed"), b"committed\n")?;
        File::open(root.join(".project-context"))
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("cannot sync .project-context after transaction: {error}"))?;
        Ok(())
    })();

    if let Err(error) = result {
        let rollback = rollback_transaction(root);
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error}; transaction rollback also failed: {rollback_error}"
            )),
        };
    }
    fs::remove_dir_all(&transaction)
        .map_err(|error| format!("cannot remove committed project-context transaction: {error}"))?;
    Ok(())
}

fn recover_transaction(root: &Path) -> Result<(), String> {
    let transaction = root.join(TRANSACTION_DIRECTORY);
    match fs::symlink_metadata(&transaction) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err("project-context transaction path is not a regular directory".to_owned());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot inspect project-context transaction: {error}"
            ));
        }
    }
    transaction_kind(root)?;
    validate_transaction_member_types(&transaction)?;
    if transaction.join("committed").is_file() {
        fs::remove_dir_all(&transaction).map_err(|error| {
            format!("cannot finish committed project-context transaction: {error}")
        })?;
        return Ok(());
    }
    rollback_transaction(root)
}

fn validate_transaction_member_types(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect project-context transaction member: {error}"))?;
    if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
        return Err(format!(
            "project-context transaction contains an unsupported member: {}",
            path.display()
        ));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|error| format!("cannot read project-context transaction: {error}"))?
        {
            let entry = entry
                .map_err(|error| format!("cannot read project-context transaction: {error}"))?;
            validate_transaction_member_types(&entry.path())?;
        }
    }
    Ok(())
}

fn transaction_kind(root: &Path) -> Result<TransactionKind, String> {
    let marker = root.join(TRANSACTION_DIRECTORY).join("kind");
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err("project-context transaction kind is not a regular file".to_owned());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            for directory in ["staged", "backup"] {
                let path = root.join(TRANSACTION_DIRECTORY).join(directory);
                let metadata = fs::symlink_metadata(&path).map_err(|error| {
                    format!("legacy initialization transaction is incomplete: {error}")
                })?;
                if !metadata.file_type().is_dir() {
                    return Err(format!(
                        "legacy initialization transaction has invalid {directory} directory"
                    ));
                }
            }
            return Ok(TransactionKind::Init);
        }
        Err(error) => {
            return Err(format!(
                "cannot inspect project-context transaction kind: {error}"
            ));
        }
    }
    match fs::read_to_string(&marker) {
        Ok(value) => match value.as_str() {
            "init\n" => Ok(TransactionKind::Init),
            "migration\n" => Ok(TransactionKind::Migration),
            "reconstruction\n" => Ok(TransactionKind::Reconstruction),
            other => Err(format!(
                "unknown project-context transaction kind: {}",
                other.escape_default()
            )),
        },
        Err(error) => Err(format!(
            "cannot read project-context transaction kind: {error}"
        )),
    }
}

fn rollback_transaction(root: &Path) -> Result<(), String> {
    let transaction = root.join(TRANSACTION_DIRECTORY);
    let kind = transaction_kind(root)?;
    validate_transaction_member_types(&transaction)?;
    let typed_transaction = transaction.join("kind").is_file();
    let prepared = transaction.join("prepared").is_file();
    let staged_root = transaction.join("staged");
    let backup_root = transaction.join("backup");
    for relative in kind.paths().iter().rev() {
        let inner = relative.trim_start_matches(".project-context/");
        let destination = root.join(relative);
        let staged = staged_root.join(inner);
        let backup = backup_root.join(inner);
        if backup.exists() {
            if destination.exists() {
                remove_regular_file(&destination)?;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot restore {}: {error}", parent.display()))?;
            }
            fs::rename(&backup, &destination)
                .map_err(|error| format!("cannot restore {relative}: {error}"))?;
        } else if (!typed_transaction || prepared) && !staged.exists() && destination.exists() {
            remove_regular_file(&destination)?;
        }
    }
    fs::remove_dir_all(&transaction)
        .map_err(|error| format!("cannot remove rolled-back transaction: {error}"))?;
    Ok(())
}

fn remove_regular_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "refusing to remove non-regular transaction target {}",
            path.display()
        ));
    }
    fs::remove_file(path).map_err(|error| format!("cannot remove {}: {error}", path.display()))
}

fn write_synced_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn cleanup_stale_temporary_files(root: &Path) {
    let project = root.join(".project-context");
    let Ok(entries) = fs::read_dir(&project) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(".events.jsonl.tmp-") {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn discover_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| candidate.join(".project-context").is_dir())
        .map(Path::to_path_buf)
}

pub fn validate_repository(root: &Path) -> Result<ValidationReport, StoreError> {
    recover_before_read(root)?;
    let lock = open_lock(root, false).map_err(StoreError::Environment)?;
    let documents = read_documents(root)?;
    let (mut report, data) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &documents.model,
        &documents.events,
    );
    add_git_validation_warnings(root, &data, &mut report);
    report.normalize();
    let _ = lock.unlock();
    Ok(report)
}

pub fn load_valid_repository(root: &Path) -> Result<RepositoryData, StoreError> {
    recover_before_read(root)?;
    let lock = open_lock(root, false).map_err(StoreError::Environment)?;
    let documents = read_documents(root)?;
    let (report, data) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &documents.model,
        &documents.events,
    );
    let _ = lock.unlock();
    if report.valid {
        Ok(data)
    } else {
        Err(StoreError::Invalid(report))
    }
}

pub fn configure(root: &Path, input: ConfigureInput) -> Result<ConfigureReport, StoreError> {
    let lock = open_lock(root, true).map_err(StoreError::Environment)?;
    recover_transaction(root).map_err(StoreError::Environment)?;
    let documents = read_documents(root)?;
    let (current_report, _) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &documents.model,
        &documents.events,
    );
    if !current_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(current_report));
    }

    let mut model: serde_yaml_ng::Value = serde_yaml_ng::from_str(&documents.model)
        .map_err(|error| StoreError::Environment(format!("cannot parse model.yaml: {error}")))?;
    let model_mapping = model
        .as_mapping_mut()
        .ok_or_else(|| StoreError::Environment("model.yaml root must be a mapping".to_owned()))?;
    let mut updated = Vec::new();

    if input.project_id.is_some() || input.description.is_some() {
        let project = model_mapping
            .get_mut(serde_yaml_ng::Value::String("project".to_owned()))
            .and_then(serde_yaml_ng::Value::as_mapping_mut)
            .ok_or_else(|| {
                StoreError::Environment("model.yaml has no project mapping".to_owned())
            })?;
        if let Some(value) = input.project_id {
            project.insert(
                serde_yaml_ng::Value::String("id".to_owned()),
                serde_yaml_ng::Value::String(value),
            );
            updated.push("project.id".to_owned());
        }
        if let Some(value) = input.description {
            project.insert(
                serde_yaml_ng::Value::String("description".to_owned()),
                serde_yaml_ng::Value::String(value),
            );
            updated.push("project.description".to_owned());
        }
    }

    let schema_version = model_mapping
        .get(serde_yaml_ng::Value::String("schema_version".to_owned()))
        .and_then(serde_yaml_ng::Value::as_i64)
        .unwrap_or(1);
    let mut operation_updates = BTreeMap::from([
        ("build".to_owned(), input.build),
        ("test".to_owned(), input.test),
        ("lint".to_owned(), input.lint),
        ("format".to_owned(), input.format),
    ]);
    for (name, commands) in input.operations {
        operation_updates.entry(name).or_default().extend(commands);
    }
    operation_updates.retain(|_, commands| !commands.is_empty());
    if !operation_updates.is_empty() {
        let operations = model_mapping
            .get_mut(serde_yaml_ng::Value::String("operations".to_owned()))
            .and_then(serde_yaml_ng::Value::as_mapping_mut)
            .ok_or_else(|| {
                StoreError::Environment("model.yaml has no operations mapping".to_owned())
            })?;
        for (name, commands) in operation_updates {
            if schema_version == 1 && !["build", "test", "lint", "format"].contains(&name.as_str())
            {
                let _ = lock.unlock();
                return Err(StoreError::Environment(format!(
                    "custom operation '{name}' requires project-context migrate"
                )));
            }
            let commands = if schema_version == 1 {
                commands
                    .into_iter()
                    .map(serde_yaml_ng::Value::String)
                    .collect()
            } else {
                commands
                    .into_iter()
                    .map(|command| {
                        serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::from_iter([(
                            serde_yaml_ng::Value::String("command".to_owned()),
                            serde_yaml_ng::Value::String(command),
                        )]))
                    })
                    .collect()
            };
            operations.insert(
                serde_yaml_ng::Value::String(name.to_owned()),
                serde_yaml_ng::Value::Sequence(commands),
            );
            updated.push(format!("operations.{name}"));
        }
    }

    if updated.is_empty() {
        let _ = lock.unlock();
        return Ok(ConfigureReport { updated });
    }
    let proposed_model = serde_yaml_ng::to_string(&model).map_err(|error| {
        StoreError::Environment(format!("cannot serialize model.yaml: {error}"))
    })?;
    let (proposed_report, _) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &proposed_model,
        &documents.events,
    );
    if !proposed_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(proposed_report));
    }
    let warnings = atomic_write(
        &root.join(".project-context/model.yaml"),
        proposed_model.as_bytes(),
    )
    .map_err(StoreError::Environment)?;
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    let _ = lock.unlock();
    Ok(ConfigureReport { updated })
}

pub fn migrate(root: &Path) -> Result<MigrateReport, StoreError> {
    let lock = open_lock(root, true).map_err(StoreError::Environment)?;
    recover_transaction(root).map_err(StoreError::Environment)?;
    let documents = read_documents(root)?;
    let (current_report, current_data) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &documents.model,
        &documents.events,
    );
    if !current_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(current_report));
    }

    let mut model = current_data.model;
    let model_migrated = model.get("schema_version").and_then(Value::as_u64) == Some(1);
    if model_migrated {
        migrate_model_value(&mut model);
    }
    let mut events = current_data.events;
    let mut events_migrated = 0;
    for event in &mut events {
        if event.get("schema_version").and_then(Value::as_u64) == Some(1) {
            migrate_event_value(event);
            events_migrated += 1;
        }
    }
    let schemas_updated = !schema_matches(&documents.model_schema, MODEL_SCHEMA)
        || !schema_matches(&documents.event_schema, EVENT_SCHEMA);
    let no_op = !model_migrated && events_migrated == 0 && !schemas_updated;
    if no_op {
        let _ = lock.unlock();
        return Ok(MigrateReport {
            model_migrated,
            events_migrated,
            schemas_updated,
            no_op,
        });
    }

    let proposed_model = serde_yaml_ng::to_string(&model).map_err(|error| {
        StoreError::Environment(format!("cannot serialize migrated model: {error}"))
    })?;
    let proposed_events = serialize_event_values(&events)?;
    let (proposed_report, _) = inspect_documents(
        MODEL_SCHEMA,
        EVENT_SCHEMA,
        &proposed_model,
        &proposed_events,
    );
    if !proposed_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(proposed_report));
    }
    transactional_replace(
        root,
        TransactionKind::Migration,
        &[
            proposed_model.into_bytes(),
            proposed_events.into_bytes(),
            MODEL_SCHEMA.as_bytes().to_vec(),
            EVENT_SCHEMA.as_bytes().to_vec(),
        ],
    )
    .map_err(StoreError::Environment)?;
    let _ = lock.unlock();
    Ok(MigrateReport {
        model_migrated,
        events_migrated,
        schemas_updated,
        no_op,
    })
}

fn migrate_model_value(model: &mut Value) {
    model["schema_version"] = Value::from(2);
    for section in ["principles", "architecture", "behaviors", "constraints"] {
        let Some(entries) = model.get_mut(section).and_then(Value::as_array_mut) else {
            continue;
        };
        for entry in entries {
            let Some(object) = entry.as_object_mut() else {
                continue;
            };
            if let Some(Value::Array(references)) = object.remove("related_events") {
                object.insert(
                    "event_relations".to_owned(),
                    Value::Array(
                        references
                            .into_iter()
                            .filter_map(|reference| reference.as_str().map(str::to_owned))
                            .map(|event| {
                                json_object([("event", event), ("kind", "related".to_owned())])
                            })
                            .collect(),
                    ),
                );
            }
        }
    }
    if let Some(operations) = model.get_mut("operations").and_then(Value::as_object_mut) {
        for steps in operations.values_mut() {
            let Some(values) = steps.as_array_mut() else {
                continue;
            };
            for step in values.iter_mut() {
                if let Some(command) = step.as_str().map(str::to_owned) {
                    *step = json_object([("command", command)]);
                }
            }
        }
    }
}

fn migrate_event_value(event: &mut Value) {
    let Some(object) = event.as_object_mut() else {
        return;
    };
    object.insert("schema_version".to_owned(), Value::from(2));
    if let Some(Value::Array(evidence)) = object.get_mut("evidence") {
        for item in evidence.iter_mut() {
            if let Some(reference) = item.as_str().map(str::to_owned) {
                *item = json_object([("ref", reference)]);
            }
        }
    }
    if let Some(Value::Array(supersedes)) = object.remove("supersedes") {
        let relations = object
            .entry("relations".to_owned())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(relations) = relations.as_array_mut() {
            relations.extend(
                supersedes
                    .into_iter()
                    .filter_map(|target| target.as_str().map(str::to_owned))
                    .map(|target| {
                        json_object([("event", target), ("kind", "supersedes".to_owned())])
                    }),
            );
        }
    }
}

fn json_object<const N: usize>(pairs: [(&str, String); N]) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), Value::String(value)))
            .collect(),
    )
}

fn serialize_event_values(events: &[Value]) -> Result<String, StoreError> {
    let mut output = String::new();
    for event in events {
        output.push_str(&serde_json::to_string(event).map_err(|error| {
            StoreError::Environment(format!("cannot serialize event: {error}"))
        })?);
        output.push('\n');
    }
    Ok(output)
}

pub fn apply_reconstruction(
    root: &Path,
    input: ReconstructionInput,
) -> Result<ReconstructionReport, StoreError> {
    let base_model = read_input_file(&input.base_model, "base model")?;
    let base_events = read_input_file(&input.base_events, "base events")?;
    let proposed_model = read_input_file(&input.model, "proposed model")?;
    let proposed_new_events = read_input_file(&input.events, "proposed events")?;

    let lock = open_lock(root, true).map_err(StoreError::Environment)?;
    recover_transaction(root).map_err(StoreError::Environment)?;
    let documents = read_documents(root)?;
    if documents.model.as_bytes() != base_model.as_bytes()
        || documents.events.as_bytes() != base_events.as_bytes()
    {
        let _ = lock.unlock();
        return Err(StoreError::Conflict(
            "project-context data changed after reconstruction began".to_owned(),
        ));
    }

    let (current_report, current_data) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &documents.model,
        &documents.events,
    );
    if !current_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(current_report));
    }

    let base_model_value = yaml_to_json(&documents.model, "base model")?;
    let mut proposed_model_value = yaml_to_json(&proposed_model, "proposed model")?;
    let mut merge_errors = validate_model_merge(&base_model_value, &proposed_model_value);
    let mut candidates =
        parse_event_lines(&proposed_new_events, "proposed events", &mut merge_errors);
    let mut candidate_ids = BTreeSet::new();
    for event in &candidates {
        if let Some(id) = event.get("id").and_then(Value::as_str) {
            if !valid_candidate_key(id) {
                merge_errors.push(format!(
                    "proposed candidate ID '{id}' must use the candidate: namespace"
                ));
            } else if !candidate_ids.insert(id.to_owned()) {
                merge_errors.push(format!("duplicate proposed candidate ID '{id}'"));
            }
        }
    }
    candidates.sort_by_key(|event| event_date(event).to_owned());

    let mut existing_by_key = BTreeMap::new();
    for event in &current_data.events {
        if let (Some(key), Some(id)) = (
            event_dedupe_key(event),
            event.get("id").and_then(Value::as_str),
        ) {
            existing_by_key.insert(key, (id.to_owned(), event.clone()));
        }
    }

    let mut id_remap = BTreeMap::new();
    let mut seen_new: BTreeMap<String, (String, Value)> = BTreeMap::new();
    let mut supersession_checks = Vec::new();
    let mut retained = Vec::new();
    let mut duplicates_skipped = 0;
    for event in candidates.drain(..) {
        let Some(id) = event.get("id").and_then(Value::as_str).map(str::to_owned) else {
            retained.push(event);
            continue;
        };
        let Some(key) = event_dedupe_key(&event) else {
            retained.push(event);
            continue;
        };
        if let Some((existing_id, existing)) = existing_by_key.get(&key) {
            supersession_checks.push((
                id.clone(),
                existing_id.clone(),
                event.clone(),
                existing.clone(),
            ));
            id_remap.insert(id, existing_id.clone());
            duplicates_skipped += 1;
        } else if let Some((first_id, first_event)) = seen_new.get(&key) {
            supersession_checks.push((
                id.clone(),
                first_id.clone(),
                event.clone(),
                first_event.clone(),
            ));
            id_remap.insert(id, first_id.clone());
            duplicates_skipped += 1;
        } else {
            seen_new.insert(key, (id, event.clone()));
            retained.push(event);
        }
    }

    // Stable ordering preserves evidence-derived order when only day precision exists.
    retained.sort_by_key(event_timeline_key);
    let mut allocated_events = current_data.events.clone();
    for event in &mut retained {
        let Some(candidate_id) = event.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let prefix = match event.get("type").and_then(Value::as_str) {
            Some("decision") => "D",
            Some("attempt") => "A",
            _ => continue,
        };
        let allocated_id = next_event_id(prefix, &allocated_events).map_err(|error| {
            StoreError::Environment(format!("cannot allocate event ID: {error}"))
        })?;
        event["id"] = Value::String(allocated_id.clone());
        id_remap.insert(candidate_id, allocated_id);
        allocated_events.push(event.clone());
    }
    let resolved_remap = match resolve_reference_map(&id_remap) {
        Ok(remap) => remap,
        Err(error) => {
            merge_errors.push(error);
            BTreeMap::new()
        }
    };
    for (candidate_id, matched_id, mut candidate, mut matched) in supersession_checks {
        remap_event_references(std::slice::from_mut(&mut candidate), &resolved_remap);
        remap_event_references(std::slice::from_mut(&mut matched), &resolved_remap);
        if normalized_relations(&candidate) != normalized_relations(&matched) {
            merge_errors.push(format!(
                "proposed event '{candidate_id}' duplicates '{matched_id}' with different relations"
            ));
        }
    }
    remap_event_references(&mut retained, &resolved_remap);
    remap_model_references(&mut proposed_model_value, &resolved_remap);
    let proposed_model = serde_yaml_ng::to_string(&proposed_model_value).map_err(|error| {
        StoreError::Environment(format!("cannot serialize proposed model: {error}"))
    })?;

    let combined_events =
        merge_events_in_timeline_order(&documents.events, &current_data.events, &retained)?;

    let (mut proposed_report, _) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &proposed_model,
        &combined_events,
    );
    proposed_report.errors.extend(merge_errors);
    proposed_report.normalize();
    if !proposed_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(proposed_report));
    }

    let model_changed = proposed_model.as_bytes() != documents.model.as_bytes();
    let events_changed = combined_events.as_bytes() != documents.events.as_bytes();
    let no_op = !model_changed && !events_changed;
    if !no_op {
        transactional_replace(
            root,
            TransactionKind::Reconstruction,
            &[proposed_model.into_bytes(), combined_events.into_bytes()],
        )
        .map_err(StoreError::Environment)?;
    }
    let _ = lock.unlock();
    Ok(ReconstructionReport {
        model_changed,
        events_added: retained.len(),
        duplicates_skipped,
        no_op,
    })
}

fn valid_candidate_key(id: &str) -> bool {
    id.strip_prefix("candidate:").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 240
            && suffix
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._:-".contains(character))
    })
}

fn event_date(event: &Value) -> &str {
    event.get("date").and_then(Value::as_str).unwrap_or("")
}

fn event_timeline_key(event: &Value) -> i128 {
    if let Some(timestamp) = event
        .get("occurred_at")
        .and_then(Value::as_str)
        .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
    {
        return timestamp.unix_timestamp_nanos();
    }
    Date::parse(
        event_date(event),
        &format_description!("[year]-[month]-[day]"),
    )
    .ok()
    .and_then(|date| date.with_hms(0, 0, 0).ok())
    .map(|date_time| date_time.assume_utc().unix_timestamp_nanos())
    .unwrap_or(i128::MIN)
}

fn merge_events_in_timeline_order(
    existing_text: &str,
    existing_events: &[Value],
    new_events: &[Value],
) -> Result<String, StoreError> {
    let already_ordered = existing_events
        .windows(2)
        .all(|events| event_timeline_key(&events[0]) <= event_timeline_key(&events[1]));
    if new_events.is_empty() && already_ordered {
        return Ok(existing_text.to_owned());
    }

    let existing_lines: Vec<&str> = existing_text.lines().collect();
    if existing_lines.len() != existing_events.len() {
        return Err(StoreError::Environment(
            "cannot align canonical event lines with parsed events".to_owned(),
        ));
    }
    let mut lines: Vec<(i128, String)> = existing_events
        .iter()
        .zip(existing_lines)
        .map(|(event, line)| (event_timeline_key(event), line.to_owned()))
        .collect();
    for event in new_events {
        let line = serde_json::to_string(event)
            .map_err(|error| StoreError::Environment(format!("cannot serialize event: {error}")))?;
        lines.push((event_timeline_key(event), line));
    }
    // Rust's stable sort retains canonical or candidate order when only day precision exists.
    lines.sort_by_key(|line| line.0);
    Ok(lines
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n")
}

fn resolve_reference_map(
    remap: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    remap
        .keys()
        .map(|source| {
            let mut target = source.clone();
            let mut visited = BTreeSet::new();
            while let Some(next) = remap.get(&target) {
                if !visited.insert(target.clone()) {
                    return Err(format!(
                        "candidate reference remap cycle includes '{source}'"
                    ));
                }
                target = next.clone();
            }
            Ok((source.clone(), target))
        })
        .collect()
}

fn read_input_file(path: &Path, label: &str) -> Result<String, StoreError> {
    fs::read_to_string(path).map_err(|error| {
        StoreError::Environment(format!("cannot read {label} {}: {error}", path.display()))
    })
}

fn yaml_to_json(content: &str, label: &str) -> Result<Value, StoreError> {
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(content)
        .map_err(|error| invalid_data_error(format!("cannot parse {label}: {error}")))?;
    serde_json::to_value(yaml)
        .map_err(|error| invalid_data_error(format!("cannot convert {label}: {error}")))
}

fn invalid_data_error(message: String) -> StoreError {
    let mut report = ValidationReport::default();
    report.errors.push(message);
    report.normalize();
    StoreError::Invalid(report)
}

fn validate_model_merge(base: &Value, proposed: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(project) = base.get("project").and_then(Value::as_object) {
        for (field, base_value) in project {
            let proposed_value = proposed.get("project").and_then(|value| value.get(field));
            if !model_value_is_empty(base_value) && proposed_value != Some(base_value) {
                errors.push(format!("proposed model changes existing project.{field}"));
            }
        }
    }
    for section in ["principles", "architecture", "behaviors", "constraints"] {
        let proposed_entries = proposed
            .get(section)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in base
            .get(section)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !proposed_entries.contains(entry) {
                let id = entry
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                errors.push(format!(
                    "proposed model changes or removes existing {section} entry '{id}'"
                ));
            }
        }
    }
    let base_entries = model_entries(base);
    let base_ids: BTreeSet<&str> = base_entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect();
    let base_statements: BTreeSet<String> = base_entries
        .iter()
        .filter_map(|entry| normalized_model_statement(entry))
        .collect();
    let mut new_ids = BTreeSet::new();
    let mut new_statements = BTreeSet::new();
    for entry in model_entries(proposed) {
        if base_entries.contains(&entry) {
            continue;
        }
        if let Some(id) = entry.get("id").and_then(Value::as_str)
            && (base_ids.contains(id) || !new_ids.insert(id.to_owned()))
        {
            errors.push(format!(
                "proposed model entry ID '{id}' conflicts with another entry"
            ));
        }
        if let Some(statement) = normalized_model_statement(entry)
            && (base_statements.contains(&statement) || !new_statements.insert(statement.clone()))
        {
            errors.push(format!(
                "proposed model statement conflicts with another entry: {statement}"
            ));
        }
    }
    if let Some(operations) = base.get("operations").and_then(Value::as_object) {
        for (operation, base_value) in operations {
            if base_value
                .as_array()
                .is_some_and(|values| !values.is_empty())
                && proposed
                    .get("operations")
                    .and_then(|value| value.get(operation))
                    != Some(base_value)
            {
                errors.push(format!(
                    "proposed model changes non-empty operations.{operation}"
                ));
            }
        }
    }
    if proposed.get("extensions") != base.get("extensions") {
        errors.push("proposed model changes existing extensions".to_owned());
    }
    errors
}

fn model_entries(model: &Value) -> Vec<&Value> {
    ["principles", "architecture", "behaviors", "constraints"]
        .into_iter()
        .flat_map(|section| {
            model
                .get(section)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect()
}

fn normalized_model_statement(entry: &Value) -> Option<String> {
    entry
        .get("statement")
        .and_then(Value::as_str)
        .map(|statement| statement.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn model_value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => values.is_empty(),
        _ => false,
    }
}

fn parse_event_lines(content: &str, label: &str, errors: &mut Vec<String>) -> Vec<Value> {
    let mut events = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            errors.push(format!("{label} line {} is empty", index + 1));
            continue;
        }
        match serde_json::from_str::<UniqueValue>(line) {
            Ok(UniqueValue(value)) => events.push(value),
            Err(error) => errors.push(format!(
                "{label} line {} is not valid JSON: {error}",
                index + 1
            )),
        }
    }
    events
}

fn event_dedupe_key(event: &Value) -> Option<String> {
    let mut value = event.clone();
    let object = value.as_object_mut()?;
    object.remove("id");
    object.remove("supersedes");
    object.remove("relations");
    normalize_semantic_value(&mut value);
    serde_json::to_string(&value).ok()
}

fn normalize_semantic_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        }
        Value::Array(values) => {
            for value in values.iter_mut() {
                normalize_semantic_value(value);
            }
            values.sort_by_key(|value| serde_json::to_string(value).unwrap_or_default());
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                normalize_semantic_value(value);
            }
        }
        _ => {}
    }
}

fn normalized_relations(event: &Value) -> Vec<String> {
    let mut values: Vec<Value> = event
        .get("supersedes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|event| {
            json_object([
                ("event", event.to_owned()),
                ("kind", "supersedes".to_owned()),
            ])
        })
        .collect();
    values.extend(
        event
            .get("relations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .cloned(),
    );
    values.iter_mut().for_each(normalize_semantic_value);
    let mut values: Vec<String> = values
        .into_iter()
        .filter_map(|value| serde_json::to_string(&value).ok())
        .collect();
    values.sort();
    values.dedup();
    values
}

fn remap_event_references(events: &mut [Value], remap: &BTreeMap<String, String>) {
    for event in events {
        if let Some(references) = event.get_mut("supersedes").and_then(Value::as_array_mut) {
            for reference in references.iter_mut() {
                if let Some(id) = reference.as_str()
                    && let Some(replacement) = remap.get(id)
                {
                    *reference = Value::String(replacement.clone());
                }
            }
        }
        if let Some(relations) = event.get_mut("relations").and_then(Value::as_array_mut) {
            for relation in relations {
                if let Some(reference) = relation.get_mut("event")
                    && let Some(id) = reference.as_str()
                    && let Some(replacement) = remap.get(id)
                {
                    *reference = Value::String(replacement.clone());
                }
            }
        }
    }
}

fn remap_model_references(model: &mut Value, remap: &BTreeMap<String, String>) {
    for section in ["principles", "architecture", "behaviors", "constraints"] {
        let Some(entries) = model.get_mut(section).and_then(Value::as_array_mut) else {
            continue;
        };
        for entry in entries {
            if let Some(references) = entry
                .get_mut("related_events")
                .and_then(Value::as_array_mut)
            {
                for reference in references {
                    if let Some(id) = reference.as_str()
                        && let Some(replacement) = remap.get(id)
                    {
                        *reference = Value::String(replacement.clone());
                    }
                }
            }
            if let Some(relations) = entry
                .get_mut("event_relations")
                .and_then(Value::as_array_mut)
            {
                for relation in relations {
                    if let Some(reference) = relation.get_mut("event")
                        && let Some(id) = reference.as_str()
                        && let Some(replacement) = remap.get(id)
                    {
                        *reference = Value::String(replacement.clone());
                    }
                }
            }
        }
    }
}

pub fn add_decision(root: &Path, input: DecisionInput) -> Result<Value, StoreError> {
    append_event(root, move |events, schema_version| {
        let mut event = base_event(
            "decision",
            input.id,
            input.date,
            input.occurred_at,
            &input.subject,
            events,
            schema_version,
        )?;
        event.insert("decision".to_owned(), Value::String(input.decision));
        event.insert("reason".to_owned(), Value::String(input.reason));
        insert_array(&mut event, "rejected", input.rejected);
        insert_optional(&mut event, "conditions", input.conditions);
        insert_event_metadata(
            &mut event,
            schema_version,
            input.evidence,
            input.evidence_details,
            input.supersedes,
            input.relations,
        )?;
        Ok(Value::Object(event))
    })
}

pub fn add_attempt(root: &Path, input: AttemptInput) -> Result<Value, StoreError> {
    append_event(root, move |events, schema_version| {
        let mut event = base_event(
            "attempt",
            input.id,
            input.date,
            input.occurred_at,
            &input.subject,
            events,
            schema_version,
        )?;
        event.insert("approach".to_owned(), Value::String(input.approach));
        event.insert("result".to_owned(), Value::String(input.result));
        event.insert("finding".to_owned(), Value::String(input.finding));
        insert_optional(&mut event, "conditions", input.conditions);
        insert_event_metadata(
            &mut event,
            schema_version,
            input.evidence,
            input.evidence_details,
            Vec::new(),
            input.relations,
        )?;
        Ok(Value::Object(event))
    })
}

fn append_event<F>(root: &Path, create: F) -> Result<Value, StoreError>
where
    F: FnOnce(&[Value], u64) -> Result<Value, String>,
{
    let lock = open_lock(root, true).map_err(StoreError::Environment)?;
    recover_transaction(root).map_err(StoreError::Environment)?;
    cleanup_stale_temporary_files(root);
    let documents = read_documents(root)?;
    let (current_report, data) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &documents.model,
        &documents.events,
    );
    if !current_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(current_report));
    }

    let schema_version = data
        .model
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let event = create(&data.events, schema_version).map_err(StoreError::Environment)?;
    let proposed_events = merge_events_in_timeline_order(
        &documents.events,
        &data.events,
        std::slice::from_ref(&event),
    )?;

    let (proposed_report, _) = inspect_documents(
        &documents.model_schema,
        &documents.event_schema,
        &documents.model,
        &proposed_events,
    );
    if !proposed_report.valid {
        let _ = lock.unlock();
        return Err(StoreError::Invalid(proposed_report));
    }

    let warnings = atomic_write(
        &root.join(".project-context/events.jsonl"),
        proposed_events.as_bytes(),
    )
    .map_err(StoreError::Environment)?;
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    let _ = lock.unlock();
    Ok(event)
}

fn base_event(
    kind: &str,
    requested_id: Option<String>,
    requested_date: Option<String>,
    occurred_at: Option<String>,
    subject: &str,
    events: &[Value],
    schema_version: u64,
) -> Result<Map<String, Value>, String> {
    let prefix = match kind {
        "decision" => "D",
        "attempt" => "A",
        _ => return Err(format!("unsupported event type '{kind}'")),
    };
    let id = match requested_id {
        Some(id) => id,
        None => next_event_id(prefix, events)?,
    };
    let date = requested_date
        .or_else(|| {
            occurred_at
                .as_deref()
                .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
                .map(|timestamp| timestamp.date().to_string())
        })
        .unwrap_or_else(|| OffsetDateTime::now_utc().date().to_string());
    let mut event = Map::new();
    event.insert("schema_version".to_owned(), Value::from(schema_version));
    event.insert("type".to_owned(), Value::String(kind.to_owned()));
    event.insert("id".to_owned(), Value::String(id));
    event.insert("date".to_owned(), Value::String(date));
    if let Some(occurred_at) = occurred_at {
        if schema_version == 1 {
            return Err("--occurred-at requires project-context migrate".to_owned());
        }
        event.insert("occurred_at".to_owned(), Value::String(occurred_at));
    }
    event.insert("subject".to_owned(), Value::String(subject.to_owned()));
    Ok(event)
}

fn insert_event_metadata(
    event: &mut Map<String, Value>,
    schema_version: u64,
    evidence: Vec<String>,
    evidence_details: Vec<Value>,
    supersedes: Vec<String>,
    mut relations: Vec<Value>,
) -> Result<(), String> {
    if schema_version == 1 {
        if !evidence_details.is_empty() || !relations.is_empty() {
            return Err(
                "structured evidence and relations require project-context migrate".to_owned(),
            );
        }
        insert_array(event, "supersedes", supersedes);
        insert_array(event, "evidence", evidence);
        return Ok(());
    }
    let mut evidence_values: Vec<Value> = evidence
        .into_iter()
        .map(|reference| json_object([("ref", reference)]))
        .collect();
    evidence_values.extend(evidence_details);
    if !evidence_values.is_empty() {
        event.insert("evidence".to_owned(), Value::Array(evidence_values));
    }
    relations.extend(
        supersedes
            .into_iter()
            .map(|target| json_object([("event", target), ("kind", "supersedes".to_owned())])),
    );
    if !relations.is_empty() {
        event.insert("relations".to_owned(), Value::Array(relations));
    }
    Ok(())
}

fn next_event_id(prefix: &str, events: &[Value]) -> Result<String, String> {
    let mut maximum = "0".to_owned();
    for id in events
        .iter()
        .filter_map(|event| event.get("id"))
        .filter_map(Value::as_str)
    {
        if let Some(number) = id.strip_prefix(&format!("{prefix}-"))
            && number.chars().all(|character| character.is_ascii_digit())
            && decimal_greater(number, &maximum)
        {
            maximum = number.trim_start_matches('0').to_owned();
            if maximum.is_empty() {
                maximum.push('0');
            }
        }
    }
    let next = increment_decimal(&maximum);
    Ok(format!("{prefix}-{next:0>4}"))
}

fn decimal_greater(candidate: &str, current: &str) -> bool {
    let candidate = candidate.trim_start_matches('0');
    let candidate = if candidate.is_empty() { "0" } else { candidate };
    let current = current.trim_start_matches('0');
    let current = if current.is_empty() { "0" } else { current };
    candidate.len() > current.len() || (candidate.len() == current.len() && candidate > current)
}

fn increment_decimal(value: &str) -> String {
    let mut digits: Vec<u8> = value.bytes().collect();
    let mut carry = 1_u8;
    for digit in digits.iter_mut().rev() {
        if carry == 0 {
            break;
        }
        let next = (*digit - b'0') + carry;
        *digit = b'0' + (next % 10);
        carry = next / 10;
    }
    if carry > 0 {
        digits.insert(0, b'1');
    }
    String::from_utf8(digits).expect("decimal digits are UTF-8")
}

fn insert_optional(event: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        event.insert(key.to_owned(), Value::String(value));
    }
}

fn insert_array(event: &mut Map<String, Value>, key: &str, values: Vec<String>) {
    if !values.is_empty() {
        event.insert(
            key.to_owned(),
            Value::Array(values.into_iter().map(Value::String).collect()),
        );
    }
}

fn open_lock(root: &Path, exclusive: bool) -> Result<File, String> {
    let lock_path = root.join(".project-context/.lock");
    let mut options = OpenOptions::new();
    options.read(true);
    if exclusive {
        options.create(true).write(true).truncate(false);
    }
    let lock = options
        .open(&lock_path)
        .map_err(|error| format!("cannot open project-context lock: {error}"))?;
    let result = if exclusive {
        FileExt::try_lock_exclusive(&lock)
    } else {
        FileExt::try_lock_shared(&lock)
    };
    result.map_err(|error| format!("project-context data is being updated: {error}"))?;
    Ok(lock)
}

fn recover_before_read(root: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(root.join(TRANSACTION_DIRECTORY)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(StoreError::Environment(format!(
                "cannot inspect project-context transaction: {error}"
            )));
        }
        Ok(_) => {}
    }
    let lock = open_lock(root, true).map_err(StoreError::Environment)?;
    let result = recover_transaction(root).map_err(StoreError::Environment);
    let _ = lock.unlock();
    result
}

fn read_documents(root: &Path) -> Result<RepositoryDocuments, StoreError> {
    Ok(RepositoryDocuments {
        model_schema: read_required(&root.join(".project-context/schemas/model.schema.json"))?,
        event_schema: read_required(&root.join(".project-context/schemas/event.schema.json"))?,
        model: read_required(&root.join(".project-context/model.yaml"))?,
        events: read_required(&root.join(".project-context/events.jsonl"))?,
    })
}

fn read_required(path: &Path) -> Result<String, StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            let mut report = ValidationReport::default();
            report.errors.push(format!(
                "required project-context path is not a regular file: {}",
                path.display()
            ));
            report.normalize();
            return Err(StoreError::Invalid(report));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut report = ValidationReport::default();
            report.errors.push(format!(
                "required project-context file is missing: {}",
                path.display()
            ));
            report.normalize();
            return Err(StoreError::Invalid(report));
        }
        Err(error) => {
            return Err(StoreError::Environment(format!(
                "cannot inspect {}: {error}",
                path.display()
            )));
        }
    }
    fs::read_to_string(path).map_err(|error| {
        StoreError::Environment(format!("cannot read {}: {error}", path.display()))
    })
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueValue(value)) = sequence.next_element::<UniqueValue>()? {
            values.push(value);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate object key '{key}'")));
            }
            let UniqueValue(value) = object.next_value::<UniqueValue>()?;
            values.insert(key, value);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

fn inspect_documents(
    model_schema_text: &str,
    event_schema_text: &str,
    model_text: &str,
    events_text: &str,
) -> (ValidationReport, RepositoryData) {
    let mut report = ValidationReport::default();
    let model_schema = accepted_schema(
        "model.schema.json",
        model_schema_text,
        MODEL_SCHEMA,
        MODEL_SCHEMA_V1,
        &mut report,
    );
    let event_schema = accepted_schema(
        "event.schema.json",
        event_schema_text,
        EVENT_SCHEMA,
        EVENT_SCHEMA_V1,
        &mut report,
    );
    let model_validator = model_schema
        .and_then(|schema| compile_schema("embedded model.schema.json", schema, &mut report));
    let event_validator = event_schema
        .and_then(|schema| compile_schema("embedded event.schema.json", schema, &mut report));

    let model = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(model_text) {
        Ok(model_yaml) => match serde_json::to_value(model_yaml) {
            Ok(model) => {
                if let Some(validator) = &model_validator {
                    collect_schema_errors("model.yaml", validator, &model, &mut report.errors);
                }
                model
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("model.yaml cannot be converted to JSON: {error}"));
                Value::Null
            }
        },
        Err(error) => {
            report
                .errors
                .push(format!("model.yaml is not valid YAML: {error}"));
            Value::Null
        }
    };

    let mut events = Vec::new();
    for (index, line) in events_text.lines().enumerate() {
        if line.trim().is_empty() {
            report
                .errors
                .push(format!("events.jsonl line {} is empty", index + 1));
            continue;
        }
        match serde_json::from_str::<UniqueValue>(line) {
            Ok(UniqueValue(event)) => {
                if let Some(validator) = &event_validator {
                    collect_schema_errors(
                        &format!("events.jsonl line {}", index + 1),
                        validator,
                        &event,
                        &mut report.errors,
                    );
                }
                events.push(event);
            }
            Err(error) => report.errors.push(format!(
                "events.jsonl line {} is not valid JSON: {error}",
                index + 1
            )),
        }
    }

    validate_cross_records(&model, &events, &mut report.errors);
    report.normalize();
    (report, RepositoryData { model, events })
}

fn accepted_schema(
    label: &str,
    content: &str,
    current: &'static str,
    legacy: &'static str,
    report: &mut ValidationReport,
) -> Option<&'static str> {
    let local = serde_json::from_str::<Value>(content);
    let current_value =
        serde_json::from_str::<Value>(current).expect("embedded current schema is valid JSON");
    let legacy_value =
        serde_json::from_str::<Value>(legacy).expect("embedded legacy schema is valid JSON");
    match local {
        Ok(local) if local == current_value => Some(current),
        Ok(local) if local == legacy_value => Some(legacy),
        Ok(_) => {
            report.errors.push(format!(
                "{label} differs from the supported embedded schemas"
            ));
            None
        }
        Err(error) => {
            report
                .errors
                .push(format!("{label} is not valid JSON: {error}"));
            None
        }
    }
}

fn schema_matches(content: &str, canonical: &str) -> bool {
    serde_json::from_str::<Value>(content).ok() == serde_json::from_str::<Value>(canonical).ok()
}

fn evidence_reference(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("ref").and_then(Value::as_str))
}

fn add_git_validation_warnings(root: &Path, data: &RepositoryData, report: &mut ValidationReport) {
    let event_evidence = data
        .events
        .iter()
        .filter_map(|event| event.get("evidence").and_then(Value::as_array))
        .flatten();
    let model_evidence = model_entries(&data.model)
        .into_iter()
        .filter_map(|entry| entry.get("evidence").and_then(Value::as_array))
        .flatten();
    let evidence: BTreeSet<String> = event_evidence
        .chain(model_evidence)
        .filter_map(evidence_reference)
        .filter_map(|item| item.strip_prefix("commit:"))
        .filter(|commit| !commit.is_empty())
        .map(str::to_owned)
        .collect();
    let inside_git = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !inside_git {
        if !evidence.is_empty() {
            report
                .warnings
                .push("Git is unavailable; commit evidence could not be verified".to_owned());
        }
        return;
    }
    let shallow = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-shallow-repository"])
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout == b"true\n");
    if shallow {
        report
            .warnings
            .push("Git history is shallow; commit evidence may be unavailable".to_owned());
    }
    for commit in evidence {
        let object = format!("{commit}^{{commit}}");
        let exists = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["cat-file", "-e", &object])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !exists {
            report.warnings.push(format!(
                "commit evidence '{commit}' could not be verified in the available Git history"
            ));
        }
    }
}

fn compile_schema(
    label: &str,
    content: &str,
    report: &mut ValidationReport,
) -> Option<jsonschema::Validator> {
    let schema = match serde_json::from_str::<Value>(content) {
        Ok(schema) => schema,
        Err(error) => {
            report
                .errors
                .push(format!("{label} is not valid JSON: {error}"));
            return None;
        }
    };
    match jsonschema::validator_for(&schema) {
        Ok(validator) => Some(validator),
        Err(error) => {
            report
                .errors
                .push(format!("{label} is not a valid JSON Schema: {error}"));
            None
        }
    }
}

fn collect_schema_errors(
    label: &str,
    validator: &jsonschema::Validator,
    instance: &Value,
    errors: &mut Vec<String>,
) {
    for error in validator.iter_errors(instance) {
        let location = error.instance_path().as_str();
        if location.is_empty() {
            errors.push(format!("{label}: {error}"));
        } else {
            errors.push(format!("{label} at {location}: {error}"));
        }
    }
}

fn validate_cross_records(model: &Value, events: &[Value], errors: &mut Vec<String>) {
    let sections = ["principles", "architecture", "behaviors", "constraints"];
    for section in sections {
        let mut ids = BTreeSet::new();
        if let Some(entries) = model.get(section).and_then(Value::as_array) {
            for entry in entries {
                if let Some(id) = entry.get("id").and_then(Value::as_str)
                    && !ids.insert(id)
                {
                    errors.push(format!("duplicate model entry ID '{id}' in {section}"));
                }
            }
        }
    }

    let mut event_types: BTreeMap<String, String> = BTreeMap::new();
    for event in events {
        if let Some(date) = event.get("date").and_then(Value::as_str)
            && Date::parse(date, &format_description!("[year]-[month]-[day]")).is_err()
        {
            errors.push(format!(
                "event date '{date}' is not a valid ISO calendar date"
            ));
        }
        if let (Some(date), Some(occurred_at)) = (
            event.get("date").and_then(Value::as_str),
            event.get("occurred_at").and_then(Value::as_str),
        ) && let (Ok(date), Ok(timestamp)) = (
            Date::parse(date, &format_description!("[year]-[month]-[day]")),
            OffsetDateTime::parse(occurred_at, &Rfc3339),
        ) && date != timestamp.date()
        {
            errors.push(format!(
                "event occurred_at '{occurred_at}' does not fall on its date '{date}'"
            ));
        }
        if let (Some(id), Some(kind)) = (
            event.get("id").and_then(Value::as_str),
            event.get("type").and_then(Value::as_str),
        ) && event_types.insert(id.to_owned(), kind.to_owned()).is_some()
        {
            errors.push(format!("duplicate event ID '{id}'"));
        }
    }

    for section in sections {
        if let Some(entries) = model.get(section).and_then(Value::as_array) {
            for entry in entries {
                let entry_id = entry
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                if let Some(references) = entry.get("related_events").and_then(Value::as_array) {
                    for reference in references.iter().filter_map(Value::as_str) {
                        if !event_types.contains_key(reference) {
                            errors.push(format!(
                                "model entry '{entry_id}' references missing event '{reference}'"
                            ));
                        }
                    }
                }
                if let Some(relations) = entry.get("event_relations").and_then(Value::as_array) {
                    for reference in relations
                        .iter()
                        .filter_map(|relation| relation.get("event").and_then(Value::as_str))
                    {
                        if !event_types.contains_key(reference) {
                            errors.push(format!(
                                "model entry '{entry_id}' references missing event '{reference}'"
                            ));
                        }
                    }
                }
            }
        }
    }

    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for event in events {
        let Some(id) = event.get("id").and_then(Value::as_str) else {
            continue;
        };
        let mut supersedes: Vec<String> = event
            .get("supersedes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        if let Some(relations) = event.get("relations").and_then(Value::as_array) {
            for relation in relations {
                let Some(target) = relation.get("event").and_then(Value::as_str) else {
                    continue;
                };
                let kind = relation
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("related");
                if target == id {
                    errors.push(format!("event '{id}' cannot relate to itself"));
                } else if !event_types.contains_key(target) {
                    errors.push(format!(
                        "event '{id}' relation '{kind}' references missing event '{target}'"
                    ));
                }
                if matches!(kind, "supersedes" | "partially_supersedes") {
                    if event.get("type").and_then(Value::as_str) != Some("decision")
                        || event_types.get(target).map(String::as_str) != Some("decision")
                    {
                        errors.push(format!(
                            "event '{id}' relation '{kind}' must connect two decisions"
                        ));
                    }
                    supersedes.push(target.to_owned());
                }
            }
        }
        if event.get("type").and_then(Value::as_str) != Some("decision") {
            continue;
        }
        for target in &supersedes {
            if target == id {
                errors.push(format!("decision '{id}' cannot supersede itself"));
            } else if event_types.get(target).map(String::as_str) != Some("decision") {
                errors.push(format!(
                    "decision '{id}' supersedes missing or non-decision event '{target}'"
                ));
            }
        }
        graph.insert(id.to_owned(), supersedes);
    }

    let mut permanent = BTreeSet::new();
    let mut temporary = BTreeSet::new();
    for node in graph.keys() {
        detect_cycle(node, &graph, &mut temporary, &mut permanent, errors);
    }
}

fn detect_cycle(
    node: &str,
    graph: &BTreeMap<String, Vec<String>>,
    temporary: &mut BTreeSet<String>,
    permanent: &mut BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    if permanent.contains(node) {
        return;
    }
    if !temporary.insert(node.to_owned()) {
        errors.push(format!("decision supersession cycle includes '{node}'"));
        return;
    }
    if let Some(targets) = graph.get(node) {
        for target in targets {
            if graph.contains_key(target) {
                detect_cycle(target, graph, temporary, permanent, errors);
            }
        }
    }
    temporary.remove(node);
    permanent.insert(node.to_owned());
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<Vec<String>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has an invalid file name", path.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_nanos();
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    let before_commit = (|| -> Result<Vec<String>, String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create temporary file: {error}"))?;
        file.write_all(content)
            .map_err(|error| format!("cannot write temporary file: {error}"))?;
        let warnings = if path.exists() {
            copy_metadata(path, &temporary)?
        } else {
            Vec::new()
        };
        file.sync_all()
            .map_err(|error| format!("cannot sync temporary file: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("cannot commit temporary file: {error}"))?;
        Ok(warnings)
    })();
    let mut warnings = match before_commit {
        Ok(warnings) => warnings,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "cannot write {} atomically: {error}",
                path.display()
            ));
        }
    };
    if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
        warnings.push(format!(
            "{} was committed, but its parent directory could not be synced: {error}",
            path.display()
        ));
    }
    Ok(warnings)
}

fn copy_metadata(source: &Path, destination: &Path) -> Result<Vec<String>, String> {
    let metadata = fs::metadata(source)
        .map_err(|error| format!("cannot read metadata for {}: {error}", source.display()))?;
    fs::set_permissions(destination, metadata.permissions()).map_err(|error| {
        format!(
            "cannot preserve permissions from {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    let mut warnings = Vec::new();
    #[cfg(unix)]
    match xattr::list(source) {
        Ok(attributes) => {
            for name in attributes {
                match xattr::get(source, &name) {
                    Ok(Some(value)) => {
                        if let Err(error) = xattr::set(destination, &name, &value) {
                            warnings.push(format!(
                                "could not preserve extended attribute {:?} on {}: {error}",
                                name,
                                destination.display()
                            ));
                        }
                    }
                    Ok(None) => {}
                    Err(error) => warnings.push(format!(
                        "could not read extended attribute {:?} from {}: {error}",
                        name,
                        source.display()
                    )),
                }
            }
        }
        Err(error) => warnings.push(format!(
            "could not enumerate extended attributes on {}: {error}",
            source.display()
        )),
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::TempDir;

    fn initialized() -> TempDir {
        let directory = TempDir::new().expect("temporary directory");
        initialize(directory.path(), false).expect("initialize fixture");
        directory
    }

    fn write(path: &Path, content: &str) {
        fs::write(path, content).expect("write fixture");
    }

    fn validate(directory: &TempDir) -> ValidationReport {
        validate_repository(directory.path()).expect("validation runs")
    }

    fn fixture(name: &str) -> TempDir {
        let directory = initialized();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
            .join(".project-context");
        for file in ["model.yaml", "events.jsonl"] {
            fs::copy(
                source.join(file),
                directory.path().join(".project-context").join(file),
            )
            .expect("copy fixture file");
        }
        write(
            &directory
                .path()
                .join(".project-context/schemas/model.schema.json"),
            MODEL_SCHEMA_V1,
        );
        write(
            &directory
                .path()
                .join(".project-context/schemas/event.schema.json"),
            EVENT_SCHEMA_V1,
        );
        directory
    }

    fn decision(subject: &str) -> DecisionInput {
        DecisionInput {
            subject: subject.to_owned(),
            decision: "Keep the boundary.".to_owned(),
            reason: "It preserves ownership.".to_owned(),
            id: None,
            date: Some("2026-07-17".to_owned()),
            occurred_at: None,
            rejected: Vec::new(),
            supersedes: Vec::new(),
            conditions: None,
            evidence: Vec::new(),
            evidence_details: Vec::new(),
            relations: Vec::new(),
        }
    }

    fn attempt(result: &str) -> AttemptInput {
        AttemptInput {
            subject: "callback delivery".to_owned(),
            approach: "Try the platform callback.".to_owned(),
            result: result.to_owned(),
            finding: "The callback was not delivered.".to_owned(),
            id: None,
            date: Some("2026-07-17".to_owned()),
            occurred_at: None,
            conditions: None,
            evidence: Vec::new(),
            evidence_details: Vec::new(),
            relations: Vec::new(),
        }
    }

    #[test]
    fn init_preserves_the_project_installed_skill() {
        let directory = TempDir::new().expect("temporary directory");
        let skill = directory
            .path()
            .join(".agents/skills/project-context/SKILL.md");
        fs::create_dir_all(skill.parent().expect("skill parent")).expect("create skill directory");
        write(&skill, "installed skill\n");
        initialize(directory.path(), false).expect("initialize fixture");
        assert!(validate(&directory).valid);
        assert_eq!(
            fs::read_to_string(skill).expect("read skill"),
            "installed skill\n"
        );
    }

    #[test]
    fn generic_project_directory_is_ignored_and_preserved() {
        let directory = TempDir::new().expect("temporary directory");
        let generic = directory.path().join(".project");
        fs::create_dir(&generic).expect("generic project directory");
        write(&generic.join("owner.txt"), "another tool\n");
        let nested = directory.path().join("src/nested");
        fs::create_dir_all(&nested).expect("nested directory");
        assert_eq!(discover_root(&nested), None);

        initialize(directory.path(), false).expect("initialize project context");
        assert_eq!(
            fs::read_to_string(generic.join("owner.txt")).expect("generic owner file"),
            "another tool\n"
        );
        assert_eq!(discover_root(&nested), Some(directory.path().to_path_buf()));
        assert!(
            directory
                .path()
                .join(".project-context/model.yaml")
                .is_file()
        );
    }

    #[test]
    fn fixtures_cover_valid_duplicate_and_cycle_stores() {
        assert!(validate(&fixture("valid")).valid);
        assert!(
            validate(&fixture("duplicate-id"))
                .errors
                .iter()
                .any(|error| error.contains("duplicate event ID"))
        );
        assert!(
            validate(&fixture("supersession-cycle"))
                .errors
                .iter()
                .any(|error| error.contains("supersession cycle"))
        );
    }

    #[test]
    fn init_refuses_overwrite_and_force_replaces_files() {
        let directory = initialized();
        write(
            &directory.path().join(".project-context/model.yaml"),
            "existing content\n",
        );
        assert!(initialize(directory.path(), false).is_err());
        assert_eq!(
            fs::read_to_string(directory.path().join(".project-context/model.yaml"))
                .expect("read model"),
            "existing content\n"
        );
        initialize(directory.path(), true).expect("force initialization");
        assert!(validate(&directory).valid);
    }

    #[cfg(unix)]
    #[test]
    fn init_rejects_dangling_symlinks_even_with_force() {
        for force in [false, true] {
            let directory = TempDir::new().expect("temporary directory");
            fs::create_dir_all(directory.path().join(".project-context"))
                .expect("project directory");
            symlink(
                directory.path().join("missing-model"),
                directory.path().join(".project-context/model.yaml"),
            )
            .expect("dangling symlink");
            let error = initialize(directory.path(), force).expect_err("symlink is refused");
            assert!(error.contains("symbolic link"));
        }
    }

    #[test]
    fn startup_rolls_back_an_incomplete_initialization_transaction() {
        let directory = initialized();
        let model = directory.path().join(".project-context/model.yaml");
        let original = fs::read_to_string(&model).expect("original model");
        let transaction = directory.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir_all(transaction.join("backup")).expect("backup directory");
        fs::create_dir_all(transaction.join("staged/schemas")).expect("staging directory");
        fs::rename(&model, transaction.join("backup/model.yaml")).expect("backup model");
        write(&model, "schema_version: 1\nproject: {id: interrupted}\n");
        for relative in INIT_PATHS.iter().skip(1) {
            let staged = transaction
                .join("staged")
                .join(relative.trim_start_matches(".project-context/"));
            if let Some(parent) = staged.parent() {
                fs::create_dir_all(parent).expect("staged parent");
            }
            write(&staged, "staged\n");
        }

        let report = validate_repository(directory.path()).expect("recover then validate");
        assert!(report.valid, "{:?}", report.errors);
        assert_eq!(fs::read_to_string(model).expect("restored model"), original);
        assert!(!transaction.exists());
    }

    #[test]
    fn startup_rolls_back_only_reconstruction_targets_for_a_typed_transaction() {
        let directory = initialized();
        let project = directory.path().join(".project-context");
        let original_model = fs::read(project.join("model.yaml")).expect("original model");
        let original_events = fs::read(project.join("events.jsonl")).expect("original events");
        let original_schema =
            fs::read(project.join("schemas/model.schema.json")).expect("original schema");
        let transaction = directory.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir_all(transaction.join("backup")).expect("backup directory");
        write(&transaction.join("kind"), "reconstruction\n");
        fs::rename(
            project.join("model.yaml"),
            transaction.join("backup/model.yaml"),
        )
        .expect("backup model");
        fs::rename(
            project.join("events.jsonl"),
            transaction.join("backup/events.jsonl"),
        )
        .expect("backup events");
        write(&project.join("model.yaml"), "interrupted model\n");
        write(&project.join("events.jsonl"), "interrupted events\n");

        let report = validate_repository(directory.path()).expect("recover then validate");
        assert!(report.valid, "{:?}", report.errors);
        assert_eq!(
            fs::read(project.join("model.yaml")).unwrap(),
            original_model
        );
        assert_eq!(
            fs::read(project.join("events.jsonl")).unwrap(),
            original_events
        );
        assert_eq!(
            fs::read(project.join("schemas/model.schema.json")).unwrap(),
            original_schema
        );
        assert!(!transaction.exists());
    }

    #[test]
    fn startup_preserves_targets_when_a_typed_transaction_was_not_prepared() {
        let directory = initialized();
        let project = directory.path().join(".project-context");
        let original_model = fs::read(project.join("model.yaml")).expect("original model");
        let original_events = fs::read(project.join("events.jsonl")).expect("original events");
        let transaction = directory.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir_all(transaction.join("staged")).expect("staging directory");
        fs::create_dir_all(transaction.join("backup")).expect("backup directory");
        write(&transaction.join("kind"), "reconstruction\n");

        let report = validate_repository(directory.path()).expect("recover then validate");
        assert!(report.valid, "{:?}", report.errors);
        assert_eq!(
            fs::read(project.join("model.yaml")).unwrap(),
            original_model
        );
        assert_eq!(
            fs::read(project.join("events.jsonl")).unwrap(),
            original_events
        );
        assert!(!transaction.exists());
    }

    #[test]
    fn startup_rejects_an_unknown_transaction_kind_without_mutation() {
        let directory = initialized();
        let project = directory.path().join(".project-context");
        let original_model = fs::read(project.join("model.yaml")).expect("original model");
        let transaction = directory.path().join(TRANSACTION_DIRECTORY);
        fs::create_dir_all(&transaction).expect("transaction directory");
        write(&transaction.join("kind"), "future-kind\n");
        write(&transaction.join("committed"), "committed\n");

        let error = validate_repository(directory.path()).expect_err("unknown kind is refused");
        assert!(
            matches!(error, StoreError::Environment(message) if message.contains("unknown project-context transaction kind"))
        );
        assert_eq!(
            fs::read(project.join("model.yaml")).unwrap(),
            original_model
        );
        assert!(transaction.exists());
    }

    #[test]
    fn validation_rejects_schema_tampering_and_recursive_duplicate_keys() {
        let schema_directory = initialized();
        write(
            &schema_directory
                .path()
                .join(".project-context/schemas/event.schema.json"),
            "{}\n",
        );
        assert!(
            validate(&schema_directory)
                .errors
                .iter()
                .any(|error| error.contains("supported embedded schemas"))
        );

        let duplicate_directory = initialized();
        write(
            &duplicate_directory
                .path()
                .join(".project-context/events.jsonl"),
            concat!(
                "{\"schema_version\":2,\"type\":\"decision\",\"id\":\"D-1\",",
                "\"date\":\"2026-07-17\",\"subject\":\"x\",",
                "\"decision\":\"first\",\"decision\":\"second\",\"reason\":\"x\"}\n"
            ),
        );
        assert!(
            validate(&duplicate_directory)
                .errors
                .iter()
                .any(|error| error.contains("duplicate object key 'decision'"))
        );
    }

    #[test]
    fn generated_event_ids_have_no_integer_ceiling() {
        let directory = initialized();
        let existing = concat!(
            "{\"schema_version\":2,\"type\":\"decision\",",
            "\"id\":\"D-184467440737095516160000\",\"date\":\"2026-07-17\",",
            "\"subject\":\"old\",\"decision\":\"old\",\"reason\":\"old\"}\n"
        );
        write(
            &directory.path().join(".project-context/events.jsonl"),
            existing,
        );
        let event = add_decision(directory.path(), decision("new")).expect("large next ID");
        assert_eq!(event["id"], "D-184467440737095516160001");
    }

    #[cfg(unix)]
    #[test]
    fn mutation_preserves_mode_and_validation_uses_a_read_only_lock() {
        let directory = initialized();
        let events = directory.path().join(".project-context/events.jsonl");
        fs::set_permissions(&events, fs::Permissions::from_mode(0o640)).expect("event mode");
        add_attempt(directory.path(), attempt("failed")).expect("append event");
        assert_eq!(
            fs::metadata(&events)
                .expect("event metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );

        let lock = directory.path().join(".project-context/.lock");
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o400)).expect("read-only lock");
        assert!(validate(&directory).valid);
    }

    #[test]
    fn startup_removes_only_recognized_stale_event_temporary_files() {
        let directory = initialized();
        let stale = directory
            .path()
            .join(".project-context/.events.jsonl.tmp-123-456");
        let unrelated = directory.path().join(".project-context/.unrelated.tmp-123");
        write(&stale, "stale");
        write(&unrelated, "keep");
        add_attempt(directory.path(), attempt("failed")).expect("mutation");
        assert!(!stale.exists());
        assert!(unrelated.exists());
    }

    #[cfg(unix)]
    #[test]
    fn post_rename_directory_sync_failure_is_a_committed_warning() {
        let directory = initialized();
        let project = directory.path().join(".project-context");
        let events = project.join("events.jsonl");
        fs::set_permissions(&project, fs::Permissions::from_mode(0o300))
            .expect("restrict project directory");
        let result = atomic_write(&events, b"committed\n");
        fs::set_permissions(&project, fs::Permissions::from_mode(0o700))
            .expect("restore project directory");
        let warnings = result.expect("rename remains successful");
        assert_eq!(
            fs::read_to_string(events).expect("committed content"),
            "committed\n"
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("was committed"))
        );
    }

    #[test]
    fn validation_rejects_unknown_fields_versions_ids_and_dates() {
        let cases = [
            json!({"schema_version":2,"type":"gap","id":"G-1","date":"2026-07-17","subject":"x"}),
            json!({"schema_version":2,"type":"decision","id":"A-1","date":"2026-07-17","subject":"x","decision":"x","reason":"x"}),
            json!({"schema_version":3,"type":"attempt","id":"A-1","date":"2026-07-17","subject":"x","approach":"x","result":"failed","finding":"x"}),
            json!({"schema_version":2,"type":"attempt","id":"A-1","date":"2026-07-17","subject":"x","approach":"x","result":"failed","finding":"x","unknown":true}),
            json!({"schema_version":2,"type":"attempt","id":"A-1","date":"2026-99-99","subject":"x","approach":"x","result":"failed","finding":"x"}),
            json!({"schema_version":2,"type":"attempt","id":"A-1","date":"2026-07-17","occurred_at":"2026-07-18T00:00:00+09:00","subject":"x","approach":"x","result":"failed","finding":"x"}),
        ];
        for event in cases {
            let directory = initialized();
            write(
                &directory.path().join(".project-context/events.jsonl"),
                &format!("{event}\n"),
            );
            assert!(
                !validate(&directory).valid,
                "event should be invalid: {event}"
            );
        }
    }

    #[test]
    fn reconstruction_merge_preserves_authoritative_base_model_fields() {
        let base = json!({
            "project": {"id": "fixture", "description": "Base description."},
            "principles": [{"id": "base", "statement": "Preserve this."}],
            "architecture": [],
            "behaviors": [],
            "constraints": [],
            "operations": {"build": ["cargo build"], "test": [], "lint": [], "format": []},
            "extensions": {"owner": "base"}
        });
        let proposed = json!({
            "project": {"id": "changed", "description": "Changed."},
            "principles": [],
            "architecture": [],
            "behaviors": [],
            "constraints": [],
            "operations": {"build": ["make"], "test": [], "lint": [], "format": []},
            "extensions": {"owner": "changed"}
        });
        let errors = validate_model_merge(&base, &proposed);
        for expected in [
            "project.id",
            "project.description",
            "principles entry 'base'",
            "operations.build",
            "extensions",
        ] {
            assert!(
                errors.iter().any(|error| error.contains(expected)),
                "missing merge error for {expected}: {errors:?}"
            );
        }

        let duplicate_statement = json!({
            "project": {"id": "fixture", "description": "Base description."},
            "principles": [{"id": "base", "statement": "Preserve this."}],
            "architecture": [{"id": "duplicate", "statement": " Preserve   this. "}],
            "behaviors": [],
            "constraints": [],
            "operations": {"build": ["cargo build"], "test": [], "lint": [], "format": []},
            "extensions": {"owner": "base"}
        });
        assert!(
            validate_model_merge(&base, &duplicate_statement)
                .iter()
                .any(|error| error.contains("statement conflicts"))
        );
    }

    #[test]
    fn reconstruction_candidate_keys_cannot_collide_or_cycle() {
        assert!(valid_candidate_key("candidate:decision-one"));
        assert!(!valid_candidate_key("D-0001"));
        assert!(!valid_candidate_key("A-0001"));
        let remap = BTreeMap::from([
            ("candidate:first".to_owned(), "candidate:second".to_owned()),
            ("candidate:second".to_owned(), "candidate:first".to_owned()),
        ]);
        assert!(resolve_reference_map(&remap).is_err());
    }

    #[test]
    fn validation_rejects_duplicate_model_ids_and_missing_references() {
        let directory = initialized();
        let model = MODEL_TEMPLATE.replace(
            "principles: []",
            "principles:\n  - id: same\n    statement: One.\n  - id: same\n    statement: Two.\n    related_events:\n      - D-404",
        );
        write(
            &directory.path().join(".project-context/model.yaml"),
            &model,
        );
        let report = validate(&directory);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("duplicate model entry ID"))
        );
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("references missing event"))
        );
    }

    #[test]
    fn validation_rejects_missing_self_and_cyclic_supersession() {
        let cases = [
            "{\"schema_version\":1,\"type\":\"decision\",\"id\":\"D-1\",\"date\":\"2026-07-17\",\"subject\":\"x\",\"decision\":\"x\",\"reason\":\"x\",\"supersedes\":[\"D-9\"]}\n",
            "{\"schema_version\":1,\"type\":\"decision\",\"id\":\"D-1\",\"date\":\"2026-07-17\",\"subject\":\"x\",\"decision\":\"x\",\"reason\":\"x\",\"supersedes\":[\"D-1\"]}\n",
            concat!(
                "{\"schema_version\":1,\"type\":\"decision\",\"id\":\"D-1\",\"date\":\"2026-07-17\",\"subject\":\"x\",\"decision\":\"x\",\"reason\":\"x\",\"supersedes\":[\"D-2\"]}\n",
                "{\"schema_version\":1,\"type\":\"decision\",\"id\":\"D-2\",\"date\":\"2026-07-17\",\"subject\":\"x\",\"decision\":\"x\",\"reason\":\"x\",\"supersedes\":[\"D-1\"]}\n"
            ),
        ];
        for events in cases {
            let directory = initialized();
            write(
                &directory.path().join(".project-context/events.jsonl"),
                events,
            );
            assert!(!validate(&directory).valid);
        }
    }

    #[test]
    fn add_decision_allocates_id_and_preserves_existing_lines() {
        let directory = initialized();
        let existing = "{\"schema_version\":2,\"type\":\"decision\",\"id\":\"D-9\",\"date\":\"2026-07-16\",\"subject\":\"old\",\"decision\":\"old\",\"reason\":\"old\"}\n";
        write(
            &directory.path().join(".project-context/events.jsonl"),
            existing,
        );
        let mut input = decision("process boundary");
        input.rejected = vec!["Put state in both processes.".to_owned()];
        input.evidence = vec!["file:src/boundary.rs".to_owned()];
        let event = add_decision(directory.path(), input).expect("append decision");
        assert_eq!(event["id"], "D-0010");
        let stored = fs::read_to_string(directory.path().join(".project-context/events.jsonl"))
            .expect("read events");
        assert!(stored.starts_with(existing));
        assert_eq!(stored.lines().count(), 2);
        assert!(validate(&directory).valid);
    }

    #[test]
    fn add_attempt_supports_explicit_id_date_and_inconclusive_result() {
        let directory = initialized();
        write(
            &directory.path().join(".project-context/events.jsonl"),
            "{\"schema_version\":2,\"type\":\"decision\",\"id\":\"D-1\",\"date\":\"2026-06-27\",\"subject\":\"later\",\"decision\":\"later\",\"reason\":\"later\"}\n",
        );
        let mut input = attempt("inconclusive");
        input.id = Some("A-42".to_owned());
        input.date = Some("2026-06-26".to_owned());
        input.conditions = Some("Current platform permissions.".to_owned());
        let event = add_attempt(directory.path(), input).expect("append attempt");
        assert_eq!(event["id"], "A-42");
        assert_eq!(event["result"], "inconclusive");
        let stored = fs::read_to_string(directory.path().join(".project-context/events.jsonl"))
            .expect("read events");
        let subjects: Vec<String> = stored
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("event JSON")["subject"]
                    .as_str()
                    .expect("subject")
                    .to_owned()
            })
            .collect();
        assert_eq!(subjects, ["callback delivery", "later"]);
        assert!(validate(&directory).valid);
    }

    #[test]
    fn invalid_proposed_event_does_not_change_store() {
        let directory = initialized();
        let path = directory.path().join(".project-context/events.jsonl");
        let before = fs::read_to_string(&path).expect("read events");
        let error = add_attempt(directory.path(), attempt("unknown"));
        assert!(matches!(error, Err(StoreError::Invalid(_))));
        assert_eq!(fs::read_to_string(path).expect("read events"), before);
    }

    #[test]
    fn invalid_current_store_blocks_mutation_without_changes() {
        let directory = initialized();
        let path = directory.path().join(".project-context/events.jsonl");
        write(&path, "not json\n");
        let before = fs::read_to_string(&path).expect("read events");
        assert!(matches!(
            add_decision(directory.path(), decision("x")),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(fs::read_to_string(path).expect("read events"), before);
    }

    #[test]
    fn concurrent_mutation_fails_while_lock_is_held() {
        let directory = initialized();
        let lock = open_lock(directory.path(), true).expect("hold lock");
        assert!(matches!(
            add_decision(directory.path(), decision("x")),
            Err(StoreError::Environment(_))
        ));
        let _ = lock.unlock();
    }
}
