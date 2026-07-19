---
name: reconstruct-project-context
description: Reconstruct durable project-context model entries and decision or attempt events from repository-linked local Git, repository files, and Codex or Claude Code history. Use only when the user asks to recover, rebuild, backfill, or reconstruct project context from past project history.
---

# Reconstruct Project Context

Recover durable intent from local history without importing transcripts or guesses.

## Required reading

Read [references/sources.md](references/sources.md) and [references/qualification.md](references/qualification.md) before collecting substantive history.

## Scope gate

If the user did not explicitly select sources, run only the metadata preflight and ask one question that presents these choices together:

- reachable Git refs and tracked history;
- repository-linked Codex and Claude Code sessions;
- optionally reflogs and unreachable commits, initialized submodules, tracked worktree changes, and non-ignored untracked files.

Do not read conversation messages, tool bodies, Git diffs, historical file contents, or untracked file contents before the answer. Do not mutate `.project-context`. The preflight may read only source existence/counts and bounded association metadata such as session ID, timestamp, cwd, project root, and Git common directory.

Never infer consent from invoking this skill. Skip the question only when the user's request already names the selected source scope.

## Workflow

1. Discover the repository root and require an initialized, valid `.project-context` store.
2. Run the preflight. When scope is already explicit, pass only its base-source flag; when scope is unspecified, omit both flags to inventory bounded metadata for the one scope question:

   ```sh
   python3 .agents/skills/reconstruct-project-context/scripts/inventory_local_history.py preflight --root "$PWD"
   ```

3. Complete the scope gate when required. Do not access external services or the network.
4. Create a mode-0700 temporary directory outside the repository. Copy the canonical `model.yaml` and `events.jsonl` into it as immutable base snapshots.
5. Run `collect` with `--include-git`, `--include-conversations`, or both according to the approved base sources, plus only the approved optional flags:

   ```sh
   python3 .agents/skills/reconstruct-project-context/scripts/inventory_local_history.py \
     collect --root "$PWD" --output "$temporary/inventory" \
     --include-git --include-conversations
   ```

   Omit either base-source flag unless that source was approved. Treat the selected Git and conversation source list in `summary.json` as frozen for this run.
6. Review history in two passes: first chronologically classify every `pending` commit, conversation record, and selected untracked file; then inspect relevant topics in depth. Change every coverage item to `analyzed`, `excluded`, or `unavailable`, with a reason for the latter two. Verify that no pending or unreconciled item remains:

   ```sh
   python3 .agents/skills/reconstruct-project-context/scripts/inventory_local_history.py \
     verify-coverage --inventory "$temporary/inventory"
   ```

   Do not construct candidates unless verification succeeds.
7. Build a complete proposed model from the base model and a JSONL file containing only new candidate events. Follow the qualification and merge rules exactly.
8. Apply once through the concurrency-safe CLI:

   ```sh
   project-context apply-reconstruction \
     --base-model "$temporary/base-model.yaml" \
     --base-events "$temporary/base-events.jsonl" \
     --model "$temporary/proposed-model.yaml" \
     --events "$temporary/new-events.jsonl"
   ```

9. Report selected sources, coverage counts, additions, duplicates, conflicts, unavailable sources, and contradictions preserved in the base model. Delete the temporary directory on success, failure, or interruption.

## Hard constraints

- Include only Codex or Claude Code sessions whose metadata links them to the repository root or a worktree sharing its Git common directory. Never search unrelated conversation content to establish relevance.
- Always exclude ignored files. Read non-ignored untracked files only when explicitly selected.
- Never save transcript text, secrets, absolute transcript paths, routine failures, or inferred rationale in canonical data.
- Express conversation evidence only as `conversation:<provider>:<session-id>#<record-index>`.
- Treat CLI exit 3 as a base-state conflict. Do not retry against a new base without reporting the conflict and re-evaluating the candidates.
- Do not change versions, tags, releases, published assets, or installed skill contents.
