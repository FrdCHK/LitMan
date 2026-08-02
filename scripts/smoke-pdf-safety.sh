#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/.." && pwd)"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/litman-safety.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT

cd "$project_dir"
cargo run -p litman-core --example generate_fixtures -- "$work_dir/pdfs"
cargo build -p litman-cli --locked

(
  cd "$work_dir/pdfs"
  sha256sum ./*.pdf | sort
) > "$work_dir/before.sha256"

target/debug/litman init --config "$work_dir/library.toml" --root "$work_dir/pdfs" --language zh-CN
target/debug/litman --config "$work_dir/library.toml" scan
target/debug/litman --config "$work_dir/library.toml" search "中文" --format json > "$work_dir/search.json"
grep -q '中文文献管理' "$work_dir/search.json"

(
  cd "$work_dir/pdfs"
  sha256sum ./*.pdf | sort
) > "$work_dir/after.sha256"
cmp "$work_dir/before.sha256" "$work_dir/after.sha256"
echo "PDF immutability smoke test passed"
