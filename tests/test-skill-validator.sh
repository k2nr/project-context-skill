#!/bin/sh
set -eu

repository_root=$(
  CDPATH=
  cd -- "$(dirname -- "$0")/.."
  pwd
)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/project-context-skill-validator-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

"${repository_root}/bin/validate-skill" "${repository_root}/project-context" >/dev/null
"${repository_root}/bin/validate-skill" "${repository_root}/reconstruct-project-context" >/dev/null

mkdir -p "${test_root}/invalid/agents"
cp "${repository_root}/project-context/agents/openai.yaml" "${test_root}/invalid/agents/openai.yaml"
printf '%s\n' \
  '---' \
  'name: Invalid_Name' \
  'description: Invalid fixture.' \
  '---' \
  '# Invalid' > "${test_root}/invalid/SKILL.md"
if "${repository_root}/bin/validate-skill" "${test_root}/invalid" >/dev/null 2>&1; then
  printf 'invalid skill unexpectedly passed validation\n' >&2
  exit 1
fi

mkdir -p "${test_root}/duplicate/agents"
cp "${repository_root}/project-context/agents/openai.yaml" "${test_root}/duplicate/agents/openai.yaml"
printf '%s\n' \
  '---' \
  'name: project-context' \
  'name: duplicate' \
  'description: Invalid fixture.' \
  '---' \
  '# Invalid' > "${test_root}/duplicate/SKILL.md"
if "${repository_root}/bin/validate-skill" "${test_root}/duplicate" >/dev/null 2>&1; then
  printf 'duplicate frontmatter unexpectedly passed validation\n' >&2
  exit 1
fi

printf 'skill validator tests passed\n'
