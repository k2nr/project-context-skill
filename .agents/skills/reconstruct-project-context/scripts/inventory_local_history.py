#!/usr/bin/env python3
"""Create a frozen inventory of repository-linked local history."""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
from typing import Any, Iterable

MAX_METADATA_BYTES = 64 * 1024
MAX_RECORD_BYTES = 2 * 1024 * 1024
MAX_UNTRACKED_BYTES = 2 * 1024 * 1024
MAX_DOCUMENT_BYTES = 2 * 1024 * 1024
SESSION_ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,255}\Z")
CANDIDATE_ID_PATTERN = re.compile(r"candidate:[A-Za-z0-9][A-Za-z0-9._:-]{0,239}\Z")
MODEL_CANDIDATE_PATTERN = re.compile(
    r"(?:principles|architecture|behaviors|constraints):[A-Za-z0-9][A-Za-z0-9._-]{0,239}\Z"
)
DOCUMENT_SUFFIXES = {".md", ".mdx", ".rst", ".adoc", ".asciidoc", ".txt"}
DOCUMENT_BASENAMES = {"readme", "spec", "design", "architecture", "decisions", "roadmap"}
DOCUMENT_EXCLUSION_REASONS = {
    "non_intent",
    "navigation_or_formatting",
    "duplicate_within_document",
    "generated_reference",
}
RECOVERABLE_CODE_SUFFIXES = {
    ".c", ".cc", ".cpp", ".cs", ".go", ".h", ".hpp", ".java", ".js", ".jsx",
    ".kt", ".kts", ".lua", ".m", ".mm", ".php", ".py", ".rb", ".rs", ".scala",
    ".sh", ".swift", ".ts", ".tsx", ".zig",
}
RECOVERABLE_DATA_SUFFIXES = {".json", ".jsonl", ".snap", ".toml", ".yaml", ".yml"}
RECOVERABLE_SCHEMA_SUFFIXES = {".json", ".proto", ".xsd", ".yaml", ".yml"}
STRING_FIELD = rb'"%s"\s*:\s*("(?:\\.|[^"\\])*")'
FORBIDDEN_METADATA_KEYS = [
    b'"message"',
    b'"content"',
    b'"tool"',
    b'"tool_calls"',
    b'"base_instructions"',
    b'"dynamic_tools"',
]
CANONICAL_CONTEXT_PATHS = (
    ".project-context/model.yaml",
    ".project-context/events.jsonl",
)
SYNTHETIC_USER_PREFIXES = (
    "<recommended_plugins>",
    "<environment_context>",
    "<skill>",
    "# AGENTS.md instructions",
    "The following is the Codex agent history",
    "<codex_delegation>",
)


class InventoryError(RuntimeError):
    pass


def run_git(root: Path, *arguments: str, check: bool = True) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if check and result.returncode != 0:
        raise InventoryError(result.stderr.strip() or "git command failed")
    return result.stdout


def evidence_pathspecs() -> list[str]:
    return [
        ".",
        ":(glob,exclude)**/.project-context/model.yaml",
        ":(glob,exclude)**/.project-context/events.jsonl",
    ]


def tracked_paths(root: Path) -> list[str]:
    return sorted(run_git(root, "ls-files", "--", *evidence_pathspecs()).splitlines())


def head_tracked_paths(root: Path) -> list[str]:
    return sorted(
        relative
        for relative in run_git(root, "ls-tree", "-r", "--name-only", "HEAD").splitlines()
        if not canonical_context_path(relative)
    )


def selected_tracked_paths(root: Path, include_worktree: bool) -> list[str]:
    return tracked_paths(root) if include_worktree else head_tracked_paths(root)


def document_path(relative: str) -> bool:
    path = Path(relative)
    return path.suffix.lower() in DOCUMENT_SUFFIXES or path.name.lower() in DOCUMENT_BASENAMES


def document_evidence_path(reference: str) -> str | None:
    if not reference.startswith("file:"):
        return None
    path = reference.removeprefix("file:").split("#", 1)[0]
    return path if document_path(path) else None


def recoverable_file_path(relative: str) -> bool:
    path = Path(relative)
    suffix = path.suffix.lower()
    components = {part.lower() for part in path.parts[:-1]}
    if suffix in RECOVERABLE_CODE_SUFFIXES:
        return True
    if path.name.lower().endswith(".schema.json"):
        return True
    if components & {"test", "tests"} and suffix in RECOVERABLE_DATA_SUFFIXES:
        return True
    return bool(components & {"schema", "schemas"} and suffix in RECOVERABLE_SCHEMA_SUFFIXES)


def document_blocks(text: str) -> list[tuple[int, int, str]]:
    blocks: list[tuple[int, int, str]] = []
    lines = text.split("\n")
    start: int | None = None
    content: list[str] = []
    for line_number, line in enumerate(lines, 1):
        if line.strip():
            if start is None:
                start = line_number
            content.append(line)
        elif start is not None:
            blocks.append((start, line_number - 1, "\n".join(content)))
            start = None
            content = []
    if start is not None:
        blocks.append((start, len(lines), "\n".join(content)))
    return blocks


def head_line_origins(root: Path, relative: str) -> tuple[list[str], dict[int, str]]:
    exists = subprocess.run(
        ["git", "-C", str(root), "cat-file", "-e", f"HEAD:{relative}"],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if exists.returncode != 0:
        return [], {}
    result = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "blame",
            "--line-porcelain",
            "--follow",
            "HEAD",
            "--",
            relative,
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        raise InventoryError(
            result.stderr.decode("utf-8", "replace").strip()
            or f"cannot determine document origins for {relative}"
        )
    origins: dict[int, str] = {}
    current_line: int | None = None
    for raw_line in result.stdout.decode("utf-8", "replace").splitlines():
        fields = raw_line.split()
        if (
            len(fields) >= 3
            and re.fullmatch(r"[0-9a-f]{40}", fields[0])
            and fields[2].isdigit()
        ):
            current_line = int(fields[2])
            origins[current_line] = fields[0]
        elif raw_line.startswith("\t"):
            current_line = None
    head = subprocess.run(
        ["git", "-C", str(root), "show", f"HEAD:{relative}"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if head.returncode != 0:
        raise InventoryError(
            head.stderr.decode("utf-8", "replace").strip()
            or f"cannot read HEAD document {relative}"
        )
    return head.stdout.decode("utf-8").split("\n"), origins


def selected_line_origins(
    root: Path, relative: str, text: str, include_worktree: bool
) -> dict[int, str | None]:
    head_lines, head_origins = head_line_origins(root, relative)
    selected_lines = text.split("\n")
    if not include_worktree:
        return {line: head_origins.get(line) for line in range(1, len(selected_lines) + 1)}
    mapped: dict[int, str | None] = {
        line: None for line in range(1, len(selected_lines) + 1)
    }
    matcher = difflib.SequenceMatcher(a=head_lines, b=selected_lines, autojunk=False)
    for head_start, selected_start, length in matcher.get_matching_blocks():
        for offset in range(length):
            mapped[selected_start + offset + 1] = head_origins.get(head_start + offset + 1)
    return mapped


def worktree_patch(root: Path) -> bytes:
    return subprocess.run(
        ["git", "-C", str(root), "diff", "--binary", "HEAD", "--", *evidence_pathspecs()],
        check=False,
        stdout=subprocess.PIPE,
    ).stdout


def canonical_context_path(relative: str) -> bool:
    normalized = relative.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return any(
        normalized == path or normalized.endswith(f"/{path}")
        for path in CANONICAL_CONTEXT_PATHS
    )


def text_content(value: Any) -> str:
    if isinstance(value, str):
        return value
    if not isinstance(value, list):
        return ""
    parts: list[str] = []
    for item in value:
        if isinstance(item, str):
            parts.append(item)
        elif isinstance(item, dict) and isinstance(item.get("text"), str):
            parts.append(item["text"])
    return "\n".join(parts)


def direct_user_message(provider: str, record: dict[str, Any]) -> str | None:
    record_type = record.get("type")
    payload = record.get("payload") if isinstance(record.get("payload"), dict) else {}
    text = ""
    if provider == "codex":
        if record_type == "response_item" and payload.get("type") == "message":
            if payload.get("role") == "user":
                text = text_content(payload.get("content"))
        elif record_type == "message" and payload.get("role") == "user":
            text = text_content(payload.get("content"))
    elif provider == "claude" and record_type == "user":
        message = record.get("message")
        if isinstance(message, dict):
            text = text_content(message.get("content"))
    text = text.strip()
    if not text or text.startswith(SYNTHETIC_USER_PREFIXES):
        return None
    return text


def contains_canonical_context_artifact(value: Any) -> bool:
    if isinstance(value, str):
        normalized = value.replace("\\", "/")
        return any(path in normalized for path in CANONICAL_CONTEXT_PATHS)
    if isinstance(value, list):
        return any(contains_canonical_context_artifact(item) for item in value)
    if isinstance(value, dict):
        return any(contains_canonical_context_artifact(item) for item in value.values())
    return False


def repository_identity(root: Path) -> tuple[Path, Path]:
    top = Path(run_git(root, "rev-parse", "--show-toplevel").strip()).resolve()
    common_text = run_git(top, "rev-parse", "--git-common-dir").strip()
    common = Path(common_text)
    if not common.is_absolute():
        common = top / common
    return top, common.resolve()


def same_repository(candidate: str, root: Path, common: Path) -> bool:
    if not candidate:
        return False
    try:
        resolved = Path(candidate).expanduser().resolve()
    except OSError:
        return False
    if resolved == root:
        return True
    result = subprocess.run(
        ["git", "-C", str(resolved), "rev-parse", "--git-common-dir"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    if result.returncode != 0:
        return False
    candidate_common = Path(result.stdout.strip())
    if not candidate_common.is_absolute():
        candidate_common = resolved / candidate_common
    try:
        return candidate_common.resolve() == common
    except OSError:
        return False


def regular_file(path: Path) -> bool:
    try:
        return stat.S_ISREG(path.lstat().st_mode)
    except OSError:
        return False


def regular_directory(path: Path) -> bool:
    try:
        return stat.S_ISDIR(path.lstat().st_mode)
    except OSError:
        return False


def path_identity(path: Path) -> tuple[int, int, int, int, int]:
    metadata = path.lstat()
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mtime_ns,
        stat.S_IMODE(metadata.st_mode),
    )


def fingerprint(path: Path) -> tuple[int, int, int, int, int]:
    identity = path_identity(path)
    if not regular_file(path):
        raise InventoryError(f"source is not a regular file: {path}")
    return identity


def local_adapter_status() -> dict[str, str]:
    home = Path.home()
    paths = {
        "codex_sessions": home / ".codex/sessions",
        "codex_archived_sessions": home / ".codex/archived_sessions",
        "claude_projects": home / ".claude/projects",
    }
    return {
        name: (
            "available"
            if regular_directory(path)
            else "unavailable"
            if path.exists() or path.is_symlink()
            else "absent"
        )
        for name, path in paths.items()
    }


def decode_json_string(raw: bytes) -> str | None:
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    return value if isinstance(value, str) else None


def metadata_fields(
    path: Path,
    required: set[str],
    one_of: set[str],
    optional: set[str],
) -> tuple[dict[str, str] | None, str | None]:
    """Read only the prefix needed for allowlisted association fields."""
    if not regular_file(path):
        return None, "not a regular file"
    patterns = {
        key: re.compile(STRING_FIELD % re.escape(key.encode()))
        for key in required | one_of | optional
    }
    buffer = bytearray()
    try:
        with path.open("rb", buffering=0) as stream:
            while len(buffer) < MAX_METADATA_BYTES:
                byte = stream.read(1)
                if not byte or byte == b"\n":
                    break
                buffer.extend(byte)
                fields: dict[str, str] = {}
                for key, pattern in patterns.items():
                    match = pattern.search(buffer)
                    if match:
                        value = decode_json_string(match.group(1))
                        if value is not None:
                            fields[key] = value
                if required <= fields.keys() and (not one_of or one_of & fields.keys()):
                    return fields, None
                if any(marker in buffer for marker in FORBIDDEN_METADATA_KEYS):
                    return None, "association metadata is absent before substantive content"
    except OSError as error:
        return None, str(error)
    return None, "bounded association metadata is missing or malformed"


def safe_session_id(candidates: Iterable[str]) -> str | None:
    return next((value for value in candidates if SESSION_ID_PATTERN.fullmatch(value)), None)


def candidate_record(
    provider: str,
    path: Path,
    root: Path,
    common: Path,
    require_type: bool,
) -> tuple[dict[str, Any] | None, str | None]:
    required: set[str] = set()
    one_of = {"cwd", "project_root"}
    optional = {"session_id", "sessionId", "id"}
    if require_type:
        required.add("type")
    fields, error = metadata_fields(path, required, one_of, optional)
    if fields is None:
        return None, error
    if require_type and fields.get("type") != "session_meta":
        return None, "first record is not session_meta"
    association = fields.get("cwd") or fields.get("project_root") or ""
    if not same_repository(association, root, common):
        return None, "association metadata belongs to another repository"
    session_id = safe_session_id(
        [
            fields.get("session_id", ""),
            fields.get("sessionId", ""),
            fields.get("id", ""),
            path.stem,
        ]
    )
    if session_id is None:
        return None, "session ID is missing or invalid"
    return {
        "provider": provider,
        "session_id": session_id,
        "modified_ns": path.lstat().st_mtime_ns,
        "source": str(path),
        "fingerprint": fingerprint(path),
    }, None


def conversation_candidates(
    root: Path, common: Path
) -> tuple[list[dict[str, Any]], list[dict[str, str]]]:
    records: list[dict[str, Any]] = []
    unavailable: list[dict[str, str]] = []
    home = Path.home()
    for provider, source, require_type in [
        ("codex", home / ".codex/sessions", True),
        ("codex", home / ".codex/archived_sessions", True),
    ]:
        if not regular_directory(source):
            continue
        for path in sorted(source.rglob("*.jsonl")):
            record, error = candidate_record(provider, path, root, common, require_type)
            if record is not None:
                records.append(record)
            elif error and "another repository" not in error:
                unavailable.append({"provider": provider, "reason": error})

    claude_root = home / ".claude/projects"
    encoded_roots = {str(root).replace(os.sep, "-")}
    for line in run_git(root, "worktree", "list", "--porcelain", check=False).splitlines():
        if line.startswith("worktree "):
            encoded_roots.add(line[9:].replace(os.sep, "-"))
    if regular_directory(claude_root):
        for project_directory in sorted(claude_root.iterdir()):
            if not regular_directory(project_directory) or project_directory.name not in encoded_roots:
                continue
            for path in sorted(project_directory.rglob("*.jsonl")):
                record, error = candidate_record("claude", path, root, common, False)
                if record is not None:
                    records.append(record)
                elif error and "another repository" not in error:
                    unavailable.append({"provider": "claude", "reason": error})
    return records, unavailable


def preflight(root: Path, include_git: bool, include_conversations: bool) -> dict[str, Any]:
    root, common = repository_identity(root)
    sessions: list[dict[str, Any]] = []
    unavailable: list[dict[str, str]] = []
    if include_conversations:
        sessions, unavailable = conversation_candidates(root, common)
    session_keys = sorted({(item["provider"], item["session_id"]) for item in sessions})
    report: dict[str, Any] = {
        "repository": str(root),
        "selected": {"git": include_git, "conversations": include_conversations},
    }
    if include_git:
        report.update(
            {
                "reachable_ref_count": len(
                    run_git(root, "for-each-ref", "--format=%(refname)").splitlines()
                ),
                "reachable_commit_count": len(
                    set(run_git(root, "rev-list", "--all").splitlines())
                ),
                "optional_sources": {
                    "reflog_commit_count": len(
                        set(run_git(root, "rev-list", "--reflog", check=False).splitlines())
                    ),
                    "initialized_submodule_count": sum(
                        1
                        for line in run_git(
                            root, "submodule", "status", "--recursive", check=False
                        ).splitlines()
                        if line and not line.startswith("-")
                    ),
                    "tracked_worktree_changed_path_count": len(
                        set(
                            run_git(
                                root,
                                "diff",
                                "--name-only",
                                "HEAD",
                                "--",
                                *evidence_pathspecs(),
                                check=False,
                            ).splitlines()
                        )
                    ),
                    "non_ignored_untracked_path_count": len(
                        set(
                            relative
                            for relative in run_git(
                                root, "ls-files", "--others", "--exclude-standard"
                            ).splitlines()
                            if not canonical_context_path(relative)
                        )
                    ),
                },
            }
        )
    if include_conversations:
        report.update(
            {
                "conversation_counts": {
                    provider: sum(1 for key in session_keys if key[0] == provider)
                    for provider in ("codex", "claude")
                },
                "conversations": [
                    {
                        "provider": provider,
                        "session_id": session_id,
                        "modified_ns": max(
                            item["modified_ns"]
                            for item in sessions
                            if (item["provider"], item["session_id"])
                            == (provider, session_id)
                        ),
                    }
                    for provider, session_id in session_keys
                ],
                "conversation_adapter_status": local_adapter_status(),
                "unavailable_conversation_candidate_count": len(unavailable),
            }
        )
    return report


def write_jsonl(path: Path, records: Iterable[dict[str, Any]]) -> int:
    count = 0
    with path.open("w", encoding="utf-8", newline="\n") as stream:
        for record in records:
            stream.write(
                json.dumps(record, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
            )
            stream.write("\n")
            count += 1
    return count


def selected_revisions(root: Path, reflog: bool, unreachable: bool) -> set[str]:
    revisions = set(run_git(root, "rev-list", "--all").splitlines())
    if reflog or unreachable:
        revisions.update(run_git(root, "rev-list", "--reflog", check=False).splitlines())
    if unreachable:
        for line in run_git(
            root, "fsck", "--unreachable", "--no-reflogs", check=False
        ).splitlines():
            fields = line.split()
            if len(fields) == 3 and fields[1] == "commit":
                revisions.add(fields[2])
    return revisions


def selected_repositories(root: Path, args: argparse.Namespace) -> list[tuple[str, Path, set[str]]]:
    repositories = [
        (".", root, selected_revisions(root, args.include_reflog, args.include_unreachable))
    ]
    if args.include_submodules:
        for line in run_git(root, "submodule", "status", "--recursive", check=False).splitlines():
            fields = line.lstrip("-+U ").split()
            if len(fields) < 2:
                continue
            submodule = root / fields[1]
            if regular_directory(submodule) and run_git(
                submodule, "rev-parse", "--is-inside-work-tree", check=False
            ).strip() == "true":
                repositories.append(
                    (fields[1], submodule, set(run_git(submodule, "rev-list", "--all").splitlines()))
                )
    return repositories


def collect_commits(
    output: Path, repositories: list[tuple[str, Path, set[str]]]
) -> tuple[int, int]:
    inventory: list[dict[str, Any]] = []
    coverage: list[dict[str, Any]] = []
    patches = output / "patches"
    patches.mkdir(mode=0o700)
    for repository_name, repository, revisions in repositories:
        for commit in sorted(revisions):
            source = f"commit:{repository_name}:{commit}"
            metadata = run_git(
                repository,
                "show",
                "-s",
                "--format=%H%x00%aI%x00%s",
                commit,
                check=False,
            ).rstrip("\n").split("\0", 2)
            if len(metadata) != 3:
                inventory.append({"repository": repository_name, "commit": commit, "patch": None})
                coverage.append(
                    {"source": source, "status": "unavailable", "reason": "metadata unavailable"}
                )
                continue
            patch = subprocess.run(
                [
                    "git",
                    "-C",
                    str(repository),
                    "show",
                    "--format=",
                    "--binary",
                    commit,
                    "--",
                    *evidence_pathspecs(),
                ],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            patch_name = f"{hashlib.sha256(repository_name.encode()).hexdigest()[:12]}-{commit}.patch"
            if patch.returncode == 0:
                (patches / patch_name).write_bytes(patch.stdout)
                coverage.append({"source": source, "status": "pending"})
            else:
                coverage.append(
                    {
                        "source": source,
                        "status": "unavailable",
                        "reason": patch.stderr.decode("utf-8", "replace").strip()
                        or "patch unavailable",
                    }
                )
            inventory.append(
                {
                    "repository": repository_name,
                    "commit": metadata[0],
                    "date": metadata[1],
                    "subject": metadata[2],
                    "patch": f"patches/{patch_name}" if patch.returncode == 0 else None,
                }
            )
    return (
        write_jsonl(output / "commits.jsonl", inventory),
        write_jsonl(output / "commit-coverage.jsonl", coverage),
    )


def collect_conversations(
    output: Path, sessions: list[dict[str, Any]]
) -> tuple[int, int, int, list[dict[str, Any]]]:
    inventory: list[dict[str, Any]] = []
    coverage: list[dict[str, Any]] = []
    decision_coverage: list[dict[str, Any]] = []
    session_reports: list[dict[str, Any]] = []
    for provider, session_id in sorted(
        {(item["provider"], item["session_id"]) for item in sessions}
    ):
        paths = sorted(
            [
                item
                for item in sessions
                if item["provider"] == provider and item["session_id"] == session_id
            ],
            key=lambda item: item["source"],
        )
        index = 0
        frozen_bytes = 0
        for item in paths:
            path = Path(item["source"])
            try:
                stream = path.open("rb")
            except OSError as error:
                coverage.append(
                    {
                        "source": f"conversation:{provider}:{session_id}#segment",
                        "status": "unavailable",
                        "reason": str(error),
                    }
                )
                continue
            with stream:
                for line in stream:
                    evidence = f"conversation:{provider}:{session_id}#{index}"
                    index += 1
                    if len(line) > MAX_RECORD_BYTES:
                        coverage.append(
                            {"source": evidence, "status": "unavailable", "reason": "oversized record"}
                        )
                        continue
                    try:
                        value = json.loads(line)
                    except (UnicodeDecodeError, json.JSONDecodeError):
                        coverage.append(
                            {"source": evidence, "status": "unavailable", "reason": "malformed JSONL record"}
                        )
                        continue
                    user_message = (
                        direct_user_message(provider, value) if isinstance(value, dict) else None
                    )
                    if user_message is None and contains_canonical_context_artifact(value):
                        inventory.append(
                            {
                                "evidence": evidence,
                                "record": {"type": "canonical-context-artifact-redacted"},
                            }
                        )
                        coverage.append(
                            {
                                "source": evidence,
                                "status": "excluded",
                                "reason": "Canonical model or event content is not reconstruction evidence.",
                            }
                        )
                    else:
                        inventory.append({"evidence": evidence, "record": value})
                        coverage.append({"source": evidence, "status": "pending"})
                    if user_message is not None:
                        decision_coverage.append({"source": evidence, "status": "pending"})
            frozen_bytes += item["cutoff_bytes"]
        session_reports.append(
            {
                "provider": provider,
                "session_id": session_id,
                "records": index,
                "frozen_bytes": frozen_bytes,
                "frozen_segments": [
                    {
                        "device": item["fingerprint"][0],
                        "inode": item["fingerprint"][1],
                        "mode": item["fingerprint"][4],
                        "cutoff_bytes": item["cutoff_bytes"],
                        "prefix_sha256": item["prefix_sha256"],
                    }
                    for item in paths
                ],
            }
        )
    return (
        write_jsonl(output / "conversations.jsonl", inventory),
        write_jsonl(output / "conversation-coverage.jsonl", coverage),
        write_jsonl(output / "decision-coverage.jsonl", decision_coverage),
        session_reports,
    )


def frozen_prefix(path: Path, cutoff: int) -> tuple[bytes, str]:
    try:
        with path.open("rb", buffering=0) as stream:
            content = stream.read(cutoff)
    except OSError as error:
        raise InventoryError(f"cannot freeze conversation source: {error}") from error
    if len(content) != cutoff:
        raise InventoryError("conversation source was truncated during freeze")
    return content, hashlib.sha256(content).hexdigest()


def mutate_frozen_source_for_test(path: Path) -> None:
    if os.environ.get("PROJECT_CONTEXT_INVENTORY_TESTING") != "1":
        return
    mode = os.environ.pop("PROJECT_CONTEXT_INVENTORY_TEST_MUTATE_SESSION", "")
    if not mode:
        return
    if mode in {"1", "append"}:
        with path.open("ab") as stream:
            stream.write(b'{"type":"appended-after-cutoff"}\n')
    elif mode == "prefix":
        with path.open("r+b") as stream:
            first = stream.read(1)
            stream.seek(0)
            stream.write(b"[" if first != b"[" else b"{")
    elif mode == "truncate":
        with path.open("r+b") as stream:
            stream.truncate(max(0, path.stat().st_size - 1))
    elif mode == "replace":
        replacement = path.with_name(path.name + ".replacement")
        replacement.write_bytes(path.read_bytes())
        os.replace(replacement, path)
    elif mode == "remove":
        path.unlink()
    else:
        raise InventoryError(f"unknown test conversation mutation: {mode}")


def freeze_conversation_sources(
    output: Path, sessions: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    snapshots = output / "conversation-sources"
    snapshots.mkdir(mode=0o700)
    frozen: list[dict[str, Any]] = []
    for index, item in enumerate(sessions):
        source = Path(item["source"])
        initial = tuple(item["fingerprint"])
        cutoff = initial[2]
        content, digest = frozen_prefix(source, cutoff)
        mutate_frozen_source_for_test(source)
        try:
            current = fingerprint(source)
        except (OSError, InventoryError) as error:
            raise InventoryError(f"conversation source changed during freeze: {error}") from error
        if current[0] != initial[0] or current[1] != initial[1] or current[4] != initial[4]:
            raise InventoryError("conversation source identity changed during freeze")
        if current[2] < cutoff:
            raise InventoryError("conversation source was truncated during freeze")
        _, current_digest = frozen_prefix(source, cutoff)
        if current_digest != digest:
            raise InventoryError("conversation source prefix changed during freeze")
        snapshot = snapshots / f"{index:06d}-{digest}.jsonl"
        snapshot.write_bytes(content)
        os.chmod(snapshot, 0o600)
        frozen.append(
            {
                **item,
                "source": str(snapshot),
                "original_source": str(source),
                "cutoff_bytes": cutoff,
                "prefix_sha256": digest,
            }
        )
    return frozen


def safe_repository_path(root: Path, relative: str) -> Path:
    candidate = root / relative
    try:
        candidate.resolve().relative_to(root)
    except (OSError, ValueError) as error:
        raise InventoryError(f"unsafe repository path {relative}: {error}") from error
    return candidate


def collect_untracked(
    output: Path, root: Path, paths: list[str]
) -> tuple[int, dict[str, tuple[int, int, int, int, int]]]:
    snapshots = output / "untracked"
    snapshots.mkdir(mode=0o700)
    inventory: list[dict[str, Any]] = []
    coverage: list[dict[str, Any]] = []
    identities: dict[str, tuple[int, int, int, int, int]] = {}
    for relative in paths:
        source = f"untracked:{relative}"
        path = safe_repository_path(root, relative)
        identities[relative] = path_identity(path)
        if not regular_file(path):
            inventory.append({"path": relative, "snapshot": None})
            coverage.append(
                {"source": source, "status": "unavailable", "reason": "not a regular file"}
            )
            continue
        before = fingerprint(path)
        if before[2] > MAX_UNTRACKED_BYTES:
            inventory.append({"path": relative, "snapshot": None, "bytes": before[2]})
            coverage.append(
                {"source": source, "status": "unavailable", "reason": "oversized file"}
            )
            continue
        content = path.read_bytes()
        if fingerprint(path) != before:
            raise InventoryError("untracked source changed during collection")
        name = f"{hashlib.sha256(relative.encode()).hexdigest()}.bin"
        (snapshots / name).write_bytes(content)
        inventory.append({"path": relative, "snapshot": f"untracked/{name}", "bytes": len(content)})
        coverage.append({"source": source, "status": "pending"})
    write_jsonl(output / "untracked.jsonl", inventory)
    return write_jsonl(output / "untracked-coverage.jsonl", coverage), identities


def collect_documents(
    output: Path, root: Path, include_worktree: bool
) -> tuple[int, int]:
    snapshots = output / "documents"
    snapshots.mkdir(mode=0o700)
    inventory: list[dict[str, Any]] = []
    coverage: list[dict[str, Any]] = []
    candidates = selected_tracked_paths(root, include_worktree)
    for relative in candidates:
        if not document_path(relative) or canonical_context_path(relative):
            continue
        source = f"file:{relative}"
        if include_worktree:
            path = safe_repository_path(root, relative)
            if not regular_file(path):
                inventory.append(
                    {
                        "path": relative,
                        "snapshot": None,
                        "unavailable_reason": "not a regular file",
                    }
                )
                coverage.append(
                    {"source": source, "status": "unavailable", "reason": "not a regular file"}
                )
                continue
            before = fingerprint(path)
            if before[2] > MAX_DOCUMENT_BYTES:
                inventory.append(
                    {
                        "path": relative,
                        "snapshot": None,
                        "bytes": before[2],
                        "unavailable_reason": "oversized document",
                    }
                )
                coverage.append(
                    {"source": source, "status": "unavailable", "reason": "oversized document"}
                )
                continue
            content = path.read_bytes()
            if fingerprint(path) != before:
                raise InventoryError("tracked document changed during collection")
        else:
            tree = subprocess.run(
                ["git", "-C", str(root), "ls-tree", "-z", "HEAD", "--", relative],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            mode = tree.stdout.split(b" ", 1)[0] if tree.returncode == 0 else b""
            if mode not in {b"100644", b"100755"}:
                inventory.append(
                    {
                        "path": relative,
                        "snapshot": None,
                        "unavailable_reason": "not a regular file in HEAD",
                    }
                )
                coverage.append(
                    {
                        "source": source,
                        "status": "unavailable",
                        "reason": "not a regular file in HEAD",
                    }
                )
                continue
            result = subprocess.run(
                ["git", "-C", str(root), "show", f"HEAD:{relative}"],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            if result.returncode != 0:
                reason = (
                    result.stderr.decode("utf-8", "replace").strip()
                    or "document unavailable"
                )
                inventory.append(
                    {
                        "path": relative,
                        "snapshot": None,
                        "unavailable_reason": reason,
                    }
                )
                coverage.append(
                    {
                        "source": source,
                        "status": "unavailable",
                        "reason": reason,
                    }
                )
                continue
            content = result.stdout
            if len(content) > MAX_DOCUMENT_BYTES:
                inventory.append(
                    {
                        "path": relative,
                        "snapshot": None,
                        "bytes": len(content),
                        "unavailable_reason": "oversized document",
                    }
                )
                coverage.append(
                    {"source": source, "status": "unavailable", "reason": "oversized document"}
                )
                continue
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError:
            inventory.append(
                {
                    "path": relative,
                    "snapshot": None,
                    "bytes": len(content),
                    "unavailable_reason": "document is not UTF-8",
                }
            )
            coverage.append(
                {"source": source, "status": "unavailable", "reason": "document is not UTF-8"}
            )
            continue
        digest = hashlib.sha256(content).hexdigest()
        snapshot = f"documents/{hashlib.sha256(relative.encode()).hexdigest()}-{digest}.txt"
        snapshot_path = output / snapshot
        snapshot_path.write_bytes(content)
        os.chmod(snapshot_path, 0o600)
        line_origins = selected_line_origins(root, relative, text, include_worktree)
        blocks = []
        for start, end, _ in document_blocks(text):
            block_source = f"file:{relative}#L{start}-L{end}"
            origins = [
                {"line": line, "commit": line_origins.get(line)}
                for line in range(start, end + 1)
                if text.split("\n")[line - 1].strip()
            ]
            blocks.append(
                {
                    "source": block_source,
                    "start": start,
                    "end": end,
                    "line_origins": origins,
                }
            )
            coverage.append({"source": block_source, "status": "pending"})
        inventory.append(
            {
                "path": relative,
                "snapshot": snapshot,
                "bytes": len(content),
                "sha256": digest,
                "blocks": blocks,
            }
        )
    return write_jsonl(output / "documents.jsonl", inventory), write_jsonl(
        output / "document-coverage.jsonl", coverage
    )


def collect(args: argparse.Namespace) -> dict[str, Any]:
    if not args.include_git and not args.include_conversations:
        raise InventoryError("select --include-git, --include-conversations, or both")
    if not args.include_git and any(
        [args.include_reflog, args.include_unreachable, args.include_submodules,
         args.include_worktree, args.include_untracked]
    ):
        raise InventoryError("optional Git sources require --include-git")
    root, common = repository_identity(args.root)
    output = args.output
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    os.chmod(output, 0o700)

    sessions, unavailable_candidates = (
        conversation_candidates(root, common) if args.include_conversations else ([], [])
    )
    if args.include_conversations:
        sessions = freeze_conversation_sources(output, sessions)
    repositories = selected_repositories(root, args) if args.include_git else []
    commit_count, commit_coverage_count = collect_commits(output, repositories)
    (
        conversation_count,
        conversation_coverage_count,
        decision_coverage_count,
        session_reports,
    ) = collect_conversations(output, sessions)
    for item in sessions:
        snapshot = Path(item["source"])
        if snapshot.parent == output / "conversation-sources":
            snapshot.unlink()
    snapshots = output / "conversation-sources"
    if snapshots.exists():
        snapshots.rmdir()
    tracked = selected_tracked_paths(root, args.include_worktree) if args.include_git else []
    (output / "tracked-paths.json").write_text(
        json.dumps(tracked, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    if args.include_git:
        document_count, document_coverage_count = collect_documents(
            output, root, args.include_worktree
        )
    else:
        document_count, document_coverage_count = 0, 0
    worktree = b""
    if args.include_worktree:
        worktree = worktree_patch(root)
        (output / "worktree.patch").write_bytes(worktree)
    untracked = (
        sorted(
            relative
            for relative in run_git(
                root, "ls-files", "--others", "--exclude-standard"
            ).splitlines()
            if not canonical_context_path(relative)
        )
        if args.include_untracked
        else []
    )
    if args.include_untracked:
        untracked_coverage_count, untracked_identities = collect_untracked(output, root, untracked)
    else:
        untracked_coverage_count, untracked_identities = 0, {}

    if args.include_git:
        current_repositories = selected_repositories(root, args)
        frozen = [(name, revisions) for name, _, revisions in repositories]
        current = [(name, revisions) for name, _, revisions in current_repositories]
        if current != frozen or selected_tracked_paths(root, args.include_worktree) != tracked:
            raise InventoryError("Git sources changed during collection")
    if args.include_worktree:
        current_worktree = worktree_patch(root)
        if current_worktree != worktree:
            raise InventoryError("tracked worktree changed during collection")
    if args.include_untracked:
        current_untracked = sorted(
            relative
            for relative in run_git(
                root, "ls-files", "--others", "--exclude-standard"
            ).splitlines()
            if not canonical_context_path(relative)
        )
        if current_untracked != untracked:
            raise InventoryError("untracked source set changed during collection")
        current_identities = {
            relative: path_identity(safe_repository_path(root, relative))
            for relative in current_untracked
        }
        if current_identities != untracked_identities:
            raise InventoryError("untracked sources changed during collection")
    summary = {
        "repository": str(root),
        "git_common_directory_hash": hashlib.sha256(str(common).encode()).hexdigest(),
        "selected": {
            "git": args.include_git,
            "conversations": args.include_conversations,
            "reflog": args.include_reflog,
            "unreachable": args.include_unreachable,
            "submodules": args.include_submodules,
            "tracked_worktree": args.include_worktree,
            "non_ignored_untracked": args.include_untracked,
        },
        "counts": {
            "commits": commit_count,
            "commit_coverage": commit_coverage_count,
            "conversation_records": conversation_coverage_count,
            "parseable_conversation_records": conversation_count,
            "decision_signals": decision_coverage_count,
            "tracked_paths": len(tracked),
            "documents": document_count,
            "document_blocks": document_coverage_count,
            "untracked_paths": len(untracked),
            "untracked_coverage": untracked_coverage_count,
        },
        "sessions": session_reports,
        "conversation_adapter_status": local_adapter_status(),
        "unavailable_conversation_candidate_count": len(unavailable_candidates),
    }
    (output / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    coverage_names = [
        "commit-coverage.jsonl",
        "conversation-coverage.jsonl",
        "decision-coverage.jsonl",
    ]
    if args.include_git:
        coverage_names.append("document-coverage.jsonl")
    if args.include_untracked:
        coverage_names.append("untracked-coverage.jsonl")
    source_manifest: dict[str, list[str]] = {}
    for name in coverage_names:
        sources = [record.get("source") for record in load_jsonl(output / name)]
        if any(not isinstance(source, str) for source in sources) or len(set(sources)) != len(sources):
            raise InventoryError(f"{name} produced a missing or duplicate source")
        source_manifest[name] = sources
    (output / "coverage-sources.json").write_text(
        json.dumps(
            {
                "version": 3,
                "sources": source_manifest,
                "frozen_artifacts": {
                    "documents.jsonl": hashlib.sha256(
                        (output / "documents.jsonl").read_bytes()
                    ).hexdigest()
                }
                if args.include_git
                else {},
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return summary


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for index, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            raise InventoryError(f"invalid coverage JSON at {path}:{index}: {error}") from error
        if not isinstance(value, dict):
            raise InventoryError(f"coverage record is not an object at {path}:{index}")
        records.append(value)
    return records


def verify_document_coverage(
    inventory: Path, summary: dict[str, Any], manifest: dict[str, Any]
) -> dict[str, int]:
    records = load_jsonl(inventory / "document-coverage.jsonl")
    documents = load_jsonl(inventory / "documents.jsonl")
    if len(documents) != summary["counts"]["documents"]:
        raise InventoryError("documents.jsonl count does not match frozen inventory")
    inventory_sources: set[str] = set()
    document_paths_seen: set[str] = set()
    for document in documents:
        relative = document.get("path")
        if not isinstance(relative, str) or relative in document_paths_seen:
            raise InventoryError("documents.jsonl has a missing or duplicate path")
        document_paths_seen.add(relative)
        snapshot = document.get("snapshot")
        if snapshot is None:
            inventory_sources.add(f"file:{relative}")
            continue
        if not isinstance(snapshot, str) or not snapshot.startswith("documents/"):
            raise InventoryError("documents.jsonl has an invalid snapshot path")
        snapshot_path = inventory / snapshot
        try:
            snapshot_path.resolve().relative_to((inventory / "documents").resolve())
        except (OSError, ValueError) as error:
            raise InventoryError(f"documents.jsonl has an unsafe snapshot path: {error}") from error
        content = snapshot_path.read_bytes()
        if len(content) != document.get("bytes"):
            raise InventoryError("frozen document size changed")
        if hashlib.sha256(content).hexdigest() != document.get("sha256"):
            raise InventoryError("frozen document digest changed")
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError as error:
            raise InventoryError("frozen document is no longer UTF-8") from error
        expected_blocks = [
            (f"file:{relative}#L{start}-L{end}", start, end)
            for start, end, _ in document_blocks(text)
        ]
        recorded_blocks = document.get("blocks", [])
        recorded_shapes = [
            (block.get("source"), block.get("start"), block.get("end"))
            for block in recorded_blocks
            if isinstance(block, dict)
        ]
        if recorded_shapes != expected_blocks:
            raise InventoryError("frozen document blocks do not match its snapshot")
        lines = text.split("\n")
        for block in recorded_blocks:
            expected_lines = [
                line
                for line in range(block["start"], block["end"] + 1)
                if lines[line - 1].strip()
            ]
            origins = block.get("line_origins")
            if not isinstance(origins, list) or [
                origin.get("line") for origin in origins if isinstance(origin, dict)
            ] != expected_lines:
                raise InventoryError("document block line origins are incomplete")
            for origin in origins:
                commit = origin.get("commit")
                if commit is not None and not (
                    isinstance(commit, str) and re.fullmatch(r"[0-9a-f]{40}", commit)
                ):
                    raise InventoryError("document block has an invalid origin commit")
        inventory_sources.update(source for source, _, _ in expected_blocks)
    expected = summary["counts"]["document_blocks"]
    if len(records) != expected:
        raise InventoryError("document-coverage.jsonl count does not match frozen inventory")
    manifest_sources = manifest["sources"].get("document-coverage.jsonl")
    sources: set[str] = set()
    totals = {
        "model": 0,
        "decision": 0,
        "attempt": 0,
        "recoverable": 0,
        "excluded": 0,
        "unavailable": 0,
    }
    tracked = set(json.loads((inventory / "tracked-paths.json").read_text(encoding="utf-8")))
    document_paths = {
        record["path"]
        for record in load_jsonl(inventory / "documents.jsonl")
        if isinstance(record.get("path"), str)
    }
    frozen_blocks = {
        block["source"]: block
        for document in documents
        for block in document.get("blocks", [])
        if isinstance(block, dict) and isinstance(block.get("source"), str)
    }
    analyzed_commits = {
        record["source"].rsplit(":", 1)[-1]
        for record in load_jsonl(inventory / "commit-coverage.jsonl")
        if record.get("status") == "analyzed" and isinstance(record.get("source"), str)
    }
    patch_commits = {
        record["commit"]
        for record in load_jsonl(inventory / "commits.jsonl")
        if isinstance(record.get("commit"), str) and isinstance(record.get("patch"), str)
    }
    direct_user = {
        record["source"]: record
        for record in load_jsonl(inventory / "decision-coverage.jsonl")
        if isinstance(record.get("source"), str)
    }
    for record in records:
        source = record.get("source")
        status_value = record.get("status")
        if not isinstance(source, str) or source in sources:
            raise InventoryError("document-coverage.jsonl has a missing or duplicate source")
        sources.add(source)
        if status_value not in totals:
            raise InventoryError(
                f"document-coverage.jsonl contains unresolved status {status_value!r}"
            )
        if status_value in {"model", "decision", "attempt", "recoverable"} and not record.get(
            "topic"
        ):
            raise InventoryError(f"document-coverage.jsonl requires a topic for {status_value}")
        if status_value == "model":
            if not (
                isinstance(record.get("candidate"), str)
                and MODEL_CANDIDATE_PATTERN.fullmatch(record["candidate"])
            ):
                raise InventoryError(
                    "document-coverage.jsonl requires a section:id candidate for model"
                )
            if not isinstance(record.get("statement"), str) or not record["statement"].strip():
                raise InventoryError("document-coverage.jsonl requires statement for model")
        if status_value in {"decision", "attempt"} and not (
            isinstance(record.get("candidate"), str)
            and CANDIDATE_ID_PATTERN.fullmatch(record["candidate"])
        ):
            raise InventoryError(
                f"document-coverage.jsonl requires a candidate ID for {status_value}"
            )
        if status_value == "decision" and not record.get("rationale"):
            raise InventoryError("document-coverage.jsonl requires rationale for decision")
        if status_value == "attempt" and not record.get("finding"):
            raise InventoryError("document-coverage.jsonl requires finding for attempt")
        if status_value in {"model", "decision", "attempt"}:
            supported_by = record.get("supported_by")
            if not isinstance(supported_by, list) or not supported_by or any(
                not isinstance(reference, str) for reference in supported_by
            ):
                raise InventoryError(
                    f"document-coverage.jsonl requires supported_by for {status_value}"
                )
            block = frozen_blocks.get(source, {})
            origins = block.get("line_origins", [])
            origin_commits = {
                origin["commit"]
                for origin in origins
                if isinstance(origin, dict) and isinstance(origin.get("commit"), str)
            }
            uncovered = any(
                isinstance(origin, dict) and origin.get("commit") is None
                for origin in origins
            )
            supplied_commits = {
                reference.removeprefix("commit:")
                for reference in supported_by
                if reference.startswith("commit:")
            }
            invalid_commits = supplied_commits - origin_commits
            unavailable_commits = supplied_commits - (analyzed_commits & patch_commits)
            if invalid_commits or unavailable_commits:
                raise InventoryError(
                    f"document {source} uses unrelated or unavailable origin commits"
                )
            conversation_support = False
            for reference in supported_by:
                if reference.startswith("commit:"):
                    continue
                signal = direct_user.get(reference)
                if (
                    signal is None
                    or signal.get("status") != status_value
                    or signal.get("candidate") != record.get("candidate")
                ):
                    raise InventoryError(
                        f"document {source} has unsupported direct-user evidence {reference}"
                    )
                conversation_support = True
            commit_complete = (
                not uncovered
                and origin_commits <= supplied_commits
                and origin_commits <= analyzed_commits
                and origin_commits <= patch_commits
            )
            if not commit_complete and not conversation_support:
                raise InventoryError(
                    f"document {source} lacks complete origin or direct-user support"
                )
        if status_value == "recoverable":
            recovered_by = record.get("recovered_by")
            if not isinstance(recovered_by, list) or not recovered_by:
                raise InventoryError("document-coverage.jsonl requires recovered_by references")
            for reference in recovered_by:
                if not isinstance(reference, str):
                    raise InventoryError("document recovered_by reference is not a string")
                if reference.startswith("file:"):
                    path = reference.removeprefix("file:").split("#", 1)[0]
                    if path not in tracked:
                        raise InventoryError(
                            f"document recovered_by uses file outside frozen inventory: {path}"
                        )
                    if path in document_paths:
                        raise InventoryError(
                            "document recovered_by must cite code, tests, or schema"
                        )
                    if not recoverable_file_path(path):
                        raise InventoryError(
                            "document recovered_by must cite current code, tests, or schema"
                        )
                else:
                    raise InventoryError(
                        f"document recovered_by has unsupported ref: {reference}"
                    )
        if status_value == "excluded":
            reason_code = record.get("reason_code")
            if reason_code not in DOCUMENT_EXCLUSION_REASONS or not record.get("reason"):
                raise InventoryError(
                    "document-coverage.jsonl requires an allowed exclusion reason code and reason"
                )
            if reason_code == "duplicate_within_document":
                duplicate_of = record.get("duplicate_of")
                if duplicate_of == source or duplicate_of not in inventory_sources:
                    raise InventoryError(
                        "duplicate document exclusion requires another frozen block"
                    )
        if status_value == "unavailable":
            document = next(
                (
                    item
                    for item in documents
                    if f"file:{item.get('path')}" == source
                ),
                None,
            )
            if (
                document is None
                or document.get("snapshot") is not None
                or record.get("reason") != document.get("unavailable_reason")
            ):
                raise InventoryError("document unavailable status is not collector-owned")
        totals[status_value] += 1
    if (
        not isinstance(manifest_sources, list)
        or len(manifest_sources) != len(records)
        or len(set(manifest_sources)) != len(manifest_sources)
        or sources != set(manifest_sources)
    ):
        raise InventoryError("document-coverage.jsonl sources do not match frozen manifest")
    if sources != inventory_sources:
        raise InventoryError("document-coverage.jsonl sources do not match frozen documents")
    return totals


def verify_coverage(inventory: Path) -> dict[str, Any]:
    summary = json.loads((inventory / "summary.json").read_text(encoding="utf-8"))
    specifications = [
        ("commit-coverage.jsonl", summary["counts"]["commits"]),
        ("conversation-coverage.jsonl", summary["counts"]["conversation_records"]),
    ]
    if summary["selected"]["non_ignored_untracked"]:
        specifications.append(
            ("untracked-coverage.jsonl", summary["counts"]["untracked_coverage"])
        )
    manifest = json.loads((inventory / "coverage-sources.json").read_text(encoding="utf-8"))
    if manifest.get("version") != 3:
        raise InventoryError(
            "coverage source manifest is unsupported; recollect the temporary inventory"
        )
    if not isinstance(manifest.get("sources"), dict):
        raise InventoryError("coverage source manifest is invalid")
    frozen_artifacts = manifest.get("frozen_artifacts")
    if not isinstance(frozen_artifacts, dict):
        raise InventoryError("coverage source manifest has no frozen artifact digests")
    if summary["selected"]["git"]:
        expected_digest = frozen_artifacts.get("documents.jsonl")
        actual_digest = hashlib.sha256((inventory / "documents.jsonl").read_bytes()).hexdigest()
        if expected_digest != actual_digest:
            raise InventoryError("documents.jsonl differs from the frozen manifest")
    expected_names = {name for name, _ in specifications} | {"decision-coverage.jsonl"}
    if summary["selected"]["git"]:
        expected_names.add("document-coverage.jsonl")
    if set(manifest["sources"]) != expected_names:
        raise InventoryError("coverage source manifest does not match selected inventories")
    totals = {"analyzed": 0, "excluded": 0, "unavailable": 0}
    for name, expected in specifications:
        records = load_jsonl(inventory / name)
        if len(records) != expected:
            raise InventoryError(f"{name} count does not match frozen inventory")
        sources: set[str] = set()
        for record in records:
            source = record.get("source")
            status_value = record.get("status")
            if not isinstance(source, str) or source in sources:
                raise InventoryError(f"{name} has a missing or duplicate source")
            sources.add(source)
            if status_value not in totals:
                raise InventoryError(f"{name} contains unresolved status {status_value!r}")
            if status_value in {"excluded", "unavailable"} and not record.get("reason"):
                raise InventoryError(f"{name} requires a reason for {status_value}")
            totals[status_value] += 1
        manifest_sources = manifest["sources"][name]
        if (
            not isinstance(manifest_sources, list)
            or any(not isinstance(source, str) for source in manifest_sources)
            or len(manifest_sources) != expected
            or len(set(manifest_sources)) != len(manifest_sources)
            or sources != set(manifest_sources)
        ):
            raise InventoryError(f"{name} sources do not match the frozen manifest")
    totals["total"] = sum(totals.values())
    decision_records = load_jsonl(inventory / "decision-coverage.jsonl")
    if len(decision_records) != summary["counts"]["decision_signals"]:
        raise InventoryError("decision-coverage.jsonl count does not match frozen inventory")
    decision_manifest = manifest["sources"]["decision-coverage.jsonl"]
    decision_sources: set[str] = set()
    decision_totals = {
        "decision": 0,
        "attempt": 0,
        "model": 0,
        "excluded": 0,
        "unavailable": 0,
    }
    for record in decision_records:
        source = record.get("source")
        status_value = record.get("status")
        if not isinstance(source, str) or source in decision_sources:
            raise InventoryError("decision-coverage.jsonl has a missing or duplicate source")
        decision_sources.add(source)
        if status_value not in decision_totals:
            raise InventoryError(
                f"decision-coverage.jsonl contains unresolved status {status_value!r}"
            )
        if status_value in {"decision", "attempt", "model"} and not record.get("topic"):
            raise InventoryError(f"decision-coverage.jsonl requires a topic for {status_value}")
        if status_value == "decision" and not record.get("rationale"):
            raise InventoryError("decision-coverage.jsonl requires rationale for decision")
        if status_value in {"decision", "attempt"} and not (
            isinstance(record.get("candidate"), str)
            and CANDIDATE_ID_PATTERN.fullmatch(record["candidate"])
        ):
            raise InventoryError(
                f"decision-coverage.jsonl requires a candidate ID for {status_value}"
            )
        if status_value == "attempt" and not record.get("finding"):
            raise InventoryError("decision-coverage.jsonl requires finding for attempt")
        if status_value == "model" and not (
            isinstance(record.get("candidate"), str)
            and MODEL_CANDIDATE_PATTERN.fullmatch(record["candidate"])
        ):
            raise InventoryError(
                "decision-coverage.jsonl requires a section:id candidate for model"
            )
        if status_value == "model" and not (
            isinstance(record.get("statement"), str) and record["statement"].strip()
        ):
            raise InventoryError(
                "decision-coverage.jsonl requires statement for model"
            )
        if status_value in {"excluded", "unavailable"} and not record.get("reason"):
            raise InventoryError(f"decision-coverage.jsonl requires a reason for {status_value}")
        decision_totals[status_value] += 1
    if (
        not isinstance(decision_manifest, list)
        or len(decision_manifest) != len(decision_records)
        or len(set(decision_manifest)) != len(decision_manifest)
        or decision_sources != set(decision_manifest)
    ):
        raise InventoryError("decision-coverage.jsonl sources do not match the frozen manifest")
    if not decision_sources <= set(manifest["sources"]["conversation-coverage.jsonl"]):
        raise InventoryError("decision coverage contains a non-conversation source")
    totals["decision_signals"] = decision_totals
    if summary["selected"]["git"]:
        totals["document_blocks"] = verify_document_coverage(inventory, summary, manifest)
    return totals


def verify_candidates(inventory: Path, events: Path) -> dict[str, Any]:
    coverage = verify_coverage(inventory)
    decision_records = load_jsonl(inventory / "decision-coverage.jsonl")
    statuses = {record["source"]: record["status"] for record in decision_records}
    required_records = [
        record
        for record in decision_records
        if record["status"] in {"decision", "attempt"}
    ]
    summary = json.loads((inventory / "summary.json").read_text(encoding="utf-8"))
    document_records = (
        load_jsonl(inventory / "document-coverage.jsonl") if summary["selected"]["git"] else []
    )
    required_document_events = [
        record for record in document_records if record["status"] in {"decision", "attempt"}
    ]
    event_records = load_jsonl(events)
    event_sources: set[str] = set()
    events_by_id: dict[str, dict[str, Any]] = {}
    evidence_by_id: dict[str, list[str]] = {}
    decision_event_count = 0

    def evidence_refs(value: Any, index: int) -> list[str]:
        if not isinstance(value, list):
            raise InventoryError(f"candidate event {index} has invalid evidence")
        refs: list[str] = []
        for item in value:
            if isinstance(item, str):
                refs.append(item)
            elif isinstance(item, dict) and isinstance(item.get("ref"), str):
                refs.append(item["ref"])
            else:
                raise InventoryError(f"candidate event {index} has invalid evidence")
        return refs

    for index, event in enumerate(event_records, 1):
        if event.get("type") == "decision":
            decision_event_count += 1
        evidence = event.get("evidence", [])
        refs = evidence_refs(evidence, index)
        if event.get("type") == "decision" and not event.get("reason"):
            raise InventoryError(f"candidate decision {index} has no reason")
        event_sources.update(refs)
        event_id = event.get("id")
        if isinstance(event_id, str):
            if event_id in events_by_id:
                raise InventoryError(f"duplicate candidate event ID {event_id}")
            events_by_id[event_id] = event
            evidence_by_id[event_id] = refs
    for record in required_records:
        source = record["source"]
        candidate = record["candidate"]
        event = events_by_id.get(candidate)
        expected_type = record["status"]
        if event is None or event.get("type") != expected_type:
            raise InventoryError(f"decision signal {source} references missing {candidate}")
        if source not in evidence_by_id.get(candidate, []):
            raise InventoryError(f"decision signal {source} is absent from {candidate} evidence")
        event_field = "reason" if expected_type == "decision" else "finding"
        audit_field = "rationale" if expected_type == "decision" else "finding"
        event_reason = " ".join(str(event.get(event_field, "")).split())
        audit_reason = " ".join(str(record[audit_field]).split())
        if event_reason != audit_reason:
            raise InventoryError(f"decision signal {source} rationale differs from {candidate}")
    for record in required_document_events:
        source = record["source"]
        candidate = record["candidate"]
        expected_type = record["status"]
        event = events_by_id.get(candidate)
        if event is None or event.get("type") != expected_type:
            raise InventoryError(
                f"document {expected_type} {source} references missing {candidate}"
            )
        support = record.get("supported_by", [])
        missing_support = [
            reference
            for reference in support
            if reference not in evidence_by_id.get(candidate, [])
        ]
        if missing_support:
            raise InventoryError(
                f"document {source} support is absent from {candidate} evidence"
            )
        field = "reason" if expected_type == "decision" else "finding"
        audit_field = "rationale" if expected_type == "decision" else "finding"
        actual = " ".join(str(event.get(field, "")).split())
        expected = " ".join(str(record[audit_field]).split())
        if actual != expected:
            raise InventoryError(
                f"document {expected_type} {source} {audit_field} differs from {candidate}"
            )
    contradicted = sorted(
        source
        for source in event_sources
        if source in statuses and statuses[source] not in {"decision", "attempt"}
    )
    if contradicted:
        raise InventoryError(
            "candidate event evidence contradicts decision coverage: "
            + ", ".join(contradicted)
        )
    document_statuses = {record["source"]: record["status"] for record in document_records}
    document_paths = {
        record["path"]
        for record in (
            load_jsonl(inventory / "documents.jsonl") if summary["selected"]["git"] else []
        )
        if isinstance(record.get("path"), str)
    }
    forbidden_document_sources = sorted(
        source
        for source in event_sources
        if document_evidence_path(source) is not None
    )
    if forbidden_document_sources:
        raise InventoryError(
            "candidate event evidence uses forbidden document references: "
            + ", ".join(forbidden_document_sources)
        )
    return {
        "candidate_events": len(event_records),
        "decision_events": decision_event_count,
        "decision_signals_covered": len(required_records),
        "document_event_signals_covered": len(required_document_events),
        "coverage": coverage,
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    commands = result.add_subparsers(dest="command", required=True)
    preflight_parser = commands.add_parser("preflight")
    preflight_parser.add_argument("--root", type=Path, required=True)
    preflight_parser.add_argument("--include-git", action="store_true")
    preflight_parser.add_argument("--include-conversations", action="store_true")
    collect_parser = commands.add_parser("collect")
    collect_parser.add_argument("--root", type=Path, required=True)
    collect_parser.add_argument("--output", type=Path, required=True)
    collect_parser.add_argument("--include-git", action="store_true")
    collect_parser.add_argument("--include-conversations", action="store_true")
    collect_parser.add_argument("--include-reflog", action="store_true")
    collect_parser.add_argument("--include-unreachable", action="store_true")
    collect_parser.add_argument("--include-submodules", action="store_true")
    collect_parser.add_argument("--include-worktree", action="store_true")
    collect_parser.add_argument("--include-untracked", action="store_true")
    verify_parser = commands.add_parser("verify-coverage")
    verify_parser.add_argument("--inventory", type=Path, required=True)
    candidate_parser = commands.add_parser("verify-candidates")
    candidate_parser.add_argument("--inventory", type=Path, required=True)
    candidate_parser.add_argument("--events", type=Path, required=True)
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "preflight":
            include_git = args.include_git
            include_conversations = args.include_conversations
            if not include_git and not include_conversations:
                include_git = include_conversations = True
            report = preflight(args.root, include_git, include_conversations)
        elif args.command == "collect":
            report = collect(args)
        elif args.command == "verify-coverage":
            report = verify_coverage(args.inventory)
        else:
            report = verify_candidates(args.inventory, args.events)
    except (InventoryError, OSError, KeyError, json.JSONDecodeError) as error:
        print(f"inventory: {error}", file=sys.stderr)
        return 2
    print(json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
