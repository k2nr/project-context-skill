---
name: reconstruct-project-context
description: Reconstruct durable project-context model entries and decision or attempt events from repository-linked local Git, repository files, and Codex or Claude Code history. Use only when the user asks to recover, rebuild, backfill, or reconstruct project context from past project history.
---

# Reconstruct Project Context

Recover durable intent from local history without importing transcripts, guesses, or prior
Project Context conclusions as ground truth.

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
4. Create a mode-0700 temporary directory outside the repository. Copy the canonical `model.yaml`
   and `events.jsonl` into it as opaque immutable base snapshots. Never open, print, summarize, or
   use their current or historical contents as reconstruction evidence. They exist only for
   preservation and concurrent-change detection during apply.
5. Run `collect` with `--include-git`, `--include-conversations`, or both according to the approved base sources, plus only the approved optional flags:

   ```sh
   python3 .agents/skills/reconstruct-project-context/scripts/inventory_local_history.py \
     collect --root "$PWD" --output "$temporary/inventory" \
     --include-git --include-conversations
   ```

   Omit either base-source flag unless that source was approved. Treat the selected Git and conversation source list in `summary.json` as frozen for this run.
   When Git is selected, collection also freezes tracked documentation from `HEAD`; it reads the
   tracked worktree version only when `--include-worktree` was approved. Documentation snapshots,
   block coverage, and manifests remain inside the private temporary directory and are never
   canonical Project Context files.
6. Classify every entry in `document-coverage.jsonl` before constructing candidates. Collection
   creates one source per non-empty, blank-line-delimited block in tracked Markdown,
   reStructuredText, AsciiDoc, text, and conventional README/SPEC/DESIGN/ARCHITECTURE/DECISIONS/ROADMAP
   files. Resolve each block as exactly one of:

   - `model`: current durable intent, with `topic`, canonical `statement`, and `candidate` as `section:id`;
   - `decision`: an explicit choice and reason, with `topic`, `rationale`, and a `candidate:` Decision;
   - `attempt`: a reusable experiment outcome, with `topic`, `finding`, and a `candidate:` Attempt;
   - `recoverable`: content recoverable from current code, tests, or schemas, with `topic` and one or
     more frozen code, test, or schema `file:` references in `recovered_by`;
   - `excluded`, with a closed `reason_code`, a specific `reason`, and `duplicate_of` when the code
     is `duplicate_within_document`;
   - `unavailable`, retained exactly as emitted by collection.

   A tracked document cannot be used as its own `recoverable` evidence. Do not summarize a whole
   file as covered when one block remains `pending`. For `model`, `decision`, and `attempt`, set
   `supported_by` to either every origin commit recorded for the block's non-empty lines or a frozen
   direct-user signal mapped to the same candidate. Uncommitted worktree lines have no commit
   support. Document block refs are temporary mapping identities and must never appear in canonical
   evidence.
7. Review the remaining history in two passes: first chronologically classify every `pending` commit,
   conversation record, and selected untracked file; then inspect relevant topics in depth. Change
   every ordinary coverage item to `analyzed`, `excluded`, or `unavailable`, with a reason for the
   latter two.

   Independently classify every entry in `decision-coverage.jsonl`. These entries represent direct
   user messages and prevent a structurally complete but low-recall review:

   - `decision`: a durable choice whose supporting rationale must be traced through the surrounding
     exchange, summarized in `rationale`, and linked through `candidate` to the exact candidate
     Decision that repeats that rationale and includes this source as evidence;
   - `attempt`: a reusable outcome whose `finding` and `candidate` match an Attempt that includes
     this source as evidence;
   - `model`: durable current intent that belongs in the model but lacks a reason-qualified Decision;
   - `excluded`: not durable project intent, with a specific reason;
   - `unavailable`: the decision signal cannot be evaluated, with a specific reason.

   Do not bulk-exclude assistant or tool records before resolving short acceptances, corrections,
   quoted proposals, and references such as "adopt all" against their neighboring context. Verify
   that no pending or unreconciled item remains:

   ```sh
   python3 .agents/skills/reconstruct-project-context/scripts/inventory_local_history.py \
     verify-coverage --inventory "$temporary/inventory"
   ```

   Do not construct candidates unless verification succeeds.
8. Build the complete independent candidate set from approved evidence, without consulting the base
   model or base events. Follow the qualification rules exactly. Write the proposed integrated model
   only after candidate extraction is complete, using the opaque base solely as a preservation
   boundary. Write candidate events from oldest to newest; within one date, preserve the order in
   which their earliest qualifying evidence occurred. Set `occurred_at` only to an explicit choice
   time for a decision or outcome time for an attempt; matching evidence must use `role: choice` or
   `role: outcome` and the same `observed_at`. Keep rationale and context timestamps only on their
   evidence items, and do not infer a time from sequence alone. Emit schema v3 structured evidence
   with `ref`, and add `role` or `observed_at` only when the source supports them. Use typed
   relations, including scoped `partially_supersedes`, instead of flattening every historical change
   into full supersession. Never emit document `file:` refs. Then require every `decision` or
   `attempt` signal to be
   represented in candidate event evidence:

   ```sh
   python3 .agents/skills/reconstruct-project-context/scripts/inventory_local_history.py \
     verify-candidates --inventory "$temporary/inventory" \
     --events "$temporary/new-events.jsonl"
   ```

   A passing record-coverage check is not a candidate-completeness check; both commands must pass.
9. Run the complete side-effect-free gate, then apply through the same validation path:

   ```sh
   project-context check-reconstruction \
     --base-model "$temporary/base-model.yaml" \
     --base-events "$temporary/base-events.jsonl" \
     --model "$temporary/proposed-model.yaml" \
     --events "$temporary/new-events.jsonl" \
     --inventory "$temporary/inventory"

   project-context apply-reconstruction \
     --base-model "$temporary/base-model.yaml" \
     --base-events "$temporary/base-events.jsonl" \
     --model "$temporary/proposed-model.yaml" \
     --events "$temporary/new-events.jsonl" \
     --inventory "$temporary/inventory"
   ```

10. Report selected sources, record and document-block coverage counts, additions, duplicates,
    conflicts, unavailable sources, and contradictions preserved in the base model. Delete the
    temporary directory on success, failure, or interruption. Reconstruction adds no persistent
    inventory or coverage files; it changes only the existing canonical model and event targets.

## Hard constraints

- Include only Codex or Claude Code sessions whose metadata links them to the repository root or a worktree sharing its Git common directory. Never search unrelated conversation content to establish relevance.
- Always exclude ignored files. Read non-ignored untracked files only when explicitly selected.
- Never use current or historical `.project-context/model.yaml` or
  `.project-context/events.jsonl` content as evidence. The inventory removes those Git paths and
  redacts non-user conversation records that materialize them.
- Never save transcript text, secrets, absolute transcript paths, routine failures, or inferred rationale in canonical data.
- Never persist document snapshots, coverage files, or inventory manifests in the repository.
- Express conversation evidence only as `conversation:<provider>:<session-id>#<record-index>`.
- Treat CLI exit 3 as a base-state conflict. Do not retry against a new base without reporting the conflict and re-evaluating the candidates.
- Do not change versions, tags, releases, published assets, or installed skill contents.
