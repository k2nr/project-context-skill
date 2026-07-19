---
name: project-context
description: Maintain and retrieve durable repository intent for non-trivial planning, implementation, debugging, refactoring, dependency, behavior, architecture, and validation work. Use automatically when the repository contains `.project-context/`, without waiting for explicit invocation, to load relevant current intent and historical decisions or attempts, preserve non-recoverable rationale, and keep implementation and commits consistent with the intended project state.
---

# Project Context Workflow

Persist only information that cannot be recovered reliably from current code,
tests, schemas, and Git. Retrieve only information relevant to the task.

Run commands from the repository root with the project-local launcher:

## Retrieve Context

1. Identify relevant paths, symbols, exact error phrases, behaviors, and topics.
2. Pass them as separate quoted queries:

   ```bash
   .agents/skills/project-context/bin/project-context context \
     "src/candidates.rs" "CandidateProvider" "candidate ownership"
   ```

3. Treat returned model sections as the current intended state.
4. Treat historical events as evidence. A supersession chain is returned as a
   unit with the current decision first.
5. Confirm that retrieved intent still applies when current code, tests,
   schemas, or Git provide conflicting evidence.

`--max-tokens` bounds successful formatted output to four UTF-8 bytes per
approximate token. Increase it if the required packet cannot fit.

## Update Project Context

When explicitly invoked as `$project-context update`, run the updater from the
repository root. First perform a dry run, then run the update when it reports no
conflict:

```bash
.agents/skills/project-context/bin/update-project-context --dry-run --format json
.agents/skills/project-context/bin/update-project-context --format json
```

The updater resolves the latest GitHub Release, verifies the installed and
target skill archives, and updates both repository-local skills plus the managed
`AGENTS.md` block. It preserves `.project-context` byte-for-byte. Exit code `3`
means the installed skills or managed block contain local changes; stop without
overwriting them and report the conflict.

## Maintain Context

1. Follow current intent unless the requested work intentionally changes it.
2. Update `.project-context/model.yaml` in the same semantic change when behavior,
   architecture, invariants, principles, project identity, or project commands
   intentionally change.
3. Do not update the model for intent-preserving renames, moves, or refactors.
4. Record a decision only when its rationale cannot be recovered accurately
   from code, tests, schemas, and Git.
5. Record an attempt only when forgetting the result would likely repeat
   meaningful work or a costly dead end.
6. Never record routine failures, trivial fixes, speculation, or transcripts.
7. Never rewrite a valid historical event. Append a decision with
   `--supersedes`, or append a new attempt under materially different
   conditions.

Record a decision:

```bash
.agents/skills/project-context/bin/project-context add-decision \
  --subject "candidate generation location" \
  --decision "Run candidate generation in the frontend." \
  --reason "The frontend owns input-session state." \
  --rejected "Run candidate generation in the backend" \
  --evidence "file:src/candidates.rs"
```

Record a meaningful experiment:

```bash
.agents/skills/project-context/bin/project-context add-attempt \
  --subject "key-event injection" \
  --approach "Deliver input through platform automation." \
  --result failed \
  --finding "The application received the event but the input method did not."
```

Use `--result inconclusive` for unresolved investigations.

Use `commit:<sha>` only for an existing commit. For work in the current commit,
citing a file, test, issue, or artifact is preferable to predicting a hash.

## Validate

Run strict validation after updating context:

```bash
.agents/skills/project-context/bin/project-context validate --strict
```

Run the relevant build, test, lint, and format commands returned in the
`operations` section.

Exit code `1` means invalid project-context data. Exit code `2` means a command
or environment error. `validate --strict` promotes validation warnings to
invalid data.

## Integrate with Git Commits

Follow project-specific commit rules first. Apply the rules below only where
they do not conflict with the project's required format or workflow:

1. Write a concrete, searchable subject and body using the relevant component,
   behavior, constraint, or error terms. Avoid vague messages that only say
   that code was updated.
2. When a commit relates to recorded durable events, include each applicable
   `Decision-Ref: D-0001` or `Attempt-Ref: A-0001` trailer. If project rules do
   not permit custom trailers, use the project's supported reference mechanism
   instead.
3. When a change modifies durable intent, commit its implementation, tests,
   `.project-context/model.yaml` update, and qualifying decision or attempt records
   together.
