#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 0 ]]; then
  echo "usage: $0 BINARY..." >&2
  exit 2
fi

failed=0
for binary in "$@"; do
  if [[ ! -x "$binary" ]]; then
    echo "not an executable: $binary" >&2
    failed=1
    continue
  fi
  required="$(readelf --version-info "$binary" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -Vu || true)"
  newest="$(printf '%s\n' "$required" | sed 's/^GLIBC_//' | sort -Vu | tail -n 1)"
  echo "$binary: newest required GLIBC symbol is ${newest:-none}"
  if printf '%s\n' "$required" | sed 's/^GLIBC_//' | awk -F. '$1 > 2 || ($1 == 2 && $2 > 17) { bad=1 } END { exit bad ? 0 : 1 }'; then
    echo "$binary requires a GLIBC symbol newer than 2.17" >&2
    failed=1
  fi
done
exit "$failed"
