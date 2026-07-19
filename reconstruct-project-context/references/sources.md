# Local source rules

## Default sources

- Reachable commits from all local Git refs, including branches and tags.
- Tracked paths and their historical changes.
- Codex JSONL sessions under `~/.codex/sessions` and `~/.codex/archived_sessions`.
- Claude Code JSONL sessions under `~/.claude/projects`.

External services are never sources. Do not call GitHub, Slack, Notion, email, web search, or network APIs.

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
