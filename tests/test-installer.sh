#!/bin/sh
set -eu

repository_root=$(
  CDPATH=
  cd -- "$(dirname -- "$0")/.."
  pwd
)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/project-context-installer-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

release_root="${test_root}/release"
mkdir -p "${release_root}/v0.1.7"
"${repository_root}/bin/package-skill" 0.1.7 "${release_root}/v0.1.7" >/dev/null
"${repository_root}/bin/package-installer" 0.1.7 "${release_root}/v0.1.7" >/dev/null
installer_under_test="${release_root}/v0.1.7/install-project-context-v0.1.7"

system_name=$(uname -s)
machine_name=$(uname -m)
case "$system_name" in Darwin) target_system=apple-darwin ;; Linux) target_system=unknown-linux-gnu ;; esac
case "$machine_name" in x86_64|amd64) target_arch=x86_64 ;; arm64|aarch64) target_arch=aarch64 ;; esac
binary="${release_root}/v0.1.7/project-context-v0.1.7-${target_arch}-${target_system}"
cp "${repository_root}/cli/target/debug/project-context" "$binary"
chmod 700 "$binary"
if command -v sha256sum >/dev/null 2>&1; then
  binary_checksum=$(sha256sum "$binary")
else
  binary_checksum=$(shasum -a 256 "$binary")
fi
printf '%s\n' "${binary_checksum%% *}" > "${binary}.sha256"

run_installer() {
  target=$1
  shift
  (
    cd "$target"
    PROJECT_CONTEXT_INSTALL_TESTING=1 \
    PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_root}" \
    TMPDIR="${test_root}/temporary" \
      "$installer_under_test" "$@"
  )
}

validate_json() {
  python3 -c 'import json, sys; json.load(sys.stdin)' >/dev/null
}

path_mode() {
  if stat -f '%Lp' "$1" >/dev/null 2>&1; then
    stat -f '%Lp' "$1"
  else
    stat -c '%a' "$1"
  fi
}

mkdir "${test_root}/temporary"
dry_run_repository="${test_root}/dry-run"
mkdir "$dry_run_repository"
git -C "$dry_run_repository" init -q
dry_run_output=$(run_installer "$dry_run_repository" \
  --dry-run --format json \
  --project-id dry-run \
  --description 'Dry-run fixture.' \
  --build 'cargo build' \
  --test 'cargo test' \
  --allow-empty lint \
  --allow-empty format)
printf '%s\n' "$dry_run_output" | validate_json
printf '%s\n' "$dry_run_output" | grep -Fq '"dry_run": true'
[ ! -e "${dry_run_repository}/.agents" ]
[ ! -e "${dry_run_repository}/AGENTS.md" ]
[ ! -e "${dry_run_repository}/.project-context" ]

bad_release="${test_root}/bad-release"
cp -R "$release_root" "$bad_release"
printf '%064d  unexpected-file\n' 0 \
  > "${bad_release}/v0.1.7/project-context-skill-v0.1.7.tar.gz.sha256"
bad_checksum_repository="${test_root}/bad-checksum"
mkdir "$bad_checksum_repository"
git -C "$bad_checksum_repository" init -q
set +e
bad_checksum_output=$(
  cd "$bad_checksum_repository"
  PROJECT_CONTEXT_INSTALL_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${bad_release}" \
  TMPDIR="${test_root}/temporary" \
    "${repository_root}/bin/install-project-context" \
      --dry-run --format json \
      --project-id bad-checksum \
      --description 'Bad checksum fixture.' \
      --allow-empty build --allow-empty test --allow-empty lint --allow-empty format
)
bad_checksum_status=$?
set -e
[ "$bad_checksum_status" -eq 2 ]
printf '%s\n' "$bad_checksum_output" | validate_json
printf '%s\n' "$bad_checksum_output" | grep -Fq 'checksum record names an unexpected file'
[ ! -e "${bad_checksum_repository}/.agents" ]

invalid_context_repository="${test_root}/invalid-context"
mkdir -p "${invalid_context_repository}/.project-context"
git -C "$invalid_context_repository" init -q
printf 'invalid: true\n' > "${invalid_context_repository}/.project-context/model.yaml"
set +e
invalid_context_output=$(run_installer "$invalid_context_repository" --format json 2>/dev/null)
invalid_context_status=$?
set -e
[ "$invalid_context_status" -eq 2 ]
printf '%s\n' "$invalid_context_output" | validate_json
printf '%s\n' "$invalid_context_output" | grep -Fq 'failed preflight validation'
[ ! -e "${invalid_context_repository}/.agents" ]
[ ! -e "${invalid_context_repository}/AGENTS.md" ]

target_repository="${test_root}/target"
mkdir "$target_repository"
git -C "$target_repository" init -q
printf '# Existing instructions\n' > "${target_repository}/AGENTS.md"
install_output=$(run_installer "$target_repository" \
  --format json \
  --project-id installer-fixture \
  --description 'Installer integration fixture.' \
  --build 'cargo build' \
  --test 'cargo test' \
  --lint 'cargo clippy' \
  --format-command 'cargo fmt --check')
printf '%s\n' "$install_output" | validate_json
printf '%s\n' "$install_output" | grep -Fq '"ok": true'
printf '%s\n' "$install_output" | grep -Fq '"validation": "passed"'
grep -Fq '# Existing instructions' "${target_repository}/AGENTS.md"
grep -Fq '<!-- project-context:managed:start -->' "${target_repository}/AGENTS.md"
grep -Fq 'id: installer-fixture' "${target_repository}/.project-context/model.yaml"
grep -Fq 'description: Installer integration fixture.' "${target_repository}/.project-context/model.yaml"
test -x "${target_repository}/.agents/skills/project-context/bin/project-context"
test -x "${target_repository}/.agents/skills/project-context/bin/update-project-context"
test -x "${target_repository}/.agents/skills/reconstruct-project-context/scripts/inventory_local_history.py"

rerun_output=$(run_installer "$target_repository" --format json)
printf '%s\n' "$rerun_output" | validate_json
printf '%s\n' "$rerun_output" | grep -Fq '"skill":"preserved-identical"'
printf '%s\n' "$rerun_output" | grep -Fq '"reconstruction_skill":"preserved-identical"'
printf '%s\n' "$rerun_output" | grep -Fq '"context":"preserved"'

additive_repository="${test_root}/additive"
mkdir "$additive_repository"
git -C "$additive_repository" init -q
cp -R "${target_repository}/.agents" "$additive_repository/.agents"
rm -rf "${additive_repository}/.agents/skills/reconstruct-project-context"
cp "${target_repository}/AGENTS.md" "$additive_repository/AGENTS.md"
cp -R "${target_repository}/.project-context" "$additive_repository/.project-context"
additive_output=$(run_installer "$additive_repository" --format json)
printf '%s\n' "$additive_output" | validate_json
printf '%s\n' "$additive_output" | grep -Fq '"skill":"preserved-identical"'
printf '%s\n' "$additive_output" | grep -Fq '"reconstruction_skill":"installed"'
test -x "${additive_repository}/.agents/skills/reconstruct-project-context/scripts/inventory_local_history.py"
printf '\nlocal reconstruction difference\n' \
  >> "${additive_repository}/.agents/skills/reconstruct-project-context/SKILL.md"
set +e
reconstruction_conflict_output=$(run_installer "$additive_repository" --format json 2>/dev/null)
reconstruction_conflict_status=$?
set -e
[ "$reconstruction_conflict_status" -eq 3 ]
printf '%s\n' "$reconstruction_conflict_output" | grep -Fq 'differs from the verified package'
grep -Fq 'local reconstruction difference' \
  "${additive_repository}/.agents/skills/reconstruct-project-context/SKILL.md"

partial_repository="${test_root}/unsupported-partial"
mkdir -p "${partial_repository}/.agents/skills"
git -C "$partial_repository" init -q
cp -R "${target_repository}/.agents/skills/reconstruct-project-context" \
  "${partial_repository}/.agents/skills/reconstruct-project-context"
set +e
partial_output=$(run_installer "$partial_repository" \
  --dry-run --format json \
  --project-id partial \
  --description 'Partial fixture.' \
  --allow-empty build --allow-empty test --allow-empty lint --allow-empty format 2>/dev/null)
partial_status=$?
set -e
[ "$partial_status" -eq 3 ]
printf '%s\n' "$partial_output" | grep -Fq 'unsupported partial installation'
[ ! -e "${partial_repository}/.agents/skills/project-context" ]

rollback_repository="${test_root}/rollback"
mkdir -p "${rollback_repository}/.agents/skills"
git -C "$rollback_repository" init -q
cp -R "${target_repository}/.agents/skills/project-context" \
  "${rollback_repository}/.agents/skills/project-context"
printf '# Rollback fixture\n' > "${rollback_repository}/AGENTS.md"
(
  cd "$rollback_repository"
  "${repository_root}/cli/target/debug/project-context" init >/dev/null
  "${repository_root}/cli/target/debug/project-context" configure \
    --description 'Rollback fixture.' --build 'cargo build' >/dev/null
)
cp "${rollback_repository}/AGENTS.md" "${test_root}/rollback-agents.before"
cp "${rollback_repository}/.project-context/model.yaml" "${test_root}/rollback-model.before"
set +e
rollback_output=$(run_installer "$rollback_repository" --format json 2>/dev/null)
rollback_status=$?
set -e
[ "$rollback_status" -eq 2 ]
printf '%s\n' "$rollback_output" | grep -Fq 'installation doctor reported incomplete configuration'
cmp "${test_root}/rollback-agents.before" "${rollback_repository}/AGENTS.md"
cmp "${test_root}/rollback-model.before" "${rollback_repository}/.project-context/model.yaml"
[ ! -e "${rollback_repository}/.agents/skills/reconstruct-project-context" ]

rollback_conflict_repository="${test_root}/rollback-conflict"
mkdir "$rollback_conflict_repository"
git -C "$rollback_conflict_repository" init -q
set +e
rollback_conflict_output=$(
  PROJECT_CONTEXT_INSTALL_INJECT_ROLLBACK_CONFLICT=content \
    run_installer "$rollback_conflict_repository" \
      --format json \
      --project-id rollback-conflict \
      --description 'Rollback conflict fixture.' \
      --allow-empty build --allow-empty test --allow-empty lint --allow-empty format \
      2>"${test_root}/rollback-conflict.stderr"
)
rollback_conflict_status=$?
unset PROJECT_CONTEXT_INSTALL_INJECT_ROLLBACK_CONFLICT
set -e
[ "$rollback_conflict_status" -eq 2 ]
printf '%s\n' "$rollback_conflict_output" | grep -Fq 'injected post-write failure'
grep -Fq 'rollback conflict' "${test_root}/rollback-conflict.stderr"
grep -Fq 'externally changed during installation' "${rollback_conflict_repository}/AGENTS.md"
[ ! -e "${rollback_conflict_repository}/.agents/skills/project-context" ]
[ ! -e "${rollback_conflict_repository}/.agents/skills/reconstruct-project-context" ]
[ ! -e "${rollback_conflict_repository}/.agents" ]
[ ! -e "${rollback_conflict_repository}/.project-context" ]

chmod_conflict_repository="${test_root}/rollback-chmod-conflict"
mkdir "$chmod_conflict_repository"
git -C "$chmod_conflict_repository" init -q
set +e
PROJECT_CONTEXT_INSTALL_INJECT_ROLLBACK_CONFLICT='chmod'
export PROJECT_CONTEXT_INSTALL_INJECT_ROLLBACK_CONFLICT
run_installer "$chmod_conflict_repository" \
    --format json \
    --project-id rollback-chmod-conflict \
    --description 'Rollback chmod conflict fixture.' \
    --allow-empty build --allow-empty test --allow-empty lint --allow-empty format \
    >"${test_root}/rollback-chmod.out" 2>"${test_root}/rollback-chmod.stderr"
chmod_conflict_status=$?
unset PROJECT_CONTEXT_INSTALL_INJECT_ROLLBACK_CONFLICT
set -e
[ "$chmod_conflict_status" -eq 2 ]
grep -Fq 'rollback conflict' "${test_root}/rollback-chmod.stderr"
[ -f "${chmod_conflict_repository}/AGENTS.md" ]

symlink_conflict_repository="${test_root}/rollback-symlink-conflict"
mkdir "$symlink_conflict_repository"
git -C "$symlink_conflict_repository" init -q
set +e
PROJECT_CONTEXT_INSTALL_INJECT_ROLLBACK_CONFLICT=symlink \
  run_installer "$symlink_conflict_repository" \
    --format json \
    --project-id rollback-symlink-conflict \
    --description 'Rollback symlink conflict fixture.' \
    --allow-empty build --allow-empty test --allow-empty lint --allow-empty format \
    >"${test_root}/rollback-symlink.out" 2>"${test_root}/rollback-symlink.stderr"
symlink_conflict_status=$?
unset PROJECT_CONTEXT_INSTALL_INJECT_ROLLBACK_CONFLICT
set -e
[ "$symlink_conflict_status" -eq 2 ]
grep -Fq 'rollback conflict' "${test_root}/rollback-symlink.stderr"
[ -L "${symlink_conflict_repository}/AGENTS.md" ]
grep -Fq 'external target' "${symlink_conflict_repository}.external-agents-target"

mode_repository="${test_root}/mode-conflict"
mkdir "$mode_repository"
git -C "$mode_repository" init -q
cp -R "${target_repository}/.agents" "$mode_repository/.agents"
cp -R "${target_repository}/.project-context" "$mode_repository/.project-context"
cp "${target_repository}/AGENTS.md" "$mode_repository/AGENTS.md"
mode_target="${mode_repository}/.agents/skills/reconstruct-project-context/SKILL.md"
if [ "$(path_mode "$mode_target")" = 600 ]; then
  chmod 644 "$mode_target"
else
  chmod 600 "$mode_target"
fi
set +e
mode_output=$(run_installer "$mode_repository" --format json 2>/dev/null)
mode_status=$?
set -e
[ "$mode_status" -eq 3 ]
printf '%s\n' "$mode_output" | grep -Fq 'differs from the verified package'

printf '\nlocal difference\n' >> "${target_repository}/.agents/skills/project-context/SKILL.md"
set +e
conflict_output=$(run_installer "$target_repository" --format json 2>/dev/null)
conflict_status=$?
set -e
[ "$conflict_status" -eq 3 ]
printf '%s\n' "$conflict_output" | validate_json
printf '%s\n' "$conflict_output" | grep -Fq '"conflict":true'
grep -Fq 'local difference' "${target_repository}/.agents/skills/project-context/SKILL.md"

marker_repository="${test_root}/malformed-marker"
mkdir "$marker_repository"
git -C "$marker_repository" init -q
printf '%s\n' '<!-- project-context:managed:start -->' > "${marker_repository}/AGENTS.md"
set +e
marker_output=$(run_installer "$marker_repository" \
  --dry-run --format json \
  --project-id malformed \
  --description 'Malformed marker fixture.' \
  --allow-empty build --allow-empty test --allow-empty lint --allow-empty format 2>/dev/null)
marker_status=$?
set -e
[ "$marker_status" -eq 3 ]
printf '%s\n' "$marker_output" | validate_json
printf '%s\n' "$marker_output" | grep -Fq '"conflict":true'
[ ! -e "${marker_repository}/.agents" ]
[ ! -e "${marker_repository}/.project-context" ]

if find "${test_root}/temporary" -mindepth 1 -maxdepth 1 -name 'project-context-install.*' -print -quit | grep -q .; then
  printf 'installer left a temporary directory behind\n' >&2
  exit 1
fi

printf 'installer tests passed\n'
