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
use time::{Date, OffsetDateTime, macros::format_description};

const MODEL_TEMPLATE: &str = include_str!("../../project-context/assets/init/model.yaml");
const MODEL_SCHEMA: &str = include_str!("../../project-context/assets/init/model.schema.json");
const EVENT_SCHEMA: &str = include_str!("../../project-context/assets/init/event.schema.json");

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
    pub rejected: Vec<String>,
    pub supersedes: Vec<String>,
    pub conditions: Option<String>,
    pub evidence: Vec<String>,
}

pub struct AttemptInput {
    pub subject: String,
    pub approach: String,
    pub result: String,
    pub finding: String,
    pub id: Option<String>,
    pub date: Option<String>,
    pub conditions: Option<String>,
    pub evidence: Vec<String>,
}

struct RepositoryDocuments {
    model_schema: String,
    event_schema: String,
    model: String,
    events: String,
}

pub fn initialize(root: &Path, force: bool) -> Result<InitReport, String> {
    let project_directory = root.join(".project-context");
    fs::create_dir_all(&project_directory)
        .map_err(|error| format!("cannot create {}: {error}", project_directory.display()))?;
    let lock = open_lock(root, true)?;
    recover_init_transaction(root)?;
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
    transactional_replace(root, &contents)?;
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

fn transactional_replace(root: &Path, contents: &[Vec<u8>; 4]) -> Result<(), String> {
    let transaction = root.join(TRANSACTION_DIRECTORY);
    if transaction.exists() {
        return Err("an initialization transaction is already present".to_owned());
    }
    fs::create_dir(&transaction)
        .map_err(|error| format!("cannot create initialization transaction: {error}"))?;
    let staged_root = transaction.join("staged");
    let backup_root = transaction.join("backup");
    fs::create_dir(&staged_root)
        .map_err(|error| format!("cannot create transaction staging area: {error}"))?;
    fs::create_dir(&backup_root)
        .map_err(|error| format!("cannot create transaction backup area: {error}"))?;

    let result = (|| -> Result<(), String> {
        for (relative, content) in INIT_PATHS.iter().zip(contents) {
            let inner = relative.trim_start_matches(".project-context/");
            let staged = staged_root.join(inner);
            if let Some(parent) = staged.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            write_synced_file(&staged, content)?;
            let destination = root.join(relative);
            if destination.exists() {
                for warning in copy_metadata(&destination, &staged)? {
                    eprintln!("warning: {warning}");
                }
            }
        }
        File::open(&staged_root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("cannot sync transaction staging area: {error}"))?;

        for relative in INIT_PATHS {
            let inner = relative.trim_start_matches(".project-context/");
            let destination = root.join(relative);
            let staged = staged_root.join(inner);
            let backup = backup_root.join(inner);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            if destination.exists() {
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
            .map_err(|error| {
                format!("cannot sync .project-context after initialization: {error}")
            })?;
        Ok(())
    })();

    if let Err(error) = result {
        let rollback = rollback_init_transaction(root);
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error}; initialization rollback also failed: {rollback_error}"
            )),
        };
    }
    fs::remove_dir_all(&transaction)
        .map_err(|error| format!("cannot remove committed initialization transaction: {error}"))?;
    Ok(())
}

fn recover_init_transaction(root: &Path) -> Result<(), String> {
    let transaction = root.join(TRANSACTION_DIRECTORY);
    if !transaction.exists() {
        return Ok(());
    }
    if transaction.join("committed").is_file() {
        fs::remove_dir_all(&transaction).map_err(|error| {
            format!("cannot finish committed initialization transaction: {error}")
        })?;
        return Ok(());
    }
    rollback_init_transaction(root)
}

fn rollback_init_transaction(root: &Path) -> Result<(), String> {
    let transaction = root.join(TRANSACTION_DIRECTORY);
    let staged_root = transaction.join("staged");
    let backup_root = transaction.join("backup");
    for relative in INIT_PATHS.into_iter().rev() {
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
        } else if !staged.exists() && destination.exists() {
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
    recover_init_transaction(root).map_err(StoreError::Environment)?;
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

    let operation_updates = [
        ("build", input.build),
        ("test", input.test),
        ("lint", input.lint),
        ("format", input.format),
    ];
    if operation_updates
        .iter()
        .any(|(_, commands)| !commands.is_empty())
    {
        let operations = model_mapping
            .get_mut(serde_yaml_ng::Value::String("operations".to_owned()))
            .and_then(serde_yaml_ng::Value::as_mapping_mut)
            .ok_or_else(|| {
                StoreError::Environment("model.yaml has no operations mapping".to_owned())
            })?;
        for (name, commands) in operation_updates {
            if commands.is_empty() {
                continue;
            }
            operations.insert(
                serde_yaml_ng::Value::String(name.to_owned()),
                serde_yaml_ng::Value::Sequence(
                    commands
                        .into_iter()
                        .map(serde_yaml_ng::Value::String)
                        .collect(),
                ),
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

pub fn add_decision(root: &Path, input: DecisionInput) -> Result<Value, StoreError> {
    append_event(root, move |events| {
        let mut event = base_event(
            "decision",
            input.id,
            input.date,
            &input.subject,
            "D",
            events,
        )?;
        event.insert("decision".to_owned(), Value::String(input.decision));
        event.insert("reason".to_owned(), Value::String(input.reason));
        insert_array(&mut event, "rejected", input.rejected);
        insert_array(&mut event, "supersedes", input.supersedes);
        insert_optional(&mut event, "conditions", input.conditions);
        insert_array(&mut event, "evidence", input.evidence);
        Ok(Value::Object(event))
    })
}

pub fn add_attempt(root: &Path, input: AttemptInput) -> Result<Value, StoreError> {
    append_event(root, move |events| {
        let mut event = base_event("attempt", input.id, input.date, &input.subject, "A", events)?;
        event.insert("approach".to_owned(), Value::String(input.approach));
        event.insert("result".to_owned(), Value::String(input.result));
        event.insert("finding".to_owned(), Value::String(input.finding));
        insert_optional(&mut event, "conditions", input.conditions);
        insert_array(&mut event, "evidence", input.evidence);
        Ok(Value::Object(event))
    })
}

fn append_event<F>(root: &Path, create: F) -> Result<Value, StoreError>
where
    F: FnOnce(&[Value]) -> Result<Value, String>,
{
    let lock = open_lock(root, true).map_err(StoreError::Environment)?;
    recover_init_transaction(root).map_err(StoreError::Environment)?;
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

    let event = create(&data.events).map_err(StoreError::Environment)?;
    let serialized = serde_json::to_string(&event)
        .map_err(|error| StoreError::Environment(format!("cannot serialize event: {error}")))?;
    let mut proposed_events = documents.events;
    if !proposed_events.is_empty() && !proposed_events.ends_with('\n') {
        proposed_events.push('\n');
    }
    proposed_events.push_str(&serialized);
    proposed_events.push('\n');

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
    subject: &str,
    prefix: &str,
    events: &[Value],
) -> Result<Map<String, Value>, String> {
    let id = match requested_id {
        Some(id) => id,
        None => next_event_id(prefix, events)?,
    };
    let date = requested_date.unwrap_or_else(|| OffsetDateTime::now_utc().date().to_string());
    let mut event = Map::new();
    event.insert("schema_version".to_owned(), Value::from(1));
    event.insert("type".to_owned(), Value::String(kind.to_owned()));
    event.insert("id".to_owned(), Value::String(id));
    event.insert("date".to_owned(), Value::String(date));
    event.insert("subject".to_owned(), Value::String(subject.to_owned()));
    Ok(event)
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
    if !root.join(TRANSACTION_DIRECTORY).exists() {
        return Ok(());
    }
    let lock = open_lock(root, true).map_err(StoreError::Environment)?;
    let result = recover_init_transaction(root).map_err(StoreError::Environment);
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
    fs::read_to_string(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            let mut report = ValidationReport::default();
            report.errors.push(format!(
                "required project-context file is missing: {}",
                path.display()
            ));
            report.normalize();
            StoreError::Invalid(report)
        } else {
            StoreError::Environment(format!("cannot read {}: {error}", path.display()))
        }
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
    validate_canonical_schema_copy(
        "model.schema.json",
        model_schema_text,
        MODEL_SCHEMA,
        &mut report,
    );
    validate_canonical_schema_copy(
        "event.schema.json",
        event_schema_text,
        EVENT_SCHEMA,
        &mut report,
    );
    let model_validator = compile_schema("embedded model.schema.json", MODEL_SCHEMA, &mut report);
    let event_validator = compile_schema("embedded event.schema.json", EVENT_SCHEMA, &mut report);

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

fn validate_canonical_schema_copy(
    label: &str,
    content: &str,
    canonical: &str,
    report: &mut ValidationReport,
) {
    let local = serde_json::from_str::<Value>(content);
    let embedded = serde_json::from_str::<Value>(canonical).expect("embedded schema is valid JSON");
    match local {
        Ok(local) if local == embedded => {}
        Ok(_) => report.errors.push(format!(
            "{label} differs from the canonical embedded v1 schema"
        )),
        Err(error) => report
            .errors
            .push(format!("{label} is not valid JSON: {error}")),
    }
}

fn add_git_validation_warnings(root: &Path, data: &RepositoryData, report: &mut ValidationReport) {
    let evidence: BTreeSet<String> = data
        .events
        .iter()
        .filter_map(|event| event.get("evidence").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
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
            }
        }
    }

    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for event in events {
        if event.get("type").and_then(Value::as_str) != Some("decision") {
            continue;
        }
        let Some(id) = event.get("id").and_then(Value::as_str) else {
            continue;
        };
        let supersedes: Vec<String> = event
            .get("supersedes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
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
        directory
    }

    fn decision(subject: &str) -> DecisionInput {
        DecisionInput {
            subject: subject.to_owned(),
            decision: "Keep the boundary.".to_owned(),
            reason: "It preserves ownership.".to_owned(),
            id: None,
            date: Some("2026-07-17".to_owned()),
            rejected: Vec::new(),
            supersedes: Vec::new(),
            conditions: None,
            evidence: Vec::new(),
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
            conditions: None,
            evidence: Vec::new(),
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
                .any(|error| error.contains("canonical embedded v1 schema"))
        );

        let duplicate_directory = initialized();
        write(
            &duplicate_directory
                .path()
                .join(".project-context/events.jsonl"),
            concat!(
                "{\"schema_version\":1,\"type\":\"decision\",\"id\":\"D-1\",",
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
            "{\"schema_version\":1,\"type\":\"decision\",",
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
            json!({"schema_version":1,"type":"gap","id":"G-1","date":"2026-07-17","subject":"x"}),
            json!({"schema_version":1,"type":"decision","id":"A-1","date":"2026-07-17","subject":"x","decision":"x","reason":"x"}),
            json!({"schema_version":2,"type":"attempt","id":"A-1","date":"2026-07-17","subject":"x","approach":"x","result":"failed","finding":"x"}),
            json!({"schema_version":1,"type":"attempt","id":"A-1","date":"2026-07-17","subject":"x","approach":"x","result":"failed","finding":"x","unknown":true}),
            json!({"schema_version":1,"type":"attempt","id":"A-1","date":"2026-99-99","subject":"x","approach":"x","result":"failed","finding":"x"}),
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
        let existing = "{\"schema_version\":1,\"type\":\"decision\",\"id\":\"D-9\",\"date\":\"2026-07-16\",\"subject\":\"old\",\"decision\":\"old\",\"reason\":\"old\"}\n";
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
        let mut input = attempt("inconclusive");
        input.id = Some("A-42".to_owned());
        input.date = Some("2026-06-26".to_owned());
        input.conditions = Some("Current platform permissions.".to_owned());
        let event = add_attempt(directory.path(), input).expect("append attempt");
        assert_eq!(event["id"], "A-42");
        assert_eq!(event["result"], "inconclusive");
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
