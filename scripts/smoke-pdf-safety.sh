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

cp "$work_dir/pdfs/xmp-prism.pdf" "$work_dir/publisher.pdf"
selected_hash="$(sha256sum "$work_dir/pdfs/info-only.pdf" | cut -d ' ' -f 1)"
publisher_hash="$(sha256sum "$work_dir/publisher.pdf" | cut -d ' ' -f 1)"
cargo run -p litman-core --example replacement_smoke -- \
  "$work_dir/library.toml" info-only.pdf "$work_dir/publisher.pdf"

backup_pdf="$work_dir/pdfs/LitMan-backups/2008MNRAS.386..619C_bk.pdf"
active_pdf="$work_dir/pdfs/2008MNRAS.386..619C.pdf"
test "$(sha256sum "$backup_pdf" | cut -d ' ' -f 1)" = "$selected_hash"
test "$(sha256sum "$active_pdf" | cut -d ' ' -f 1)" = "$publisher_hash"
test "$(sha256sum "$work_dir/publisher.pdf" | cut -d ' ' -f 1)" = "$publisher_hash"
grep -v '  ./info-only.pdf$' "$work_dir/before.sha256" > "$work_dir/unrelated.sha256"
(
  cd "$work_dir/pdfs"
  sha256sum -c "$work_dir/unrelated.sha256"
)

echo "Ordinary PDF immutability and explicit replacement safety smoke tests passed"
