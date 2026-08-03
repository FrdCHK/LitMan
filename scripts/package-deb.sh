#!/usr/bin/env bash
set -euo pipefail

version="${1:-0.1.5}"
project_dir="$(cd "$(dirname "$0")/.." && pwd)"
stage_dir="$project_dir/target/packaging/debian/litman_${version}_amd64"
dist_dir="$project_dir/dist"

case "$version" in
  *[!0-9.]*|'') echo "Version must contain only digits and dots" >&2; exit 2 ;;
esac

cd "$project_dir"
cargo build --workspace --release --locked
mdbook build docs/en
mdbook build docs/zh-CN

rm -rf "$stage_dir"
mkdir -p "$stage_dir/DEBIAN" "$stage_dir/usr/bin" \
  "$stage_dir/usr/share/applications" "$stage_dir/usr/share/icons/hicolor/scalable/apps" \
  "$stage_dir/usr/share/doc/litman/en" "$stage_dir/usr/share/doc/litman/zh-CN" \
  "$stage_dir/usr/share/licenses/litman"

sed "s/@VERSION@/$version/g" packaging/debian/control.in > "$stage_dir/DEBIAN/control"
install -m 0755 target/release/litman target/release/litman-gui "$stage_dir/usr/bin/"
install -m 0644 packaging/linux/litman.desktop "$stage_dir/usr/share/applications/"
install -m 0644 packaging/linux/litman.svg "$stage_dir/usr/share/icons/hicolor/scalable/apps/"
cp -R docs/en/book/. "$stage_dir/usr/share/doc/litman/en/"
cp -R docs/zh-CN/book/. "$stage_dir/usr/share/doc/litman/zh-CN/"
install -m 0644 LICENSE crates/litman-gui/assets/LICENSE-NOTO.txt "$stage_dir/usr/share/licenses/litman/"

mkdir -p "$dist_dir"
dpkg-deb --root-owner-group --build "$stage_dir" "$dist_dir/litman_${version}_amd64.deb"
sha256sum "$dist_dir/litman_${version}_amd64.deb"
