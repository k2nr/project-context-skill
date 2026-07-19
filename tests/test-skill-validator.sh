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
python3 "${repository_root}/bin/validate-skill-yaml" "${repository_root}/project-context" >/dev/null
python3 "${repository_root}/bin/validate-skill-yaml" \
  "${repository_root}/reconstruct-project-context" >/dev/null

expect_yaml_failure() {
  fixture=$1
  if python3 "${repository_root}/bin/validate-skill-yaml" "$fixture" >/dev/null 2>&1; then
    printf 'invalid YAML skill unexpectedly passed validation: %s\n' "$fixture" >&2
    exit 1
  fi
}

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
expect_yaml_failure "${test_root}/duplicate"

for fixture in malformed unexpected non-string-name non-string-description; do
  mkdir -p "${test_root}/${fixture}/agents"
  cp "${repository_root}/project-context/agents/openai.yaml" \
    "${test_root}/${fixture}/agents/openai.yaml"
done
printf '%s\n' \
  '---' \
  'name: malformed' \
  'description: [unterminated' \
  '---' \
  '# Invalid' > "${test_root}/malformed/SKILL.md"
printf '%s\n' \
  '---' \
  'name: unexpected' \
  'description: Invalid fixture.' \
  'unknown: true' \
  '---' \
  '# Invalid' > "${test_root}/unexpected/SKILL.md"
printf '%s\n' \
  '---' \
  'name:' \
  '  - non-string-name' \
  'description: Invalid fixture.' \
  '---' \
  '# Invalid' > "${test_root}/non-string-name/SKILL.md"
printf '%s\n' \
  '---' \
  'name: non-string-description' \
  'description: 42' \
  '---' \
  '# Invalid' > "${test_root}/non-string-description/SKILL.md"

expect_yaml_failure "${test_root}/malformed"
expect_yaml_failure "${test_root}/unexpected"
expect_yaml_failure "${test_root}/non-string-name"
expect_yaml_failure "${test_root}/non-string-description"

printf 'skill validator tests passed\n'
