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

The installation flow is:

1. Copy the instruction below.
2. Send it to the coding agent working in the target repository.
3. Let the agent verify and install the skill, maintain the managed `AGENTS.md`
   block, initialize repository data, populate the discovered project details,
   and validate the result.

The official GitHub repository is `k2nr/project-context-skill`.

Copy this instruction verbatim:

```text
Install Project Context v0.1.0 into the current repository.

Official GitHub repository:
k2nr/project-context-skill

Installation requirements:
1. Work from the target repository root.
2. Download these assets for tag v0.1.0 into a private temporary directory:
   - https://github.com/k2nr/project-context-skill/releases/download/v0.1.0/project-context-skill-v0.1.0.tar.gz
   - https://github.com/k2nr/project-context-skill/releases/download/v0.1.0/project-context-skill-v0.1.0.tar.gz.sha256
3. Verify the archive with `sha256sum -c project-context-skill-v0.1.0.tar.gz.sha256` or `shasum -a 256 -c project-context-skill-v0.1.0.tar.gz.sha256`. Stop if verification fails.
4. If `gh` already exists, also run `gh attestation verify project-context-skill-v0.1.0.tar.gz --repo k2nr/project-context-skill`. Stop if this verification fails. Do not install `gh` solely for this step; report that attestation verification was unavailable and continue with the checksum when `gh` is absent.
5. Inspect the archive before extraction. It must contain exactly one top-level `project-context` directory, no absolute or parent-traversal paths, and no symbolic links.
6. Extract the archive and install its `project-context` directory at `.agents/skills/project-context`. Do not install it globally.
7. If `.agents/skills/project-context` already exists, compare it with the downloaded version. Do not overwrite or delete differing local files without asking me first.
8. Ensure `.agents/skills/project-context/bin/project-context` is executable. Confirm that `SKILL.md`, `LICENSE`, `agents/openai.yaml`, `assets/init`, `assets/install/AGENTS.fragment.md`, and `bin/project-context` are present. The installed package must not contain Rust source, repository tests, or development artifacts.
9. Install the managed Project Context block from `.agents/skills/project-context/assets/install/AGENTS.fragment.md` into the repository-root `AGENTS.md`. Create `AGENTS.md` when absent. When it exists without the markers, preserve all content and append the fragment. When exactly one complete marked block exists, replace only that block. Stop and ask before changing malformed, nested, or duplicate managed markers.
10. Run a shell syntax check on `.agents/skills/project-context/bin/project-context`. Run a standard skill validator only when it already works in the environment. Do not install uv, Python packages, Cargo, or another tool solely for installation.
11. If `.project-context` does not exist, run `.agents/skills/project-context/bin/project-context init`. If it already exists, preserve it and run `.agents/skills/project-context/bin/project-context validate`; never use `--force` automatically.
12. Inspect the repository and update `.project-context/model.yaml` with an accurate project ID and concise description plus the real build, test, lint, and format commands. Preserve valid existing intent and history.
13. Run `.agents/skills/project-context/bin/project-context validate --strict`. Stop and report validation errors if it fails.
14. Report the installed version, destination, checksum result, attestation result or unavailability, AGENTS.md result, initialization or preservation result, model updates, validation result, and every file not overwritten.
```

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

The installation agent creates `.project-context/` through `project-context init`, but
the CLI itself modifies only `.project-context/`. The agent separately maintains the
managed `AGENTS.md` block and never replaces existing canonical data
automatically.

The managed block instructs compatible coding agents to load Project Context
for non-trivial work without explicit user invocation. Skill discovery remains
a capability of the coding-agent host, so unsupported hosts may require their
own equivalent repository instruction mechanism.

## Supported Platforms

Prebuilt CLI binaries are published for:

- macOS 11 or newer on x86_64 and arm64.
- GNU Linux with glibc 2.31 or newer on x86_64 and arm64.

musl-based systems, including Alpine Linux, and Windows are not supported. The
launcher detects unsupported Linux libc implementations before downloading a
binary. Cargo is not needed on target machines.

## Release Requirements

Before publishing the first release:

- Enable immutable GitHub Releases and protected release tags for `v*`.
- Keep release tags on the repository default branch.
- Retain full-commit-SHA pins for every GitHub Action.

The release workflow verifies placeholders, repository identity, tag ancestry,
artifact names, checksums, package contents, executable modes, and build
provenance before publication.

## Repository Layout

```text
project-context/  Distributable skill files
cli/              Rust CLI source and CLI tests
bin/              Reproducible release packaging tools
tests/            Repository-level package and launcher tests
LICENSE           Repository license
```

The release package includes `project-context/` and the root `LICENSE`. It does
not include `cli/`, `tests/`, repository build tools, or development artifacts.
