# Project Context

Project Context prevents AI coding agents from losing or repeatedly
rediscovering durable project intent, decisions, and experimental results
across development sessions. It targets solo software projects in which AI
agents perform the complete development workflow.

The project assumes that the human developer does not read specifications
directly and always uses an AI agent as the interface to the codebase. Its
canonical data and retrieval workflow therefore optimize for efficient AI
consumption rather than human-oriented documentation. Each target repository
receives its own project-installed skill and context store.

## Install with a Coding Agent

The installation flow uses a standalone, checksummed, and attested installer.
The installer verifies the skill release before any repository write, performs a
dry run, refuses local differences, maintains the managed `AGENTS.md` block,
initializes or preserves canonical data, and returns a machine-readable report.

The official GitHub repository is `k2nr/project-context-skill`.

Copy this instruction verbatim:

```text
Install Project Context v0.1.7 into the current repository.

Official GitHub repository:
k2nr/project-context-skill

Installation requirements:
1. Work from the target repository root and inspect it to determine an accurate
   project ID, concise description, and real build, test, lint, and format
   commands. Explicitly identify any operation category that intentionally has
   no command.
2. Download these assets for tag v0.1.7 into a private temporary directory:
   - https://github.com/k2nr/project-context-skill/releases/download/v0.1.7/install-project-context-v0.1.7
   - https://github.com/k2nr/project-context-skill/releases/download/v0.1.7/install-project-context-v0.1.7.sha256
3. Verify the installer with `sha256sum -c install-project-context-v0.1.7.sha256`
   or `shasum -a 256 -c install-project-context-v0.1.7.sha256`. Stop if it fails.
4. If `gh` already exists, run
   `gh attestation verify install-project-context-v0.1.7 --repo k2nr/project-context-skill`.
   Stop if it fails. Do not install `gh`, Python packages, Cargo, shellcheck, or
   another tool solely for installation.
5. Make the verified installer executable. Run it first with
   `--dry-run --format json`, passing `--project-id`, `--description`, and repeatable
   `--build`, `--test`, `--lint`, and `--format-command` values discovered in
   step 1. For a legitimately empty category, pass `--allow-empty CATEGORY`.
6. Review the JSON report. If the installer exits with status 3, stop and ask
   before changing the reported local skill files or managed markers.
7. Run the same verified installer command without `--dry-run`. Never add
   `--force`; the installer preserves existing intent and history and updates
   only explicitly supplied model fields.
8. Report the installer and package verification, preflight availability,
   destination, files preserved or not overwritten, AGENTS.md action,
   initialization or preservation result, model action, doctor result, and
   strict validation result.
```

Example installer arguments after repository inspection:

```sh
./install-project-context-v0.1.7 \
  --format json \
  --project-id example-project \
  --description 'Concise project purpose.' \
  --build 'cargo build --locked' \
  --test 'cargo test --locked' \
  --lint 'cargo clippy --all-targets -- -D warnings' \
  --format-command 'cargo fmt -- --check'
```

The installer uses a private temporary directory with cleanup traps. Before
writing, it verifies the package checksum and available GitHub attestation,
rejects unsafe archive paths and member types, confirms the release-only skill
layout, compares an existing installation, and validates managed-marker shape.
It never overwrites a differing installed skill. Exit status `3` denotes a
local-content or managed-marker conflict that requires human direction.

`--dry-run` downloads and verifies release assets but does not modify the
repository. The JSON report includes network and repository write preflight,
optional `gh`, `shellcheck`, and standard-validator availability, planned or
completed actions, and validation state. Standard skill compatibility is
validated in release CI, so target repositories do not need PyYAML.

## Expected Installed Layout

After installation, the target repository contains:

```text
<target-repository>/
├── AGENTS.md
├── .agents/
│   └── skills/
│       └── project-context/
│           ├── SKILL.md
│           ├── LICENSE
│           ├── agents/
│           │   └── openai.yaml
│           ├── assets/
│           │   ├── init/
│           │   │   ├── event.schema.json
│           │   │   ├── model.schema.json
│           │   │   └── model.yaml
│           │   └── install/
│           │       └── AGENTS.fragment.md
│           └── bin/
│               └── project-context
└── .project-context/
    ├── .lock
    ├── model.yaml
    ├── events.jsonl
    └── schemas/
        ├── model.schema.json
        └── event.schema.json
```

The installer creates `.project-context/` through `project-context init`. It
uses `project-context configure` to update only explicitly supplied project and
operation fields, then runs `project-context doctor --installation` and strict
validation. Existing intent sections and event history are preserved.

The managed block instructs compatible coding agents to load Project Context
for non-trivial work without explicit user invocation. Skill discovery remains
a capability of the coding-agent host, so unsupported hosts may require their
own equivalent repository instruction mechanism.

## History Reconstruction Skill

The source tree defines a second repository-local skill,
`reconstruct-project-context`, alongside the automatically loaded
`project-context` skill. The reconstruction skill is invoked only when a user
asks to recover or backfill durable context from past project history; the
managed `AGENTS.md` block does not start it automatically.

When the source scope is not already explicit, the skill must ask before
reading substantive history. The choices cover reachable Git and tracked
history, repository-linked local Codex and Claude Code sessions, and opt-in
reflog/unreachable commits, initialized submodules, tracked worktree changes,
or non-ignored untracked files. Ignored files and external services are always
excluded. Conversation association uses cwd/project metadata for the repository
or a worktree sharing its Git common directory; unrelated conversations are
never selected by searching their content.

After approval, evidence-qualified model additions and new decision or attempt
events are applied automatically through `project-context
apply-reconstruction`. The command rejects stale base snapshots with exit
status `3`, preserves existing intent and event bytes, and makes repeated
semantic candidates a no-op. Canonical evidence records only provider, session
ID, and record index, not transcript paths or transcript text.

Reconstruction does not use current or historical Project Context model/event
files as evidence. Those paths are omitted from Git and worktree inventories,
materialized copies are redacted from non-user conversation records, and base
snapshots remain opaque until the final preservation merge. A separate
direct-user decision audit must also pass before apply, so ordinary source
coverage cannot conceal missed accepted proposals or reason-qualified choices.

Packages produced from this source contain exactly the two top-level skill
directories. Installation accepts only fresh, additive-companion, or fully
identical states. If either installed skill differs from the verified package,
or only the reconstruction skill exists, installation stops without changing
the repository. Updating a differing installation is intentionally a separate
task; there is no automatic upgrade path.

The versioned public installation command above remains unchanged. Publishing
the two-skill package, choosing a next version, and creating release assets are
separate release work and are not performed by this source change.

## Supported Platforms

Prebuilt CLI binaries are published for:

- macOS 11 or newer on x86_64 and arm64.
- GNU Linux with glibc 2.31 or newer on x86_64 and arm64.

musl-based systems, including Alpine Linux, and Windows are not supported. The
launcher detects unsupported Linux libc implementations before downloading a
binary. Cargo is not needed on target machines.

## Development Environment

Development dependencies are defined by `devbox.json` and locked by
`devbox.lock`. With Devbox installed, enter the environment with:

```sh
devbox shell
```

Run the complete project verification with one command:

```sh
devbox run verify
```

The environment includes the Rust toolchain selected by `rust-toolchain.toml`,
ShellCheck, Python 3.12, and PyYAML. The shared `bin/check` entry point provides
the `build`, `format`, `lint`, `test`, and `verify` modes. Linting runs
ShellCheck over the repository's shell files and validates both skills with the
strict repository validator and the PyYAML-based skill-contract validator. The
release workflow invokes the same complete verification entry point.

## Release Requirements

Before publishing a release:

- Enable immutable GitHub Releases and protected release tags for `v*`.
- Keep release tags on the repository default branch.
- Retain full-commit-SHA pins for every GitHub Action.

The release workflow verifies placeholders, repository identity, tag ancestry,
skill-contract compatibility, installer and package reproducibility, artifact
names, checksums, package contents, executable modes, and build provenance
before publication. The standalone installer and every package asset receive
GitHub build-provenance attestations.

## Repository Layout

```text
project-context/  Distributable skill files
reconstruct-project-context/  Local-history reconstruction skill files
cli/              Rust CLI source and CLI tests
bin/              Secure installer, validators, and reproducible packaging tools
tests/            Repository-level package and launcher tests
LICENSE           Repository license
```

The package built from this source includes `project-context/`,
`reconstruct-project-context/`, and a copy of the root `LICENSE` in each skill.
It does not include `cli/`, `tests/`, repository build tools, or development
artifacts.
