#!/bin/sh
set -eu

repository_root=$(
  CDPATH=
  cd -- "$(dirname -- "$0")/.."
  pwd
)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/project-context-reconstruction-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

fixture="${test_root}/collision/a-b/c"
home="${test_root}/home"
mkdir -p "$fixture" "$home/.codex/sessions" "$home/.codex/archived_sessions" "$home/.claude/projects"
fixture=$(CDPATH='' cd -- "$fixture" && pwd -P)
git -C "$fixture" init -q -b master
git -C "$fixture" config user.name Fixture
git -C "$fixture" config user.email fixture@example.invalid
printf 'first\n' > "${fixture}/source.txt"
printf 'ignored.log\n' > "${fixture}/.gitignore"
git -C "$fixture" add source.txt .gitignore
git -C "$fixture" commit -qm 'Add source'
git -C "$fixture" tag initial
git -C "$fixture" checkout -qb topic
git -C "$fixture" mv source.txt renamed.txt
git -C "$fixture" commit -qm 'Rename source'
git -C "$fixture" checkout -q master
printf 'main\n' > "${fixture}/main.txt"
git -C "$fixture" add main.txt
git -C "$fixture" commit -qm 'Add main path'
git -C "$fixture" merge -q --no-edit topic
git -C "$fixture" rm -q main.txt
git -C "$fixture" commit -qm 'Delete main path'
git -C "$fixture" revert --no-edit HEAD >/dev/null
printf 'temporary\n' > "${fixture}/unreachable.txt"
git -C "$fixture" add unreachable.txt
git -C "$fixture" commit -qm 'Create unreachable commit'
unreachable=$(git -C "$fixture" rev-parse HEAD)
git -C "$fixture" reset -q --hard HEAD~1
git -C "$fixture" reflog expire --expire=now --all
printf 'worktree\n' >> "${fixture}/renamed.txt"
printf 'untracked\n' > "${fixture}/visible.txt"
printf 'ignored\n' > "${fixture}/ignored.log"

submodule_source="${test_root}/submodule-source"
mkdir "$submodule_source"
git -C "$submodule_source" init -q -b master
git -C "$submodule_source" config user.name Fixture
git -C "$submodule_source" config user.email fixture@example.invalid
printf 'submodule\n' > "${submodule_source}/sub.txt"
git -C "$submodule_source" add sub.txt
git -C "$submodule_source" commit -qm 'Add submodule source'
git -C "$fixture" -c protocol.file.allow=always submodule add -q "$submodule_source" dependency
git -C "$fixture" commit -qm 'Add submodule'

worktree="${test_root}/worktree"
git -C "$fixture" worktree add -q -b linked-worktree "$worktree"

session_id=codex-related
session_file="${home}/.codex/sessions/${session_id}.jsonl"
printf '%s\n' \
  "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"${session_id}\",\"cwd\":\"${worktree}\",\"base_instructions\":{\"sentinel\":\"must-not-be-materialized\"}}}" \
  '{"type":"message","payload":{"role":"user","content":"durable choice"}}' \
  'malformed' > "$session_file"
python3 -c 'import sys; print("{\"type\":\"message\",\"payload\":{\"content\":\"" + "x" * (2 * 1024 * 1024 + 32) + "\"}}")' \
  >> "$session_file"
printf '%s\n' \
  "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"archived-related\",\"cwd\":\"${fixture}\"}}" \
  '{"type":"message","payload":{"role":"assistant","content":"archived"}}' \
  > "${home}/.codex/archived_sessions/archived.jsonl"
printf '%s\n' \
  '{"type":"session_meta","payload":{"session_id":"unrelated","cwd":"/tmp/unrelated-project"}}' \
  '{"type":"message","payload":{"content":"repository name appears only here"}}' \
  > "${home}/.codex/sessions/unrelated.jsonl"
printf '%s\n' \
  "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"project-root-only\",\"project_root\":\"${fixture}\"}}" \
  '{"type":"message","payload":{"content":"project root association"}}' \
  > "${home}/.codex/sessions/project-root-only.jsonl"

encoded_project=$(printf '%s' "$fixture" | tr / -)
mkdir -p "${home}/.claude/projects/${encoded_project}"
printf '%s\n' \
  "{\"type\":\"user\",\"cwd\":\"${fixture}\",\"message\":{\"content\":\"choice\"}}" \
  > "${home}/.claude/projects/${encoded_project}/claude-related.jsonl"
collision_a="$fixture"
collision_b="${test_root}/collision/a/b-c"
mkdir -p "$collision_b"
collision_b=$(CDPATH='' cd -- "$collision_b" && pwd -P)
git -C "$collision_b" init -q
collision_encoded=$(printf '%s' "$collision_a" | tr / -)
[ "$collision_encoded" = "$(printf '%s' "$collision_b" | tr / -)" ]
mkdir -p "${home}/.claude/projects/${collision_encoded}"
printf '%s\n' \
  "{\"type\":\"user\",\"cwd\":\"${collision_b}\",\"message\":{\"content\":\"unrelated collision\"}}" \
  > "${home}/.claude/projects/${collision_encoded}/collision.jsonl"

preflight="${test_root}/preflight.json"
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  preflight --root "$fixture" > "$preflight"
python3 - "$preflight" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
assert report["conversation_counts"] == {"codex": 3, "claude": 1}, report
assert all("source" not in conversation for conversation in report["conversations"])
assert report["conversation_adapter_status"]["codex_sessions"] == "available"
PY
if grep -Fq 'must-not-be-materialized' "$preflight"; then
  printf 'preflight exposed a non-association metadata body\n' >&2
  exit 1
fi

git_preflight="${test_root}/git-preflight.json"
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  preflight --root "$fixture" --include-git > "$git_preflight"
python3 - "$git_preflight" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
assert report["selected"] == {"git": True, "conversations": False}
assert "conversation_counts" not in report
PY

conversation_preflight="${test_root}/conversation-preflight.json"
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  preflight --root "$fixture" --include-conversations > "$conversation_preflight"
python3 - "$conversation_preflight" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
assert report["selected"] == {"git": False, "conversations": True}
assert "reachable_commit_count" not in report
PY

inventory="${test_root}/inventory"
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  collect --root "$fixture" --output "$inventory" \
  --include-git --include-conversations \
  --include-reflog --include-unreachable --include-submodules \
  --include-worktree --include-untracked >/dev/null
grep -Fq "$unreachable" "${inventory}/commits.jsonl"
grep -Fq '"repository":"dependency"' "${inventory}/commits.jsonl"
grep -Fq 'conversation:codex:codex-related#2' "${inventory}/conversation-coverage.jsonl"
grep -Fq '"status":"unavailable"' "${inventory}/conversation-coverage.jsonl"
grep -Fq 'visible.txt' "${inventory}/untracked.jsonl"
if grep -Fq 'ignored.log' "${inventory}/untracked.jsonl"; then
  printf 'ignored file was included in reconstruction inventory\n' >&2
  exit 1
fi
python3 - "$inventory" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
summary = json.loads((root / "summary.json").read_text())
coverage = [json.loads(line) for line in (root / "conversation-coverage.jsonl").read_text().splitlines()]
assert len(coverage) == summary["counts"]["conversation_records"]
assert "pending" in {item["status"] for item in coverage}
assert sum(item["records"] for item in summary["sessions"]) == len(coverage)
untracked = [json.loads(line) for line in (root / "untracked.jsonl").read_text().splitlines()]
visible = next(item for item in untracked if item["path"] == "visible.txt")
assert (root / visible["snapshot"]).read_text() == "untracked\n"
PY

set +e
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  verify-coverage --inventory "$inventory" >/dev/null 2>&1
pending_status=$?
set -e
[ "$pending_status" -eq 2 ]
python3 - "$inventory" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
for name in ["commit-coverage.jsonl", "conversation-coverage.jsonl", "untracked-coverage.jsonl"]:
    records = [json.loads(line) for line in (root / name).read_text().splitlines()]
    for record in records:
        if record["status"] == "pending":
            record["status"] = "analyzed"
    (root / name).write_text("".join(json.dumps(record, sort_keys=True) + "\n" for record in records))
PY
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  verify-coverage --inventory "$inventory" >/dev/null
cp "${inventory}/conversation-coverage.jsonl" "${inventory}/conversation-coverage.before"
python3 - "$inventory" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "conversation-coverage.jsonl"
records = [json.loads(line) for line in path.read_text().splitlines()]
records[0]["source"] = "conversation:codex:invented#0"
path.write_text("".join(json.dumps(record, sort_keys=True) + "\n" for record in records))
PY
set +e
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  verify-coverage --inventory "$inventory" >/dev/null 2>&1
substituted_source_status=$?
set -e
[ "$substituted_source_status" -eq 2 ]
mv "${inventory}/conversation-coverage.before" "${inventory}/conversation-coverage.jsonl"

git_only="${test_root}/git-only"
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  collect --root "$fixture" --output "$git_only" --include-git >/dev/null
python3 - "$git_only/summary.json" <<'PY'
import json, sys
summary = json.load(open(sys.argv[1]))
assert summary["selected"]["git"] is True
assert summary["selected"]["conversations"] is False
assert summary["sessions"] == []
assert summary["counts"]["conversation_records"] == 0
PY

conversation_only="${test_root}/conversation-only"
HOME="$home" "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
  collect --root "$fixture" --output "$conversation_only" --include-conversations >/dev/null
python3 - "$conversation_only/summary.json" <<'PY'
import json, sys
summary = json.load(open(sys.argv[1]))
assert summary["selected"]["git"] is False
assert summary["selected"]["conversations"] is True
assert summary["counts"]["commits"] == 0
assert summary["counts"]["tracked_paths"] == 0
PY

mutation_inventory="${test_root}/mutation-inventory"
set +e
mutation_output=$(
  HOME="$home" \
  PROJECT_CONTEXT_INVENTORY_TESTING=1 \
  PROJECT_CONTEXT_INVENTORY_TEST_MUTATE_SESSION=1 \
    "${repository_root}/reconstruct-project-context/scripts/inventory_local_history.py" \
      collect --root "$fixture" --output "$mutation_inventory" \
      --include-conversations 2>&1
)
mutation_status=$?
set -e
[ "$mutation_status" -eq 2 ]
printf '%s\n' "$mutation_output" | grep -Fq 'conversation source changed during collection'

printf 'reconstruction inventory tests passed\n'
