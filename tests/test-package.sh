#!/bin/sh
set -eu

repository_root=$(
  CDPATH=
  cd -- "$(dirname -- "$0")/.."
  pwd
)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/project-context-package-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

if [ "$#" -eq 1 ]; then
  archive=$1
else
  first_output="${test_root}/first"
  second_output="${test_root}/second"
  "$repository_root/bin/package-skill" 0.1.3 "$first_output" >/dev/null
  "$repository_root/bin/package-skill" 0.1.3 "$second_output" >/dev/null
  archive="${first_output}/project-context-skill-v0.1.3.tar.gz"
  cmp "$archive" "${second_output}/project-context-skill-v0.1.3.tar.gz"
  (
    cd "$first_output"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum -c project-context-skill-v0.1.3.tar.gz.sha256
    else
      shasum -a 256 -c project-context-skill-v0.1.3.tar.gz.sha256
    fi
  )
fi

archive_name=$(basename -- "$archive")
archive_directory=$(
  CDPATH=
  cd -- "$(dirname -- "$archive")"
  pwd
)
archive="${archive_directory}/${archive_name}"
actual="${test_root}/actual.txt"
expected="${test_root}/expected.txt"
tar -tzf "$archive" | LC_ALL=C sort > "$actual"
cat > "$expected" <<'EOF'
project-context/
project-context/LICENSE
project-context/SKILL.md
project-context/agents/
project-context/agents/openai.yaml
project-context/assets/
project-context/assets/init/
project-context/assets/init/event.schema.json
project-context/assets/init/model.schema.json
project-context/assets/init/model.yaml
project-context/assets/install/
project-context/assets/install/AGENTS.fragment.md
project-context/bin/
project-context/bin/project-context
EOF
LC_ALL=C sort -o "$expected" "$expected"
diff -u "$expected" "$actual"

if tar -tvzf "$archive" | awk '$1 !~ /^[-d]/ { found=1 } END { exit found ? 0 : 1 }'; then
  printf 'package contains a non-regular, non-directory member\n' >&2
  exit 1
fi
if tar -tzf "$archive" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
  printf 'package contains an unsafe path\n' >&2
  exit 1
fi

extract_root="${test_root}/extract"
mkdir "$extract_root"
tar -xzf "$archive" -C "$extract_root"
package_root="${extract_root}/project-context"
[ -x "${package_root}/bin/project-context" ]
sh -n "${package_root}/bin/project-context"
cmp "$repository_root/LICENSE" "${package_root}/LICENSE"
grep -q '<!-- project-context:managed:start -->' "${package_root}/assets/install/AGENTS.fragment.md"
grep -q '<!-- project-context:managed:end -->' "${package_root}/assets/install/AGENTS.fragment.md"
[ ! -e "${package_root}/cli" ]
[ ! -e "${package_root}/tests" ]

debug_binary="${repository_root}/cli/target/debug/project-context"
if [ -x "$debug_binary" ]; then
  target_repository="${test_root}/target-repository"
  mkdir -p "${target_repository}/.agents/skills"
  cp -R "$package_root" "${target_repository}/.agents/skills/project-context"
  cp "${package_root}/assets/install/AGENTS.fragment.md" "${target_repository}/AGENTS.md"

  system_name=$(uname -s)
  machine_name=$(uname -m)
  case "$system_name" in Darwin) target_system=apple-darwin ;; Linux) target_system=unknown-linux-gnu ;; esac
  case "$machine_name" in x86_64|amd64) target_arch=x86_64 ;; arm64|aarch64) target_arch=aarch64 ;; esac
  release_root="${test_root}/release/v0.1.3"
  mkdir -p "$release_root"
  asset="${release_root}/project-context-v0.1.3-${target_arch}-${target_system}"
  cp "$debug_binary" "$asset"
  chmod 700 "$asset"
  if command -v sha256sum >/dev/null 2>&1; then
    checksum=$(sha256sum "$asset")
  else
    checksum=$(shasum -a 256 "$asset")
  fi
  checksum=${checksum%% *}
  printf '%s\n' "$checksum" > "${asset}.sha256"
  (
    cd "$target_repository"
    PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
    PROJECT_CONTEXT_RELEASE_BASE_URL="file://${test_root}/release" \
    PROJECT_CONTEXT_CACHE_DIR="${test_root}/installed-cache" \
    .agents/skills/project-context/bin/project-context init
    PROJECT_CONTEXT_BOOTSTRAP_TESTING=1 \
    PROJECT_CONTEXT_RELEASE_BASE_URL="file://${test_root}/release" \
    PROJECT_CONTEXT_CACHE_DIR="${test_root}/installed-cache" \
    .agents/skills/project-context/bin/project-context validate --strict
  )
  grep -q "id: target-repository" "${target_repository}/.project-context/model.yaml"
fi

printf 'package tests passed\n'
