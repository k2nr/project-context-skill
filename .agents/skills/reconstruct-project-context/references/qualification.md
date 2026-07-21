# Reconstruction qualification rules

## Model merge

Use the base model only as a non-destructive merge boundary, not as truth or evidence. Extract and
qualify the complete candidate set before semantically inspecting the base. For an explicitly
authorized from-scratch reconstruction, initialize an empty canonical store before collection.

- Preserve every existing principle, architecture entry, behavior, constraint, extension, non-empty project field, and non-empty operation category.
- Fill a missing description or empty operation category only with direct evidence.
- Append only new, non-conflicting model entries with stable, descriptive IDs.
- Do not resolve contradictions by rewriting the base. Preserve and report them.
- Link model entries only to event candidates that survive deduplication. Use typed
  `event_relations` and attach structured evidence directly when it supports the current statement.

## Candidate completeness

Record coverage proves only that every source received a disposition. It does not prove that every
important choice was extracted. Classify every direct-user entry in `decision-coverage.jsonl`, trace
short approvals and corrections to the surrounding proposal and rationale, and run
`verify-candidates` before apply. Every entry classified as `decision` must appear in candidate event
evidence through its declared `candidate:` ID, and its `rationale` must equal that candidate
Decision's reason after whitespace normalization. Do not downgrade an explicit choice to `model`
merely because the selected implementation is recoverable from code; the unrecoverable reason still
belongs in a Decision.

## Decisions

Create a decision only when evidence contains both the selected choice and its reason. A merged commit, final implementation, or repeated pattern alone does not prove rationale. Preserve rejected alternatives or conditions only when explicit evidence supports them.

## Attempts

Create an attempt only for a non-obvious or costly experiment whose result is reusable. Exclude routine command failures, typo fixes, ordinary debugging steps, and unsuccessful actions with no durable finding.

## Evidence hygiene

- Git evidence uses durable commit or repository-relative file references supported by project-context.
- Conversation evidence uses `conversation:<provider>:<session-id>#<record-index>` only.
- Paraphrase the durable conclusion. Never copy transcript passages.
- Omit credentials, tokens, personal data, generated logs, and secret-shaped values.
- Record evidence as objects with `ref`; add `role` and `observed_at` only when explicit.
- Do not invent reasons, causal claims, dates, timestamps, or event relations.

## Candidate IDs and references

Temporary candidate IDs are local staging keys in the `candidate:` namespace, not durable IDs. They must never use a canonical `D-` or `A-` ID. Let `apply-reconstruction` deduplicate semantic content, reuse existing IDs, allocate stable D/A IDs, and resolve candidate references. Treat unresolved references, cycles, and divergent relations as invalid data; do not work around them by deleting evidence. Use `supersedes` only when the earlier decision is wholly obsolete. Use `partially_supersedes` with a concise `scope` when part remains valid, and use other typed relation kinds for provenance without claiming replacement.

Every candidate event and every newly added model entry must cite at least one source whose frozen
coverage status is `analyzed`. Preserved base entries are an opaque boundary and are not requalified
against the current inventory. Never cite excluded or unavailable conversation, commit, or untracked
sources.

## Timeline order

Write candidate events from the oldest point in time to the newest. Populate `occurred_at` only from
an explicit RFC 3339 choice timestamp for a decision or outcome timestamp for an attempt. Include
matching `choice` or `outcome` evidence whose `observed_at` equals `occurred_at`; keep rationale and
context times on the evidence item only. For events without exact time, order same-date records by the
timestamp of their earliest qualifying source record or, when only sequence is available, by that
sequence. Exact-time records are sorted only within contiguous runs; an unknown-time record is a
barrier, so exact timestamps never leapfrog it. Checking, applying, and ordinary event addition use
this same ordering rule.
