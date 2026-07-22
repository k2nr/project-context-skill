# Local source rules

## Default sources

- Reachable commits from all local Git refs, including branches and tags.
- Tracked paths and their historical changes.
- Codex JSONL sessions under `~/.codex/sessions` and `~/.codex/archived_sessions`.
- Claude Code JSONL sessions under `~/.claude/projects`.

External services are never sources. Do not call GitHub, Slack, Notion, email, web search, or network APIs.

## Canonical-data blindness

Current and historical `.project-context/model.yaml` and `.project-context/events.jsonl` are never
reconstruction sources. Git patches, tracked-path indexes, worktree patches, and untracked snapshots
exclude them. Non-user conversation records that materialize their contents are redacted during
collection. A direct user message may still discuss the desired behavior or naming of Project
Context; treat the user's statement as conversation evidence, not the canonical file as evidence.

The base snapshots used by `apply-reconstruction` are opaque concurrency and preservation inputs.
Do not open or semantically inspect them during source review or candidate extraction.

## Repository association

Accept a conversation only when its metadata cwd or project path resolves to the current repository root, or when it resolves to a worktree with the same `git rev-parse --git-common-dir` result. Resolve paths lexically and through the filesystem where possible. A mention of the repository in a message is not association evidence.

Codex metadata normally appears in a `session_meta` record. Claude Code may encode the project path in the directory name and may also expose `cwd` in records. Use only bounded metadata records during preflight. Do not scan message or tool content for cwd.

## Optional sources

Each source below requires an explicit selection:

- reflog and unreachable commits;
- initialized submodules;
- tracked working-tree changes;
- non-ignored untracked files.

Ignored files remain excluded even when untracked files are selected. Report absent, unreadable, malformed, and oversized sources as unavailable rather than silently dropping them.

## Frozen inventory

Record selected refs, commit IDs, conversation provider/session IDs, record counts, and optional-source flags at collection start. If those source identities change during analysis, report the change and restart from a new base instead of mixing snapshots.

## Tracked documentation

When Git is selected, freeze current tracked documentation from `HEAD` without reading tracked
worktree contents. If tracked worktree changes were explicitly selected, freeze the corresponding
filesystem content and retain the existing worktree concurrency check. Exclude canonical Project
Context paths before reading any content.

Audit UTF-8 documents up to 2 MiB with Markdown, reStructuredText, AsciiDoc, or text suffixes, plus
conventional extensionless README, SPEC, DESIGN, ARCHITECTURE, DECISIONS, and ROADMAP files. Split
them into non-empty blocks at blank lines. Record non-UTF-8, oversized, missing, or non-regular
documents as unavailable. Snapshots and block manifests are private temporary inventory data and
must never be copied into the repository.
