#!/bin/sh
set -eu

repository_root=$(
  CDPATH=
  cd -- "$(dirname -- "$0")/.."
  pwd
)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/project-context-installer-package-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

first="${test_root}/first"
second="${test_root}/second"
"${repository_root}/bin/package-installer" 0.1.7 "$first" >/dev/null
"${repository_root}/bin/package-installer" 0.1.7 "$second" >/dev/null
asset=install-project-context-v0.1.7
cmp "${first}/${asset}" "${second}/${asset}"
cmp "${first}/${asset}.sha256" "${second}/${asset}.sha256"
test -x "${first}/${asset}"
sh -n "${first}/${asset}"
(
  cd "$first"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${asset}.sha256"
  else
    shasum -a 256 -c "${asset}.sha256"
  fi
)

printf 'installer package tests passed\n'
