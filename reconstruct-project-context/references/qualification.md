# Reconstruction qualification rules

## Model merge

Use the base model as authoritative.

- Preserve every existing principle, architecture entry, behavior, constraint, extension, non-empty project field, and non-empty operation category.
- Fill a missing description or empty operation category only with direct evidence.
- Append only new, non-conflicting model entries with stable, descriptive IDs.
- Do not resolve contradictions by rewriting the base. Preserve and report them.
- Link model entries only to event candidates that survive deduplication.

## Decisions

Create a decision only when evidence contains both the selected choice and its reason. A merged commit, final implementation, or repeated pattern alone does not prove rationale. Preserve rejected alternatives or conditions only when explicit evidence supports them.

## Attempts

Create an attempt only for a non-obvious or costly experiment whose result is reusable. Exclude routine command failures, typo fixes, ordinary debugging steps, and unsuccessful actions with no durable finding.

## Evidence hygiene

- Git evidence uses durable commit or repository-relative file references supported by project-context.
- Conversation evidence uses `conversation:<provider>:<session-id>#<record-index>` only.
- Paraphrase the durable conclusion. Never copy transcript passages.
- Omit credentials, tokens, personal data, generated logs, and secret-shaped values.
- Do not invent reasons, causal claims, dates, or supersession links.

## Candidate IDs and references

Temporary candidate IDs are local staging keys in the `candidate:` namespace, not durable IDs. They must never use a canonical `D-` or `A-` ID. Let `apply-reconstruction` deduplicate semantic content, reuse existing IDs, allocate stable D/A IDs, and resolve candidate references. Treat unresolved references, cycles, and divergent supersession as invalid data; do not work around them by deleting evidence.
