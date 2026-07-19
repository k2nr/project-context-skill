use crate::store::{self, StoreError};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const START_MARKER: &str = "<!-- project-context:managed:start -->";
const END_MARKER: &str = "<!-- project-context:managed:end -->";

#[derive(Debug, Default, Serialize)]
pub struct DoctorReport {
    pub ready: bool,
    pub checks: BTreeMap<String, String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn inspect_installation(
    root: &Path,
    allowed_empty_operations: &BTreeSet<&str>,
) -> Result<DoctorReport, String> {
    let mut report = DoctorReport::default();
    match store::validate_repository(root) {
        Ok(validation) => {
            report.errors.extend(validation.errors);
            report.warnings.extend(validation.warnings);
        }
        Err(StoreError::Invalid(validation)) => report.errors.extend(validation.errors),
        Err(StoreError::Conflict(error)) => return Err(error),
        Err(StoreError::Environment(error)) => return Err(error),
    }
    if report.errors.is_empty() {
        let data = store::load_valid_repository(root).map_err(store_error_message)?;
        inspect_model(&data.model, allowed_empty_operations, &mut report);
    }
    inspect_skill(root, &mut report);
    inspect_reconstruction_skill(root, &mut report);
    inspect_agents(root, &mut report);
    inspect_tools(&mut report);
    report.errors.sort();
    report.errors.dedup();
    report.warnings.sort();
    report.warnings.dedup();
    report.ready = report.errors.is_empty();
    Ok(report)
}

fn store_error_message(error: StoreError) -> String {
    match error {
        StoreError::Invalid(report) => report.errors.join("; "),
        StoreError::Conflict(message) => message,
        StoreError::Environment(message) => message,
    }
}

fn inspect_model(
    model: &Value,
    allowed_empty_operations: &BTreeSet<&str>,
    report: &mut DoctorReport,
) {
    let description = model
        .pointer("/project/description")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if description.is_empty() {
        report
            .errors
            .push("project.description is required for an installation".to_owned());
    } else {
        report
            .checks
            .insert("model.description".to_owned(), "present".to_owned());
    }
    for operation in ["build", "test", "lint", "format"] {
        let populated = model
            .pointer(&format!("/operations/{operation}"))
            .and_then(Value::as_array)
            .is_some_and(|commands| !commands.is_empty());
        let status = if populated {
            "populated"
        } else if allowed_empty_operations.contains(operation) {
            "acknowledged empty"
        } else {
            report.errors.push(format!(
                "operations.{operation} is empty; add a command or pass --allow-empty {operation}"
            ));
            "empty"
        };
        report
            .checks
            .insert(format!("model.operations.{operation}"), status.to_owned());
    }
}

fn inspect_skill(root: &Path, report: &mut DoctorReport) {
    let errors_before = report.errors.len();
    let skill = root.join(".agents/skills/project-context");
    let required_files = [
        "SKILL.md",
        "LICENSE",
        "agents/openai.yaml",
        "assets/init/event.schema.json",
        "assets/init/model.schema.json",
        "assets/init/model.yaml",
        "assets/install/AGENTS.fragment.md",
        "bin/project-context",
    ];
    for relative in required_files {
        if !skill.join(relative).is_file() {
            report.errors.push(format!(
                "installed skill file is missing or invalid: .agents/skills/project-context/{relative}"
            ));
        }
    }
    for relative in ["agents", "assets", "assets/init", "assets/install", "bin"] {
        if !skill.join(relative).is_dir() {
            report.errors.push(format!(
                "installed skill directory is missing or invalid: .agents/skills/project-context/{relative}"
            ));
        }
    }
    let expected = [
        "LICENSE",
        "SKILL.md",
        "agents/openai.yaml",
        "assets/init/event.schema.json",
        "assets/init/model.schema.json",
        "assets/init/model.yaml",
        "assets/install/AGENTS.fragment.md",
        "bin/project-context",
    ];
    if let Err(error) = inspect_tree(&skill, &skill, &expected, report) {
        report.errors.push(error);
    }
    let launcher = skill.join("bin/project-context");
    #[cfg(unix)]
    if launcher.is_file() {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(&launcher) {
            Ok(metadata) if metadata.permissions().mode() & 0o111 != 0 => {}
            Ok(_) => report
                .errors
                .push("installed project-context launcher is not executable".to_owned()),
            Err(error) => report
                .errors
                .push(format!("cannot inspect installed launcher: {error}")),
        }
    }
    if launcher.is_file() {
        match fs::read_to_string(&launcher) {
            Ok(content)
                if content.lines().any(|line| {
                    line == format!("PROJECT_CONTEXT_VERSION=\"{}\"", env!("CARGO_PKG_VERSION"))
                }) => {}
            Ok(_) => report.errors.push(format!(
                "installed launcher version does not match CLI {}",
                env!("CARGO_PKG_VERSION")
            )),
            Err(error) => report
                .errors
                .push(format!("cannot read installed launcher: {error}")),
        }
        let syntax = Command::new("sh")
            .arg("-n")
            .arg(&launcher)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !syntax.is_ok_and(|status| status.success()) {
            report
                .errors
                .push("installed project-context launcher failed sh -n".to_owned());
        }
    }
    if report.errors.len() == errors_before {
        report
            .checks
            .insert("skill.package".to_owned(), "valid".to_owned());
    }
}

fn inspect_reconstruction_skill(root: &Path, report: &mut DoctorReport) {
    let errors_before = report.errors.len();
    let skill = root.join(".agents/skills/reconstruct-project-context");
    let expected = [
        "LICENSE",
        "SKILL.md",
        "agents/openai.yaml",
        "references/qualification.md",
        "references/sources.md",
        "scripts/inventory_local_history.py",
    ];
    for relative in expected {
        if !skill.join(relative).is_file() {
            report.errors.push(format!(
                "installed skill file is missing or invalid: .agents/skills/reconstruct-project-context/{relative}"
            ));
        }
    }
    for relative in ["agents", "references", "scripts"] {
        if !skill.join(relative).is_dir() {
            report.errors.push(format!(
                "installed skill directory is missing or invalid: .agents/skills/reconstruct-project-context/{relative}"
            ));
        }
    }
    if let Err(error) = inspect_tree(&skill, &skill, &expected, report) {
        report.errors.push(error);
    }
    let inventory = skill.join("scripts/inventory_local_history.py");
    #[cfg(unix)]
    if inventory.is_file() {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(&inventory) {
            Ok(metadata) if metadata.permissions().mode() & 0o111 != 0 => {}
            Ok(_) => report
                .errors
                .push("installed reconstruction inventory script is not executable".to_owned()),
            Err(error) => report.errors.push(format!(
                "cannot inspect installed reconstruction inventory script: {error}"
            )),
        }
    }
    if report.errors.len() == errors_before {
        report.checks.insert(
            "skill.reconstruction_package".to_owned(),
            "valid".to_owned(),
        );
    }
}

fn inspect_tree(
    path: &Path,
    root: &Path,
    expected: &[&str],
    report: &mut DoctorReport,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect installed skill: {error}"))?;
    if metadata.file_type().is_symlink() {
        report.errors.push(format!(
            "installed skill contains symbolic link: {}",
            path.strip_prefix(root).unwrap_or(path).display()
        ));
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|error| format!("cannot read installed skill directory: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("cannot read installed skill entry: {error}"))?;
            inspect_tree(&entry.path(), root, expected, report)?;
        }
        return Ok(());
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    let forbidden_name = relative.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some("tests" | "test" | "target" | ".git" | ".github")
        )
    });
    let rust_artifact = relative.extension().and_then(|value| value.to_str()) == Some("rs")
        || matches!(
            relative.file_name().and_then(|value| value.to_str()),
            Some("Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml")
        );
    if forbidden_name || rust_artifact {
        report.errors.push(format!(
            "installed skill contains development artifact: {}",
            relative.display()
        ));
    }
    let relative_name = relative.to_string_lossy();
    if !expected.contains(&relative_name.as_ref()) {
        report.errors.push(format!(
            "installed skill contains an unexpected file: {}",
            relative.display()
        ));
    }
    Ok(())
}

fn inspect_agents(root: &Path, report: &mut DoctorReport) {
    let agents_path = root.join("AGENTS.md");
    let fragment_path =
        root.join(".agents/skills/project-context/assets/install/AGENTS.fragment.md");
    let agents = match fs::read_to_string(&agents_path) {
        Ok(content) => content,
        Err(error) => {
            report
                .errors
                .push(format!("cannot read AGENTS.md: {error}"));
            return;
        }
    };
    let fragment = match fs::read_to_string(&fragment_path) {
        Ok(content) => content,
        Err(_) => return,
    };
    if agents.matches(START_MARKER).count() != 1 || agents.matches(END_MARKER).count() != 1 {
        report.errors.push(
            "AGENTS.md must contain exactly one complete managed Project Context block".to_owned(),
        );
        return;
    }
    let Some(start) = agents.find(START_MARKER) else {
        return;
    };
    let Some(end_start) = agents.find(END_MARKER) else {
        return;
    };
    if end_start < start {
        report
            .errors
            .push("AGENTS.md managed Project Context markers are reversed".to_owned());
        return;
    }
    let end = end_start + END_MARKER.len();
    if &agents[start..end] != fragment.trim_end_matches('\n') {
        report.errors.push(
            "AGENTS.md managed Project Context block differs from the installed fragment"
                .to_owned(),
        );
        return;
    }
    report
        .checks
        .insert("agents.managed_block".to_owned(), "current".to_owned());
}

fn inspect_tools(report: &mut DoctorReport) {
    for tool in ["gh", "shellcheck"] {
        let available = command_available(tool);
        report.checks.insert(
            format!("tool.{tool}"),
            if available {
                "available"
            } else {
                "unavailable"
            }
            .to_owned(),
        );
        if !available {
            report
                .warnings
                .push(format!("optional tool is unavailable: {tool}"));
        }
    }
    let validator = standard_validator();
    report.checks.insert(
        "tool.standard_skill_validator".to_owned(),
        validator.clone(),
    );
    if validator != "available" {
        report.warnings.push(format!(
            "standard skill validator is unavailable: {validator}"
        ));
    }
}

fn command_available(command: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v \"$1\" >/dev/null 2>&1", "sh", command])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn standard_validator() -> String {
    let Some(path) = standard_validator_path() else {
        return "script not found".to_owned();
    };
    if !command_available("python3") {
        return "python3 not found".to_owned();
    }
    let yaml = Command::new("python3")
        .args(["-c", "import yaml"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !yaml.is_ok_and(|status| status.success()) {
        return "PyYAML not found".to_owned();
    }
    if path.is_file() {
        "available".to_owned()
    } else {
        "script not found".to_owned()
    }
}

fn standard_validator_path() -> Option<PathBuf> {
    let root = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?;
    Some(root.join("skills/.system/skill-creator/scripts/quick_validate.py"))
}
