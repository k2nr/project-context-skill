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

rerun_output=$(run_installer "$target_repository" --format json)
printf '%s\n' "$rerun_output" | validate_json
printf '%s\n' "$rerun_output" | grep -Fq '"skill":"preserved-identical"'
printf '%s\n' "$rerun_output" | grep -Fq '"context":"preserved"'

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
