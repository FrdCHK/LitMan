#!/usr/bin/env bash
set -euo pipefail

version="${1:-2.2.1}"
project_dir="$(cd "$(dirname "$0")/.." && pwd)"
dist_dir="$project_dir/dist"
stage_dir="$project_dir/target/packaging/macos"
app_dir="$stage_dir/LitMan.app"

case "$version" in
  *[!0-9.]*|'') echo "Version must contain only digits and dots" >&2; exit 2 ;;
esac

mkdir -p "$dist_dir"
rm -rf "$stage_dir"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources/manual"

cd "$project_dir"
export MACOSX_DEPLOYMENT_TARGET=12.0
mdbook build docs/en
mdbook build docs/zh-CN
for target in x86_64-apple-darwin aarch64-apple-darwin; do
  cargo build --workspace --release --locked --target "$target"
done

lipo -create \
  target/x86_64-apple-darwin/release/litman-gui \
  target/aarch64-apple-darwin/release/litman-gui \
  -output "$app_dir/Contents/MacOS/litman-gui"
lipo -create \
  target/x86_64-apple-darwin/release/litman \
  target/aarch64-apple-darwin/release/litman \
  -output "$app_dir/Contents/MacOS/litman"
chmod 755 "$app_dir/Contents/MacOS/litman-gui" "$app_dir/Contents/MacOS/litman"

cp packaging/macos/Info.plist "$app_dir/Contents/Info.plist"
cp packaging/icons/litman.icns "$app_dir/Contents/Resources/litman.icns"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app_dir/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$app_dir/Contents/Info.plist"
cp -R docs/en/book "$app_dir/Contents/Resources/manual/en"
cp -R docs/zh-CN/book "$app_dir/Contents/Resources/manual/zh-CN"
cp LICENSE crates/litman-gui/assets/LICENSE-NOTO.txt "$app_dir/Contents/Resources/"

if [[ -n "${LITMAN_APPLE_IDENTITY:-}" ]]; then
  codesign --force --deep --options runtime --timestamp --sign "$LITMAN_APPLE_IDENTITY" "$app_dir"
fi

rm -rf "$dist_dir/LitMan.app"
ditto "$app_dir" "$dist_dir/LitMan.app"

rm -f "$dist_dir/LitMan-$version-universal.dmg"
hdiutil create -quiet -volname LitMan -srcfolder "$app_dir" -ov -format UDZO "$dist_dir/LitMan-$version-universal.dmg"

pkg_root="$stage_dir/pkg-root"
mkdir -p "$pkg_root/Applications" "$pkg_root/usr/local/bin" "$pkg_root/usr/local/share/doc/litman"
ditto "$app_dir" "$pkg_root/Applications/LitMan.app"
cp "$app_dir/Contents/MacOS/litman" "$pkg_root/usr/local/bin/litman"
cp -R docs/en/book "$pkg_root/usr/local/share/doc/litman/en"
cp -R docs/zh-CN/book "$pkg_root/usr/local/share/doc/litman/zh-CN"

component_pkg="$stage_dir/LitMan-component.pkg"
pkgbuild_args=(--root "$pkg_root" --identifier org.litman.desktop --version "$version" --install-location /)
pkgbuild "${pkgbuild_args[@]}" "$component_pkg"
productbuild_args=(--package "$component_pkg")
if [[ -n "${LITMAN_INSTALLER_IDENTITY:-}" ]]; then
  productbuild_args+=(--sign "$LITMAN_INSTALLER_IDENTITY")
fi
productbuild "${productbuild_args[@]}" "$dist_dir/LitMan-$version-universal.pkg"

shasum -a 256 "$dist_dir/LitMan-$version-universal.dmg" "$dist_dir/LitMan-$version-universal.pkg"
