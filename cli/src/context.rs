use crate::store::RepositoryData;
use serde::Serialize;
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MODEL_SECTIONS: [&str; 7] = [
    "project",
    "principles",
    "architecture",
    "behaviors",
    "constraints",
    "operations",
    "extensions",
];
const ENTRY_SECTIONS: [&str; 4] = ["principles", "architecture", "behaviors", "constraints"];
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const GIT_TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Default, Serialize)]
pub struct HistoryContext {
    pub decisions: Vec<Value>,
    pub attempts: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GitEvidence {
    pub commit: String,
    pub subject: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextPacket {
    pub query: Vec<String>,
    pub current_intent: BTreeMap<String, Value>,
    pub history: HistoryContext,
    pub git: Vec<GitEvidence>,
    pub paths: Vec<String>,
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub(crate) decision_groups: Vec<Vec<String>>,
}

#[derive(Clone)]
struct ScoredValue {
    score: usize,
    id: String,
    value: Value,
}

pub fn build_context(
    root: &Path,
    data: &RepositoryData,
    queries: &[String],
    _max_tokens: usize,
) -> ContextPacket {
    let normalized_queries: Vec<String> = queries
        .iter()
        .map(|query| normalize(query))
        .filter(|query| !query.is_empty())
        .collect();
    let (_, lexical_related_event_ids) = model_matches(data, &normalized_queries, &BTreeSet::new());
    let (lexical_decisions, lexical_attempts) =
        event_matches(data, &normalized_queries, &lexical_related_event_ids);
    let matched_event_ids = lexical_decisions
        .iter()
        .chain(lexical_attempts.iter())
        .map(|item| item.id.clone())
        .collect();
    let (model_matches, model_related_event_ids) =
        model_matches(data, &normalized_queries, &matched_event_ids);
    let related_event_ids = lexical_related_event_ids
        .union(&model_related_event_ids)
        .cloned()
        .collect();
    let (decisions, attempts) = event_matches(data, &normalized_queries, &related_event_ids);
    let (decisions, decision_groups) = expand_decision_components(data, decisions);
    let (git, paths, mut warnings) = git_context(root, queries, &normalized_queries);

    let mut packet = ContextPacket {
        query: queries.to_vec(),
        current_intent: BTreeMap::new(),
        history: HistoryContext::default(),
        git: Vec::new(),
        paths: Vec::new(),
        warnings: Vec::new(),
        decision_groups,
    };
    warnings.sort();
    warnings.dedup();
    packet.warnings = warnings;

    for section in MODEL_SECTIONS {
        if let Some(matches) = model_matches.get(section) {
            if ENTRY_SECTIONS.contains(&section) {
                packet.current_intent.insert(
                    section.to_owned(),
                    Value::Array(matches.iter().map(|item| item.value.clone()).collect()),
                );
            } else if let Some(item) = matches.first() {
                packet
                    .current_intent
                    .insert(section.to_owned(), item.value.clone());
            }
        }
    }
    packet.history.decisions = decisions.into_iter().map(|item| item.value).collect();
    packet.history.attempts = attempts.into_iter().map(|item| item.value).collect();
    packet.git = git;
    packet.paths = paths;
    packet
}

fn model_matches(
    data: &RepositoryData,
    queries: &[String],
    matched_event_ids: &BTreeSet<String>,
) -> (BTreeMap<String, Vec<ScoredValue>>, BTreeSet<String>) {
    let mut matches = BTreeMap::new();
    let mut related = BTreeSet::new();
    for section in MODEL_SECTIONS {
        let mut section_matches = Vec::new();
        let Some(section_value) = data.model.get(section) else {
            continue;
        };
        if ENTRY_SECTIONS.contains(&section) {
            let Some(entries) = section_value.as_array() else {
                continue;
            };
            for entry in entries {
                let references: BTreeSet<String> = entry
                    .get("related_events")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .chain(
                        entry
                            .get("event_relations")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|relation| relation.get("event").and_then(Value::as_str))
                            .map(str::to_owned),
                    )
                    .collect();
                let mut score = relevance(entry, queries);
                if !references.is_disjoint(matched_event_ids) {
                    score += 2000;
                }
                if score == 0 {
                    continue;
                }
                related.extend(references);
                section_matches.push(ScoredValue {
                    score,
                    id: value_id(entry),
                    value: entry.clone(),
                });
            }
        } else {
            let score = relevance(section_value, queries);
            if score > 0 {
                section_matches.push(ScoredValue {
                    score,
                    id: section.to_owned(),
                    value: section_value.clone(),
                });
            }
        }
        sort_matches(&mut section_matches);
        if !section_matches.is_empty() {
            matches.insert(section.to_owned(), section_matches);
        }
    }
    (matches, related)
}

fn expand_decision_components(
    data: &RepositoryData,
    decisions: Vec<ScoredValue>,
) -> (Vec<ScoredValue>, Vec<Vec<String>>) {
    let events: BTreeMap<String, &Value> = data
        .events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("decision"))
        .map(|event| (value_id(event), event))
        .collect();
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut superseded = BTreeSet::new();
    for (id, event) in &events {
        graph.entry(id.clone()).or_default();
        for target in event
            .get("supersedes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if events.contains_key(target) {
                graph
                    .entry(id.clone())
                    .or_default()
                    .insert(target.to_owned());
                graph
                    .entry(target.to_owned())
                    .or_default()
                    .insert(id.clone());
                superseded.insert(target.to_owned());
            }
        }
        for relation in event
            .get("relations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let kind = relation.get("kind").and_then(Value::as_str);
            if !matches!(kind, Some("supersedes" | "partially_supersedes")) {
                continue;
            }
            let Some(target) = relation.get("event").and_then(Value::as_str) else {
                continue;
            };
            if events.contains_key(target) {
                graph
                    .entry(id.clone())
                    .or_default()
                    .insert(target.to_owned());
                graph
                    .entry(target.to_owned())
                    .or_default()
                    .insert(id.clone());
                if kind == Some("supersedes") {
                    superseded.insert(target.to_owned());
                }
            }
        }
    }

    let scores: BTreeMap<String, usize> = decisions
        .iter()
        .map(|item| (item.id.clone(), item.score))
        .collect();
    let mut included = BTreeSet::new();
    let mut groups = Vec::new();
    for matched in decisions.iter().map(|item| item.id.clone()) {
        if included.contains(&matched) {
            continue;
        }
        let mut queue = VecDeque::from([matched]);
        let mut group = BTreeSet::new();
        while let Some(id) = queue.pop_front() {
            if !group.insert(id.clone()) {
                continue;
            }
            if let Some(neighbors) = graph.get(&id) {
                queue.extend(neighbors.iter().cloned());
            }
        }
        included.extend(group.iter().cloned());
        groups.push(group.into_iter().collect::<Vec<_>>());
    }

    let mut expanded: Vec<ScoredValue> = included
        .iter()
        .filter_map(|id| {
            events.get(id).map(|event| ScoredValue {
                score: scores.get(id).copied().unwrap_or(1_500),
                id: id.clone(),
                value: (*event).clone(),
            })
        })
        .collect();
    expanded.sort_by_key(|item| {
        (
            superseded.contains(&item.id),
            Reverse(item.score),
            Reverse(item.id.clone()),
        )
    });
    groups.sort_by_key(|group| {
        Reverse(
            group
                .iter()
                .filter_map(|id| scores.get(id))
                .copied()
                .max()
                .unwrap_or_default(),
        )
    });
    (expanded, groups)
}

fn event_matches(
    data: &RepositoryData,
    queries: &[String],
    related: &BTreeSet<String>,
) -> (Vec<ScoredValue>, Vec<ScoredValue>) {
    let mut decisions = Vec::new();
    let mut attempts = Vec::new();
    for event in &data.events {
        let id = value_id(event);
        let mut score = relevance(event, queries);
        if related.contains(&id) {
            score += 2000;
        }
        if score == 0 {
            continue;
        }
        let item = ScoredValue {
            score,
            id,
            value: event.clone(),
        };
        match event.get("type").and_then(Value::as_str) {
            Some("decision") => decisions.push(item),
            Some("attempt") => attempts.push(item),
            _ => {}
        }
    }
    sort_matches(&mut decisions);
    sort_matches(&mut attempts);
    (decisions, attempts)
}

fn sort_matches(matches: &mut [ScoredValue]) {
    matches.sort_by_key(|item| (Reverse(item.score), item.id.clone()));
}

fn relevance(value: &Value, queries: &[String]) -> usize {
    let text = normalize(&serde_json::to_string(value).unwrap_or_default());
    let words: BTreeSet<&str> = text.split_whitespace().collect();
    queries
        .iter()
        .map(|query| {
            let mut score = 0;
            if query.chars().count() > 1 && text.contains(query) {
                score += 1000 + query.len();
            }
            for token in query
                .split_whitespace()
                .filter(|token| token.chars().count() > 1)
            {
                if words.contains(token) {
                    score += 50 + token.len();
                }
            }
            score
        })
        .sum()
}

fn normalize(input: &str) -> String {
    let mut output = String::new();
    let characters: Vec<char> = input.chars().collect();
    let mut pending_space = false;
    for (index, character) in characters.iter().copied().enumerate() {
        let previous = index.checked_sub(1).and_then(|value| characters.get(value));
        let next = characters.get(index + 1);
        let camel_boundary = character.is_uppercase()
            && previous.is_some_and(|value| {
                value.is_lowercase() || value.is_ascii_digit() || matches!(value, '+' | '#')
            });
        let acronym_boundary = character.is_uppercase()
            && previous.is_some_and(|value| value.is_uppercase())
            && next.is_some_and(|value| value.is_lowercase());
        if camel_boundary || acronym_boundary {
            pending_space = true;
        }
        if character.is_alphanumeric() || matches!(character, '+' | '#') {
            if pending_space && !output.is_empty() {
                output.push(' ');
            }
            for lowercase in character.to_lowercase() {
                output.push(lowercase);
            }
            pending_space = false;
        } else {
            pending_space = true;
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn value_id(value: &Value) -> String {
    value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn git_context(
    root: &Path,
    queries: &[String],
    normalized_queries: &[String],
) -> (Vec<GitEvidence>, Vec<String>, Vec<String>) {
    let mut warnings = Vec::new();
    let started = Instant::now();
    let repository_check = run_git_budgeted(
        root,
        &["rev-parse".to_owned(), "--is-inside-work-tree".to_owned()],
        started,
    );
    if !repository_check.is_ok_and(|output| output.trim() == "true") {
        warnings.push("Git repository is not available; Git evidence was skipped".to_owned());
        return (Vec::new(), Vec::new(), warnings);
    }

    if let Ok(output) = run_git_budgeted(
        root,
        &["rev-parse".to_owned(), "--is-shallow-repository".to_owned()],
        started,
    ) && output.trim() == "true"
    {
        warnings.push("Git history is shallow; relevant older evidence may be missing".to_owned());
    }

    let mut commits = Vec::new();
    let mut seen_commits = BTreeSet::new();
    'queries: for query in queries {
        if started.elapsed() >= GIT_TOTAL_TIMEOUT {
            warnings.push("Git evidence search exceeded the 20-second total budget".to_owned());
            break;
        }
        let pickaxe = format!("-S{query}");
        let args = vec![
            "log".to_owned(),
            "--all".to_owned(),
            "--pickaxe-all".to_owned(),
            "--format=%H%x09%s".to_owned(),
            "-n".to_owned(),
            "10".to_owned(),
            pickaxe,
        ];
        match run_git_budgeted(root, &args, started) {
            Ok(output) => collect_commits(&output, &mut commits, &mut seen_commits),
            Err(error) => warnings.push(format!("Git pickaxe search failed: {error}")),
        }

        let message_args = vec![
            "log".to_owned(),
            "--all".to_owned(),
            "--regexp-ignore-case".to_owned(),
            "--fixed-strings".to_owned(),
            format!("--grep={query}"),
            "--format=%H%x09%s".to_owned(),
            "-n".to_owned(),
            "10".to_owned(),
        ];
        match run_git_budgeted(root, &message_args, started) {
            Ok(output) => collect_commits(&output, &mut commits, &mut seen_commits),
            Err(error) => warnings.push(format!("Git message search failed: {error}")),
        }

        if query.contains('/') || query.contains('.') || root.join(query).exists() {
            let path = absolute_to_relative(root, query);
            let path_args = vec![
                "log".to_owned(),
                "--follow".to_owned(),
                "--format=%H%x09%s".to_owned(),
                "-n".to_owned(),
                "10".to_owned(),
                "--".to_owned(),
                path,
            ];
            match run_git_budgeted(root, &path_args, started) {
                Ok(output) => collect_commits(&output, &mut commits, &mut seen_commits),
                Err(error) => warnings.push(format!("Git path history search failed: {error}")),
            }
        }
        if started.elapsed() >= GIT_TOTAL_TIMEOUT {
            warnings.push("Git evidence search exceeded the 20-second total budget".to_owned());
            break 'queries;
        }
    }
    commits.truncate(20);

    let mut path_scores: BTreeMap<String, usize> = BTreeMap::new();
    match run_git_bytes_budgeted(
        root,
        &[
            "-c".to_owned(),
            "core.quotePath=false".to_owned(),
            "ls-files".to_owned(),
            "-z".to_owned(),
        ],
        false,
        started,
    ) {
        Ok(output) => {
            for path in nul_strings(&output) {
                let score = relevance(&Value::String(path.clone()), normalized_queries);
                if score > 0 {
                    path_scores.insert(path, score);
                }
            }
        }
        Err(error) => warnings.push(format!("Git tracked-path search failed: {error}")),
    }
    if started.elapsed() < GIT_TOTAL_TIMEOUT {
        let mut args = vec![
            "grep".to_owned(),
            "-l".to_owned(),
            "-z".to_owned(),
            "-i".to_owned(),
            "-F".to_owned(),
        ];
        for query in queries {
            args.push("-e".to_owned());
            args.push(query.clone());
        }
        args.push("--".to_owned());
        match run_git_bytes_budgeted(root, &args, true, started) {
            Ok(output) => {
                for path in nul_strings(&output) {
                    *path_scores.entry(path).or_default() += 5_000;
                }
            }
            Err(error) => warnings.push(format!("Git content search failed: {error}")),
        }
    }
    let mut paths: Vec<(String, usize)> = path_scores.into_iter().collect();
    paths.sort_by_key(|(path, score)| (Reverse(*score), path.clone()));
    let paths: Vec<String> = paths.into_iter().map(|(path, _)| path).take(20).collect();
    warnings.sort();
    warnings.dedup();
    (commits, paths, warnings)
}

fn nul_strings(output: &[u8]) -> Vec<String> {
    output
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
        .map(|item| String::from_utf8_lossy(item).into_owned())
        .collect()
}

fn absolute_to_relative(root: &Path, query: &str) -> String {
    let path = Path::new(query);
    if path.is_absolute() {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    } else {
        query.to_owned()
    }
}

fn run_git_budgeted(root: &Path, arguments: &[String], started: Instant) -> Result<String, String> {
    let output = run_git_bytes_budgeted(root, arguments, false, started)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn run_git_bytes_budgeted(
    root: &Path,
    arguments: &[String],
    allow_no_match: bool,
    started: Instant,
) -> Result<Vec<u8>, String> {
    let remaining = GIT_TOTAL_TIMEOUT
        .checked_sub(started.elapsed())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| "Git processing exceeded the 20-second total budget".to_owned())?;
    run_git_bytes_with_timeout(
        root,
        arguments,
        allow_no_match,
        remaining.min(GIT_COMMAND_TIMEOUT),
    )
}

fn run_git_bytes_with_timeout(
    root: &Path,
    arguments: &[String],
    allow_no_match: bool,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run Git: {error}"))?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("cannot wait for Git: {error}"))?
            .is_some()
        {
            break;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "command exceeded its {:.3}-second timeout",
                timeout.as_secs_f64()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot collect Git output: {error}"))?;
    if output.status.success() || (allow_no_match && output.status.code() == Some(1)) {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn collect_commits(output: &str, commits: &mut Vec<GitEvidence>, seen: &mut BTreeSet<String>) {
    for line in output.lines() {
        let Some((commit, subject)) = line.split_once('\t') else {
            continue;
        };
        if seen.insert(commit.to_owned()) {
            commits.push(GitEvidence {
                commit: commit.to_owned(),
                subject: subject.to_owned(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn data() -> RepositoryData {
        RepositoryData {
            model: json!({
                "principles": [{"id":"preserve-intent","statement":"Preserve user wording."}],
                "architecture": [{"id":"candidate-location","statement":"Candidate generation belongs to the Swift frontend.","related_events":["D-12","D-13"]}],
                "behaviors": [{"id":"conversion-enter","statement":"Enter converts raw input."}],
                "constraints": [{"id":"stable-order","statement":"Candidate ordering remains stable across IPC.","related_events":["A-22"]}]
            }),
            events: vec![
                json!({"schema_version":1,"type":"decision","id":"D-12","date":"2026-06-18","subject":"candidate generation location","decision":"Run candidate generation in Swift.","reason":"Swift owns session state."}),
                json!({"schema_version":1,"type":"decision","id":"D-13","date":"2026-06-19","subject":"candidate caching","decision":"Keep cached candidates in Swift.","reason":"The cache shares frontend state."}),
                json!({"schema_version":1,"type":"attempt","id":"A-22","date":"2026-06-23","subject":"candidate reordering","approach":"Reorder candidates in Rust.","result":"failed","finding":"Candidate IDs no longer matched."}),
                json!({"schema_version":1,"type":"attempt","id":"A-99","date":"2026-06-24","subject":"unrelated logging","approach":"Change log format.","result":"succeeded","finding":"Logs became shorter."}),
            ],
        }
    }

    #[test]
    fn normalizes_symbol_names_and_expands_related_events() {
        let directory = TempDir::new().expect("temporary directory");
        let packet = build_context(
            directory.path(),
            &data(),
            &["CandidateGenerationLocation".to_owned()],
            4000,
        );
        assert_eq!(
            packet.current_intent["architecture"]
                .as_array()
                .expect("architecture entries")
                .len(),
            1
        );
        assert_eq!(packet.history.decisions[0]["id"], "D-12");
        assert_eq!(packet.history.decisions.len(), 2);
        assert!(
            packet
                .history
                .attempts
                .iter()
                .all(|event| event["id"] != "A-99")
        );
    }

    #[test]
    fn path_and_body_queries_return_constraints_and_attempts() {
        let directory = TempDir::new().expect("temporary directory");
        let packet = build_context(
            directory.path(),
            &data(),
            &["Sources/CandidateOrdering.swift".to_owned()],
            4000,
        );
        assert_eq!(
            packet.current_intent["constraints"]
                .as_array()
                .expect("constraint entries")
                .len(),
            1
        );
        assert_eq!(packet.history.attempts[0]["id"], "A-22");
        assert!(
            packet
                .history
                .attempts
                .iter()
                .all(|event| event["id"] != "A-99")
        );
    }

    #[test]
    fn event_match_expands_back_to_current_intent() {
        let directory = TempDir::new().expect("temporary directory");
        let packet = build_context(
            directory.path(),
            &data(),
            &["Swift owns session state".to_owned()],
            4000,
        );
        assert_eq!(packet.history.decisions[0]["id"], "D-12");
        assert_eq!(
            packet.current_intent["architecture"][0]["id"],
            "candidate-location"
        );
    }

    #[test]
    fn preserves_all_non_entry_model_section_shapes() {
        let directory = TempDir::new().expect("temporary directory");
        let data = RepositoryData {
            model: json!({
                "project": {"id":"shape-test","description":"C++ HTTPRequest service"},
                "principles": [],
                "architecture": [],
                "behaviors": [],
                "constraints": [],
                "operations": {"build":["make C++"],"test":[],"lint":[],"format":[]},
                "extensions": {"HTTPRequest":{"owner":"agent"}}
            }),
            events: Vec::new(),
        };
        let packet = build_context(
            directory.path(),
            &data,
            &["C++ HTTPRequest".to_owned()],
            4000,
        );
        assert!(packet.current_intent["project"].is_object());
        assert!(packet.current_intent["operations"].is_object());
        assert!(packet.current_intent["extensions"].is_object());
        assert_eq!(normalize("C++HTTPRequest"), "c++ http request");
    }

    #[test]
    fn returns_a_complete_supersession_component_with_current_decision_first() {
        let directory = TempDir::new().expect("temporary directory");
        let data = RepositoryData {
            model: json!({}),
            events: vec![
                json!({"type":"decision","id":"D-1","subject":"legacy transport","decision":"Use polling."}),
                json!({"type":"decision","id":"D-2","subject":"intermediate transport","decision":"Use callbacks.","supersedes":["D-1"]}),
                json!({"type":"decision","id":"D-3","subject":"current transport","decision":"Use streams.","supersedes":["D-2"]}),
            ],
        };
        let packet = build_context(
            directory.path(),
            &data,
            &["legacy transport".to_owned()],
            4000,
        );
        let ids: Vec<&str> = packet
            .history
            .decisions
            .iter()
            .filter_map(|event| event["id"].as_str())
            .collect();
        assert_eq!(ids.first(), Some(&"D-3"));
        assert_eq!(
            ids.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from(["D-1", "D-2", "D-3"])
        );
        assert_eq!(packet.decision_groups, vec![vec!["D-1", "D-2", "D-3"]]);
    }

    #[test]
    fn one_character_query_does_not_match_every_record() {
        let directory = TempDir::new().expect("temporary directory");
        let packet = build_context(directory.path(), &data(), &["a".to_owned()], 4000);
        assert!(packet.current_intent.is_empty());
        assert!(packet.history.decisions.is_empty());
        assert!(packet.history.attempts.is_empty());
    }

    #[test]
    fn token_budget_omits_whole_records() {
        let directory = TempDir::new().expect("temporary directory");
        let packet = build_context(directory.path(), &data(), &["candidate".to_owned()], 120);
        let rendered =
            crate::output::render_context(&packet, crate::output::OutputFormat::Json, 120)
                .expect("bounded context");
        assert!(rendered.len() <= 480);
        assert!(rendered.contains("omitted by the --max-tokens budget"));
        for event in packet
            .history
            .decisions
            .iter()
            .chain(packet.history.attempts.iter())
        {
            assert!(event.get("id").is_some());
            assert!(event.get("subject").is_some());
        }
    }

    #[test]
    fn old_symbol_is_found_in_git_history() {
        let directory = TempDir::new().expect("temporary directory");
        run(directory.path(), &["init"]);
        run(
            directory.path(),
            &["config", "user.email", "test@example.com"],
        );
        run(directory.path(), &["config", "user.name", "Test User"]);
        fs::write(
            directory.path().join("source.txt"),
            "struct OldCandidateProvider {}\n",
        )
        .expect("write old source");
        run(directory.path(), &["add", "source.txt"]);
        run(directory.path(), &["commit", "-m", "Add old provider"]);
        fs::write(
            directory.path().join("source.txt"),
            "struct NewCandidateProvider {}\n",
        )
        .expect("write new source");
        run(directory.path(), &["add", "source.txt"]);
        run(directory.path(), &["commit", "-m", "Rename provider"]);

        let packet = build_context(
            directory.path(),
            &RepositoryData {
                model: json!({}),
                events: Vec::new(),
            },
            &["OldCandidateProvider".to_owned()],
            4000,
        );
        assert!(!packet.git.is_empty());
        assert!(
            packet
                .git
                .iter()
                .all(|evidence| evidence.commit.len() == 40)
        );
        assert!(packet.paths.is_empty());
    }

    #[test]
    fn exact_content_outscores_many_filename_matches_and_unicode_paths_survive() {
        let directory = TempDir::new().expect("temporary directory");
        run(directory.path(), &["init"]);
        run(
            directory.path(),
            &["config", "user.email", "test@example.com"],
        );
        run(directory.path(), &["config", "user.name", "Test User"]);
        for index in 0..24 {
            fs::write(
                directory.path().join(format!("candidate-{index:02}.txt")),
                "unrelated\n",
            )
            .expect("write filename match");
        }
        fs::write(
            directory.path().join("日本語.txt"),
            "candidate exact content\n",
        )
        .expect("write Unicode content match");
        run(directory.path(), &["add", "."]);
        run(
            directory.path(),
            &["commit", "-m", "Add path scoring fixture"],
        );

        let packet = build_context(
            directory.path(),
            &RepositoryData {
                model: json!({}),
                events: Vec::new(),
            },
            &["candidate".to_owned()],
            4000,
        );
        assert_eq!(packet.paths.first().map(String::as_str), Some("日本語.txt"));
        assert!(packet.paths.len() <= 20);
    }

    #[test]
    fn deleted_root_path_is_retrieved_from_history() {
        let directory = TempDir::new().expect("temporary directory");
        run(directory.path(), &["init"]);
        run(
            directory.path(),
            &["config", "user.email", "test@example.com"],
        );
        run(directory.path(), &["config", "user.name", "Test User"]);
        fs::write(directory.path().join("removed.txt"), "legacy\n").expect("write old path");
        run(directory.path(), &["add", "removed.txt"]);
        run(directory.path(), &["commit", "-m", "Add removed root path"]);
        run(directory.path(), &["rm", "removed.txt"]);
        run(directory.path(), &["commit", "-m", "Remove root path"]);

        let packet = build_context(
            directory.path(),
            &RepositoryData {
                model: json!({}),
                events: Vec::new(),
            },
            &["removed.txt".to_owned()],
            4000,
        );
        assert!(
            packet
                .git
                .iter()
                .any(|item| item.subject == "Remove root path")
        );
    }

    #[test]
    fn no_git_repository_produces_warning_not_error() {
        let directory = TempDir::new().expect("temporary directory");
        let packet = build_context(directory.path(), &data(), &["candidate".to_owned()], 4000);
        assert!(
            packet
                .warnings
                .iter()
                .any(|warning| warning.contains("Git repository is not available"))
        );
    }

    #[test]
    fn shallow_git_repository_produces_warning() {
        let source = TempDir::new().expect("source repository");
        run(source.path(), &["init"]);
        run(source.path(), &["config", "user.email", "test@example.com"]);
        run(source.path(), &["config", "user.name", "Test User"]);
        fs::write(source.path().join("source.txt"), "candidate\n").expect("write source");
        run(source.path(), &["add", "source.txt"]);
        run(source.path(), &["commit", "-m", "Add candidate"]);

        let clone_parent = TempDir::new().expect("clone parent");
        let clone = clone_parent.path().join("shallow");
        let source_url = format!("file://{}", source.path().display());
        let output = Command::new("git")
            .args(["clone", "--depth", "1", &source_url])
            .arg(&clone)
            .output()
            .expect("clone repository");
        assert!(output.status.success());

        let packet = build_context(
            &clone,
            &RepositoryData {
                model: json!({}),
                events: Vec::new(),
            },
            &["candidate".to_owned()],
            4000,
        );
        assert!(
            packet
                .warnings
                .iter()
                .any(|warning| warning.contains("Git history is shallow"))
        );
    }

    fn run(root: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .status()
            .expect("run Git");
        assert!(status.success());
    }
}
