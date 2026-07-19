#!/bin/sh
set -eu

repository_root=$(
  CDPATH=
  cd -- "$(dirname -- "$0")/.."
  pwd
)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/project-context-update-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

release_root="${test_root}/release"
mkdir -p "${release_root}/v0.1.7"
"${repository_root}/bin/package-skill" 0.1.7 "${release_root}/v0.1.7" >/dev/null

current_extract="${test_root}/current"
mkdir "$current_extract"
tar -xzf "${release_root}/v0.1.7/project-context-skill-v0.1.7.tar.gz" -C "$current_extract"

old_extract="${test_root}/old"
cp -R "$current_extract" "$old_extract"
sed 's/PROJECT_CONTEXT_VERSION="0.1.7"/PROJECT_CONTEXT_VERSION="0.1.6"/' \
  "${old_extract}/project-context/bin/project-context" \
  > "${test_root}/old-launcher"
mv "${test_root}/old-launcher" "${old_extract}/project-context/bin/project-context"
chmod 755 "${old_extract}/project-context/bin/project-context" \
  "${old_extract}/project-context/bin/update-project-context" \
  "${old_extract}/reconstruct-project-context/scripts/inventory_local_history.py"

mkdir -p "${release_root}/v0.1.6"
old_archive="${release_root}/v0.1.6/project-context-skill-v0.1.6.tar.gz"
(
  cd "$old_extract"
  COPYFILE_DISABLE=1 tar -czf "$old_archive" project-context reconstruct-project-context
)
if command -v sha256sum >/dev/null 2>&1; then
  old_checksum=$(sha256sum "$old_archive")
else
  old_checksum=$(shasum -a 256 "$old_archive")
fi
printf '%s  %s\n' "${old_checksum%% *}" "$(basename -- "$old_archive")" \
  > "${old_archive}.sha256"

system_name=$(uname -s)
machine_name=$(uname -m)
case "$system_name" in Darwin) target_system=apple-darwin ;; Linux) target_system=unknown-linux-gnu ;; esac
case "$machine_name" in x86_64|amd64) target_arch=x86_64 ;; arm64|aarch64) target_arch=aarch64 ;; esac
for version in 0.1.6 0.1.7; do
  binary="${release_root}/v${version}/project-context-v${version}-${target_arch}-${target_system}"
  cp "${repository_root}/cli/target/debug/project-context" "$binary"
  chmod 700 "$binary"
  if command -v sha256sum >/dev/null 2>&1; then
    binary_checksum=$(sha256sum "$binary")
  else
    binary_checksum=$(shasum -a 256 "$binary")
  fi
  printf '%s\n' "${binary_checksum%% *}" > "${binary}.sha256"
done

make_target() {
  target=$1
  mkdir -p "${target}/.agents/skills"
  cp -R "${old_extract}/project-context" "${target}/.agents/skills/project-context"
  cp -R "${old_extract}/reconstruct-project-context" \
    "${target}/.agents/skills/reconstruct-project-context"
  cp "${old_extract}/project-context/assets/install/AGENTS.fragment.md" "${target}/AGENTS.md"
  git -C "$target" init -q
  (
    cd "$target"
    "${repository_root}/cli/target/debug/project-context" init >/dev/null
    "${repository_root}/cli/target/debug/project-context" configure \
      --project-id update-fixture \
      --description 'Updater integration fixture.' \
      --build 'cargo build' \
      --test 'cargo test' \
      --lint 'cargo clippy' \
      --format-command 'cargo fmt --check' >/dev/null
  )
}

run_update() {
  target=$1
  shift
  (
    cd "$target"
    PROJECT_CONTEXT_UPDATE_TESTING=1 \
    PROJECT_CONTEXT_UPDATE_LATEST_VERSION=0.1.7 \
    PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_root}" \
    TMPDIR="${test_root}" \
      .agents/skills/project-context/bin/update-project-context "$@"
  )
}

validate_json() {
  python3 -c 'import json, sys; json.load(sys.stdin)' >/dev/null
}

target="${test_root}/target"
make_target "$target"
cp -R "${target}/.project-context" "${test_root}/context.before"
cp "${target}/AGENTS.md" "${test_root}/agents.before"

dry_run_output=$(run_update "$target" --dry-run --format json)
printf '%s\n' "$dry_run_output" | validate_json
printf '%s\n' "$dry_run_output" | grep -Fq '"latest_version": "0.1.7"'
printf '%s\n' "$dry_run_output" | grep -Fq '"skill":"planned-update"'
grep -Fq 'PROJECT_CONTEXT_VERSION="0.1.6"' \
  "${target}/.agents/skills/project-context/bin/project-context"

update_output=$(run_update "$target" --format json)
printf '%s\n' "$update_output" | validate_json
printf '%s\n' "$update_output" | grep -Fq '"skill":"updated"'
grep -Fq 'PROJECT_CONTEXT_VERSION="0.1.7"' \
  "${target}/.agents/skills/project-context/bin/project-context"
diff -qr "${current_extract}/project-context" \
  "${target}/.agents/skills/project-context" >/dev/null
diff -qr "${current_extract}/reconstruct-project-context" \
  "${target}/.agents/skills/reconstruct-project-context" >/dev/null
diff -qr "${test_root}/context.before" "${target}/.project-context" >/dev/null
cmp "${test_root}/agents.before" "${target}/AGENTS.md"

current_output=$(run_update "$target" --format json)
printf '%s\n' "$current_output" | validate_json
printf '%s\n' "$current_output" | grep -Fq '"skill":"already-current"'

conflict_target="${test_root}/conflict"
make_target "$conflict_target"
printf '\nlocal change\n' >> "${conflict_target}/.agents/skills/project-context/SKILL.md"
cp -R "${conflict_target}/.agents" "${test_root}/conflict-agents.before"
set +e
conflict_output=$(run_update "$conflict_target" --format json 2>/dev/null)
conflict_status=$?
set -e
[ "$conflict_status" -eq 3 ]
printf '%s\n' "$conflict_output" | validate_json
printf '%s\n' "$conflict_output" | grep -Fq 'local changes relative to v0.1.6'
diff -qr "${test_root}/conflict-agents.before" "${conflict_target}/.agents" >/dev/null

agents_conflict_target="${test_root}/agents-conflict"
make_target "$agents_conflict_target"
sed 's/For every non-trivial/For each non-trivial/' \
  "${agents_conflict_target}/AGENTS.md" > "${test_root}/agents-conflict.next"
mv "${test_root}/agents-conflict.next" "${agents_conflict_target}/AGENTS.md"
set +e
agents_conflict_output=$(run_update "$agents_conflict_target" --format json 2>/dev/null)
agents_conflict_status=$?
set -e
[ "$agents_conflict_status" -eq 3 ]
printf '%s\n' "$agents_conflict_output" | grep -Fq 'managed block has local changes'

rollback_target="${test_root}/rollback"
make_target "$rollback_target"
cp -R "${rollback_target}/.agents" "${test_root}/rollback-agents.before"
cp "${rollback_target}/AGENTS.md" "${test_root}/rollback-agents-md.before"
set +e
rollback_output=$(
  cd "$rollback_target"
  PROJECT_CONTEXT_UPDATE_TESTING=1 \
  PROJECT_CONTEXT_UPDATE_LATEST_VERSION=0.1.7 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_root}" \
  PROJECT_CONTEXT_UPDATE_INJECT_FAILURE=1 \
  TMPDIR="${test_root}" \
    .agents/skills/project-context/bin/update-project-context --format json 2>/dev/null
)
rollback_status=$?
set -e
[ "$rollback_status" -eq 2 ]
printf '%s\n' "$rollback_output" | validate_json
printf '%s\n' "$rollback_output" | grep -Fq 'injected post-update failure'
diff -qr "${test_root}/rollback-agents.before" "${rollback_target}/.agents" >/dev/null
cmp "${test_root}/rollback-agents-md.before" "${rollback_target}/AGENTS.md"

mid_swap_target="${test_root}/mid-swap"
make_target "$mid_swap_target"
cp -R "${mid_swap_target}/.agents" "${test_root}/mid-swap-agents.before"
set +e
mid_swap_output=$(
  cd "$mid_swap_target"
  PROJECT_CONTEXT_UPDATE_TESTING=1 \
  PROJECT_CONTEXT_UPDATE_LATEST_VERSION=0.1.7 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_root}" \
  PROJECT_CONTEXT_UPDATE_INJECT_FAILURE=after-skill-backup \
  TMPDIR="${test_root}" \
    .agents/skills/project-context/bin/update-project-context --format json 2>/dev/null
)
mid_swap_status=$?
set -e
[ "$mid_swap_status" -eq 2 ]
printf '%s\n' "$mid_swap_output" | grep -Fq 'injected failure after preserving'
diff -qr "${test_root}/mid-swap-agents.before" "${mid_swap_target}/.agents" >/dev/null

printf 'update tests passed\n'
