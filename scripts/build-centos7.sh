#!/usr/bin/env bash
set -euo pipefail

version="${1:-0.1.4}"
workspace="${LITMAN_WORKSPACE:-/workspace}"
output="${LITMAN_OUTPUT:-/out}"
build_root="$(mktemp -d /tmp/litman-rpm.XXXXXX)"
trap 'rm -rf "$build_root"' EXIT

case "$version" in
  *[!0-9.]*|'') echo "Version must contain only digits and dots" >&2; exit 2 ;;
esac

source_dir="$build_root/litman-$version"
rpm_top="$build_root/rpmbuild"
mkdir -p "$source_dir" "$rpm_top"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS} "$output"

tar --exclude=target --exclude=.git --exclude=dist -cf - -C "$workspace" . | tar -xf - -C "$source_dir"
cd "$source_dir"
mdbook build docs/en
mdbook build docs/zh-CN
mkdir -p .cargo
cargo vendor --locked vendor > .cargo/config.toml

tar -czf "$rpm_top/SOURCES/litman-$version.tar.gz" -C "$build_root" "litman-$version"
sed "s/@VERSION@/$version/g" packaging/centos7/litman.spec.in > "$rpm_top/SPECS/litman.spec"
rpmbuild --define "_topdir $rpm_top" -ba "$rpm_top/SPECS/litman.spec"

rpm_path="$(find "$rpm_top/RPMS" -name 'litman-*.x86_64.rpm' -print -quit)"
test -n "$rpm_path"
mkdir -p "$build_root/audit-root"
rpm2cpio "$rpm_path" | cpio -idmv -D "$build_root/audit-root"
"$workspace/scripts/audit-glibc.sh" \
  "$build_root/audit-root/usr/bin/litman" \
  "$build_root/audit-root/usr/bin/litman-gui"
"$build_root/audit-root/usr/bin/litman" --help >/dev/null
rpm -qpl "$rpm_path" | grep -q '/usr/bin/litman-gui'

cp "$rpm_path" "$output/"
sha256sum "$output/$(basename "$rpm_path")"
