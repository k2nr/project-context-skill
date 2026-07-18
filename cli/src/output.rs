use crate::context::ContextPacket;
use crate::doctor::DoctorReport;
use crate::store::{ConfigureReport, InitReport, ValidationReport};
use clap::ValueEnum;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    Yaml,
}

pub fn render_init(report: &InitReport, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => format!("initialized\n{}", report.files.join("\n")),
        _ => render_serializable(report, format),
    }
}

pub fn render_configure(report: &ConfigureReport, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => {
            if report.updated.is_empty() {
                "configuration unchanged".to_owned()
            } else {
                format!("configured\n{}", report.updated.join("\n"))
            }
        }
        _ => render_serializable(report, format),
    }
}

pub fn render_doctor(report: &DoctorReport, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => {
            let mut lines = vec![if report.ready {
                "installation ready".to_owned()
            } else {
                "installation incomplete".to_owned()
            }];
            lines.extend(
                report
                    .checks
                    .iter()
                    .map(|(check, status)| format!("check: {check}: {status}")),
            );
            lines.extend(report.errors.iter().map(|error| format!("error: {error}")));
            lines.extend(
                report
                    .warnings
                    .iter()
                    .map(|warning| format!("warning: {warning}")),
            );
            lines.join("\n")
        }
        _ => render_serializable(report, format),
    }
}

pub fn render_validation(report: &ValidationReport, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => {
            let mut lines = vec![if report.valid { "valid" } else { "invalid" }.to_owned()];
            lines.extend(report.errors.iter().map(|error| format!("error: {error}")));
            lines.extend(
                report
                    .warnings
                    .iter()
                    .map(|warning| format!("warning: {warning}")),
            );
            lines.join("\n")
        }
        _ => render_serializable(report, format),
    }
}

pub fn render_event(event: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => {
            let kind = event.get("type").and_then(Value::as_str).unwrap_or("event");
            let id = event.get("id").and_then(Value::as_str).unwrap_or("unknown");
            format!("created {kind} {id}\n{}", render_yaml(event))
        }
        _ => render_serializable(event, format),
    }
}

pub fn render_context(
    packet: &ContextPacket,
    format: OutputFormat,
    max_tokens: usize,
) -> Result<String, String> {
    let maximum = max_tokens.saturating_mul(4);
    let mut fitted = packet.clone();
    let mut omitted = 0_usize;
    loop {
        set_budget_warning(&mut fitted, omitted);
        let rendered = render_serializable(&fitted, format);
        if rendered.len() <= maximum {
            return Ok(rendered);
        }
        if remove_lowest_priority_item(&mut fitted) {
            omitted += 1;
            continue;
        }
        let minimum = rendered.len().div_ceil(4);
        return Err(format!(
            "--max-tokens is too small for the required context packet; use at least {minimum}"
        ));
    }
}

fn set_budget_warning(packet: &mut ContextPacket, omitted: usize) {
    packet
        .warnings
        .retain(|warning| !warning.contains("omitted by the --max-tokens budget"));
    if omitted > 0 {
        packet.warnings.push(format!(
            "{omitted} relevant record group(s) were omitted by the --max-tokens budget"
        ));
        packet.warnings.sort();
    }
}

fn remove_lowest_priority_item(packet: &mut ContextPacket) -> bool {
    if packet.paths.pop().is_some() {
        return true;
    }
    if packet.git.pop().is_some() {
        return true;
    }
    if packet.history.attempts.pop().is_some() {
        return true;
    }
    if let Some(group) = packet.decision_groups.pop() {
        packet.history.decisions.retain(|decision| {
            decision
                .get("id")
                .and_then(Value::as_str)
                .is_none_or(|id| !group.iter().any(|candidate| candidate == id))
        });
        return true;
    }
    let keys: Vec<String> = packet.current_intent.keys().cloned().collect();
    for key in keys.into_iter().rev() {
        let Some(value) = packet.current_intent.get_mut(&key) else {
            continue;
        };
        if let Some(entries) = value.as_array_mut()
            && entries.pop().is_some()
        {
            if entries.is_empty() {
                packet.current_intent.remove(&key);
            }
            return true;
        }
        packet.current_intent.remove(&key);
        return true;
    }
    false
}

fn render_serializable<T: Serialize>(value: &T, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string_pretty(value).expect("serializable output"),
        OutputFormat::Yaml | OutputFormat::Text => render_yaml(value),
    }
}

fn render_yaml<T: Serialize>(value: &T) -> String {
    serde_yaml_ng::to_string(value)
        .expect("serializable output")
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{GitEvidence, HistoryContext};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn validation_machine_output_is_stable() {
        let report = ValidationReport {
            valid: false,
            errors: vec!["first error".to_owned()],
            warnings: vec!["first warning".to_owned()],
        };
        assert_eq!(
            render_validation(&report, OutputFormat::Json),
            concat!(
                "{\n",
                "  \"valid\": false,\n",
                "  \"errors\": [\n",
                "    \"first error\"\n",
                "  ],\n",
                "  \"warnings\": [\n",
                "    \"first warning\"\n",
                "  ]\n",
                "}"
            )
        );
    }

    #[test]
    fn decision_and_attempt_outputs_are_stable() {
        let decision = json!({
            "schema_version": 1,
            "type": "decision",
            "id": "D-0001",
            "date": "2026-07-17",
            "subject": "ownership",
            "decision": "Keep ownership in the frontend.",
            "reason": "The frontend owns session state."
        });
        assert_eq!(
            render_event(&decision, OutputFormat::Text),
            concat!(
                "created decision D-0001\n",
                "date: 2026-07-17\n",
                "decision: Keep ownership in the frontend.\n",
                "id: D-0001\n",
                "reason: The frontend owns session state.\n",
                "schema_version: 1\n",
                "subject: ownership\n",
                "type: decision"
            )
        );

        let attempt = json!({
            "schema_version": 1,
            "type": "attempt",
            "id": "A-0001",
            "date": "2026-07-17",
            "subject": "backend ownership",
            "approach": "Move ownership to the backend.",
            "result": "failed",
            "finding": "State was duplicated."
        });
        assert_eq!(
            render_event(&attempt, OutputFormat::Yaml),
            concat!(
                "approach: Move ownership to the backend.\n",
                "date: 2026-07-17\n",
                "finding: State was duplicated.\n",
                "id: A-0001\n",
                "result: failed\n",
                "schema_version: 1\n",
                "subject: backend ownership\n",
                "type: attempt"
            )
        );
    }

    #[test]
    fn context_machine_output_is_stable() {
        let mut current_intent = BTreeMap::new();
        current_intent.insert(
            "constraints".to_owned(),
            json!([{"id":"stable-order","statement":"Keep ordering stable."}]),
        );
        let packet = ContextPacket {
            query: vec!["ordering".to_owned()],
            current_intent,
            history: HistoryContext {
                decisions: vec![json!({"id":"D-0001","subject":"ordering"})],
                attempts: Vec::new(),
            },
            git: vec![GitEvidence {
                commit: "abc123".to_owned(),
                subject: "Preserve ordering".to_owned(),
            }],
            paths: vec!["src/order.rs".to_owned()],
            warnings: Vec::new(),
            decision_groups: vec![vec!["D-0001".to_owned()]],
        };
        assert_eq!(
            render_context(&packet, OutputFormat::Json, 4000).expect("render context"),
            concat!(
                "{\n",
                "  \"query\": [\n",
                "    \"ordering\"\n",
                "  ],\n",
                "  \"current_intent\": {\n",
                "    \"constraints\": [\n",
                "      {\n",
                "        \"id\": \"stable-order\",\n",
                "        \"statement\": \"Keep ordering stable.\"\n",
                "      }\n",
                "    ]\n",
                "  },\n",
                "  \"history\": {\n",
                "    \"decisions\": [\n",
                "      {\n",
                "        \"id\": \"D-0001\",\n",
                "        \"subject\": \"ordering\"\n",
                "      }\n",
                "    ],\n",
                "    \"attempts\": []\n",
                "  },\n",
                "  \"git\": [\n",
                "    {\n",
                "      \"commit\": \"abc123\",\n",
                "      \"subject\": \"Preserve ordering\"\n",
                "    }\n",
                "  ],\n",
                "  \"paths\": [\n",
                "    \"src/order.rs\"\n",
                "  ],\n",
                "  \"warnings\": []\n",
                "}"
            )
        );
    }

    #[test]
    fn every_context_format_obeys_the_utf8_byte_budget() {
        let mut current_intent = BTreeMap::new();
        current_intent.insert(
            "project".to_owned(),
            json!({"id":"日本語-project","description":"日本語 context description"}),
        );
        let packet = ContextPacket {
            query: vec!["日本語".to_owned()],
            current_intent,
            history: HistoryContext::default(),
            git: Vec::new(),
            paths: vec!["src/日本語.rs".to_owned()],
            warnings: Vec::new(),
            decision_groups: Vec::new(),
        };
        for format in [OutputFormat::Text, OutputFormat::Json, OutputFormat::Yaml] {
            let rendered = render_context(&packet, format, 80).expect("bounded output");
            assert!(rendered.len() <= 320);
        }
        assert!(
            render_context(&packet, OutputFormat::Json, 1)
                .expect_err("mandatory packet does not fit")
                .contains("use at least")
        );
    }
}
