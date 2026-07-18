#!/bin/sh
set -eu

launcher=$1
test_root=$(mktemp -d "${TMPDIR:-/tmp}/project-context-bootstrap-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

if grep -q 'PENDING_GITHUB_REPOSITORY' "$launcher"; then
  set +e
  pending_output=$("$launcher" validate 2>&1)
  pending_status=$?
  set -e
  [ "$pending_status" -eq 2 ]
  printf '%s\n' "$pending_output" | grep -q 'GitHub Releases repository is pending'
fi

system_name=$(uname -s)
machine_name=$(uname -m)
case "$system_name" in
  Darwin) target_system="apple-darwin" ;;
  Linux) target_system="unknown-linux-gnu" ;;
  *) printf 'unsupported test operating system: %s\n' "$system_name" >&2; exit 2 ;;
esac
case "$machine_name" in
  x86_64|amd64) target_arch="x86_64" ;;
  arm64|aarch64) target_arch="aarch64" ;;
  *) printf 'unsupported test architecture: %s\n' "$machine_name" >&2; exit 2 ;;
esac

target="${target_arch}-${target_system}"
asset_name="project-context-v0.1.7-${target}"

create_release() {
  release_root=$1
  fixture_label=$2
  release_directory="${release_root}/v0.1.7"
  mkdir -p "$release_directory"
  fixture_binary="${release_directory}/${asset_name}"
  # shellcheck disable=SC2016 # The generated fixture must expand its own arguments.
  printf '%s\n' \
    '#!/bin/sh' \
    'if [ "${1:-}" = "create-file" ]; then : > "$2"; exit 0; fi' \
    "printf '${fixture_label}:%s\\n' \"\$*\"" > "$fixture_binary"
  chmod 700 "$fixture_binary"
  if command -v sha256sum >/dev/null 2>&1; then
    checksum=$(sha256sum "$fixture_binary")
  else
    checksum=$(shasum -a 256 "$fixture_binary")
  fi
  checksum=${checksum%% *}
  printf '%s\n' "$checksum" > "${fixture_binary}.sha256"
}

release_a="${test_root}/release-a"
release_b="${test_root}/release-b"
create_release "$release_a" "fixture-a"
create_release "$release_b" "fixture-b"

set +e
missing_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${test_root}/missing" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/missing-cache" \
  "$launcher" validate 2>&1
)
missing_status=$?
set -e
[ "$missing_status" -eq 2 ]
printf '%s\n' "$missing_output" | grep -q 'download failed'

fixture_a="${release_a}/v0.1.7/${asset_name}"
cp "${fixture_a}.sha256" "${fixture_a}.sha256.valid"
printf '%064d\n' 0 > "${fixture_a}.sha256"
set +e
checksum_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/invalid-cache" \
  "$launcher" validate 2>&1
)
checksum_status=$?
set -e
[ "$checksum_status" -eq 2 ]
printf '%s\n' "$checksum_output" | grep -q 'failed SHA-256 verification'
mv "${fixture_a}.sha256.valid" "${fixture_a}.sha256"

cp "${fixture_a}.sha256" "${fixture_a}.sha256.valid"
printf '%s\n' 'not-a-checksum' > "${fixture_a}.sha256"
set +e
malformed_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/malformed-cache" \
  "$launcher" validate 2>&1
)
malformed_status=$?
set -e
[ "$malformed_status" -eq 2 ]
printf '%s\n' "$malformed_output" | grep -q 'invalid format'
mv "${fixture_a}.sha256.valid" "${fixture_a}.sha256"

hash_mock="${test_root}/hash-mock"
mkdir "$hash_mock"
printf '%s\n' '#!/bin/sh' 'exit 1' > "${hash_mock}/sha256sum"
chmod 700 "${hash_mock}/sha256sum"
set +e
hash_output=$(
  PATH="${hash_mock}:${PATH}" \
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/hash-failure-cache" \
  "$launcher" validate 2>&1
)
hash_status=$?
set -e
[ "$hash_status" -eq 2 ]
printf '%s\n' "$hash_output" | grep -q 'cannot calculate the downloaded binary checksum'

lower_checksum=$(sed -n '1p' "${fixture_a}.sha256")
printf '%s\n' "$lower_checksum" | tr 'a-f' 'A-F' > "${fixture_a}.sha256"
output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/cache" \
  "$launcher" init --format json
)
[ "$output" = 'fixture-a:init --format json' ]
printf '%s\n' "$lower_checksum" > "${fixture_a}.sha256"

mv "${release_a}/v0.1.7" "${release_a}/v0.1.7.offline"
cached_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/cache" \
  "$launcher" validate
)
[ "$cached_output" = 'fixture-a:validate' ]
mv "${release_a}/v0.1.7.offline" "${release_a}/v0.1.7"

printf '%s\n' '#!/bin/sh' 'printf "corrupt-cache-ran\n"' > "${test_root}/cache/v0.1.7/${target}/project-context"
chmod 700 "${test_root}/cache/v0.1.7/${target}/project-context"
mv "${release_a}/v0.1.7" "${release_a}/v0.1.7.offline"
set +e
corrupt_offline_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/cache" \
  "$launcher" validate 2>&1
)
corrupt_offline_status=$?
set -e
[ "$corrupt_offline_status" -eq 2 ]
printf '%s\n' "$corrupt_offline_output" | grep -q 'download failed'
if printf '%s\n' "$corrupt_offline_output" | grep -q 'corrupt-cache-ran'; then
  printf 'corrupt cached binary unexpectedly executed\n' >&2
  exit 1
fi
mv "${release_a}/v0.1.7.offline" "${release_a}/v0.1.7"
repaired_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/cache" \
  "$launcher" validate
)
[ "$repaired_output" = 'fixture-a:validate' ]

cached_checksum="${test_root}/cache/v0.1.7/${target}/project-context.sha256"
: > "$cached_checksum"
recovered_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/cache" \
  "$launcher" validate
)
[ "$recovered_output" = 'fixture-a:validate' ]

origin_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_b}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/cache" \
  "$launcher" validate
)
[ "$origin_output" = 'fixture-b:validate' ]

parallel_cache="${test_root}/parallel-cache"
parallel_pids=""
parallel_index=1
while [ "$parallel_index" -le 6 ]; do
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="$parallel_cache" \
  "$launcher" validate > "${test_root}/parallel-${parallel_index}.out" &
  parallel_pids="${parallel_pids} $!"
  parallel_index=$((parallel_index + 1))
done
for parallel_pid in $parallel_pids; do
  wait "$parallel_pid"
done
parallel_index=1
while [ "$parallel_index" -le 6 ]; do
  [ "$(sed -n '1p' "${test_root}/parallel-${parallel_index}.out")" = 'fixture-a:validate' ]
  parallel_index=$((parallel_index + 1))
done

mkdir "${test_root}/shared-cache"
chmod 755 "${test_root}/shared-cache"
set +e
shared_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/shared-cache" \
  "$launcher" validate 2>&1
)
shared_status=$?
set -e
[ "$shared_status" -eq 2 ]
printf '%s\n' "$shared_output" | grep -q 'must have mode 0700'

mode_file="${test_root}/mode-file"
(
  umask 022
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/mode-cache" \
  "$launcher" create-file "$mode_file"
)
if stat -f '%Lp' "$mode_file" >/dev/null 2>&1; then
  mode=$(stat -f '%Lp' "$mode_file")
else
  mode=$(stat -c '%a' "$mode_file")
fi
[ "$mode" = "644" ]

mkdir "${test_root}/real-cache"
ln -s "${test_root}/real-cache" "${test_root}/linked-cache"
set +e
symlink_output=$(
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/linked-cache" \
  "$launcher" validate 2>&1
)
symlink_status=$?
set -e
[ "$symlink_status" -eq 2 ]
printf '%s\n' "$symlink_output" | grep -q 'must not be a symbolic link'

mock_path="${test_root}/mock-path"
mkdir "$mock_path"
# shellcheck disable=SC2016 # The generated uname fixture must inspect its own argument.
printf '%s\n' '#!/bin/sh' 'case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; *) exit 2 ;; esac' > "${mock_path}/uname"
printf '%s\n' '#!/bin/sh' 'exit 1' > "${mock_path}/getconf"
chmod 700 "${mock_path}/uname" "${mock_path}/getconf"
set +e
musl_output=$(
  PATH="${mock_path}:${PATH}" \
  PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
  PROJECT_CONTEXT_RELEASE_BASE_URL="file://${release_a}" \
  PROJECT_CONTEXT_CACHE_DIR="${test_root}/musl-cache" \
  "$launcher" validate 2>&1
)
musl_status=$?
set -e
[ "$musl_status" -eq 2 ]
printf '%s\n' "$musl_output" | grep -q 'musl and Alpine are not supported'

printf 'bootstrap tests passed\n'
