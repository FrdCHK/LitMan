# Build and release manual

## Reproducible inputs

Install Rust through rustup and use the checked-in `rust-toolchain.toml` and `Cargo.lock`. Release commands use `--locked`; the CentOS build vendors crates. Install mdBook to build both offline manuals:

```console
cargo install mdbook --locked
mdbook build docs/en
mdbook build docs/zh-CN
cargo build --workspace --release --locked
```

Unsigned packages are the default. Record the source revision, Rust version, package-tool versions, target triple, and SHA-256 checksums. Build from a clean source archive and retain the produced manuals with artifacts.

## Windows 10/11 x64 — primary gate

Use Windows 10 22H2 or Windows 11 x64 with Visual Studio 2022 Build Tools (**Desktop development with C++** and a Windows SDK), Rust MSVC x64, mdBook, and WiX Toolset 3.14. From a Developer PowerShell:

Install WiX machine-wide with `winget install --id WiXToolset.WiXToolset --exact --scope machine`. The packaging script discovers WiX through `PATH`, the machine/user `WIX` environment variable, or the standard WiX 3.14 installation directories, so `cargo clean` does not remove it.

```powershell
./scripts/package-windows.ps1 -Version 2.2.1
```

The script builds both binaries and manuals, `dist/LitMan-2.2.1-x64.msi`, a standalone `dist/LitMan-2.2.1-portable-x64.exe`, and a portable ZIP containing `LitMan.exe`, `litman-cli.exe`, licenses, and offline manuals. The GUI executable contains the checked-in ICO. The MSI uses the executable's embedded icon for explicit, non-advertised Start Menu and Desktop shortcuts and uses the checked-in ICO for Add/Remove Programs. Shortcut components are rooted in Windows Installer's standard `ProgramMenuFolder` and `DesktopFolder`; `ALLUSERS=1` resolves them to the common Start Menu and public desktop, while their required component key paths remain in `HKCU` like the proven 2.1.0 package. Both shortcuts have uninstall cleanup, and the MSI offers the CLI PATH component. The build reads the finished MSI tables and fails if either shortcut loses its standard shell directory, component, or explicit executable target. Set `LITMAN_CERT_THUMBPRINT` and pass `-Sign` to invoke `signtool`.

MSI ProductCodes are deterministically derived from the version and source contents. WiX permits same-version major upgrades, so a changed rebuild upgrades an installed package without requiring a manual uninstall; an identical rebuild remains reproducible. Keep the UpgradeCode stable across releases.

WiX can also be used without installation: download the official [`wix314-binaries.zip`](https://github.com/wixtoolset/wix3/releases/tag/wix3141rtm), extract it, and pass `-WixBin C:\path\to\wix314`. If local policy blocks scripts, invoke the file from a trusted checkout with `powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1 ...`; do not lower the machine-wide policy.

WiX runs Windows Installer ICE validation by default. `-SkipValidation` exists only for restricted build containers where the Windows Installer service is unavailable; packages made that way must be rebuilt or ICE-validated on the release VM before publication.

Test installation, upgrade from the previous MSI, GUI launch, CLI from a new terminal, Chinese input/display/search, backup, and uninstall on a clean Windows 10 22H2 VM first. Confirm application/library data is retained after uninstall. Repeat on Windows 11. Windows 10 standard OS support ended in October 2025; record the isolated test image and security limitations.

## macOS Universal 2

Use macOS 12 or newer with Xcode command-line tools, both Rust targets, and mdBook:

```console
rustup target add x86_64-apple-darwin aarch64-apple-darwin
./scripts/package-macos.sh 2.2.1
```

The script combines each binary with `lipo`, creates `LitMan.app`, a DMG, and a PKG. The PKG installs the application plus `/usr/local/bin/litman`. For signing, set `LITMAN_APPLE_IDENTITY` and `LITMAN_INSTALLER_IDENTITY`; notarize the final DMG/PKG with `xcrun notarytool` using credentials kept outside the repository, then staple tickets with `xcrun stapler`.

Smoke-test on both Intel and Apple Silicon, including Gatekeeper, open-PDF, backup, copied-library relocation, and uninstall by removing the app and CLI.

## Ubuntu 22.04+ x64

Install `build-essential`, `pkg-config`, `libx11-dev`, `libxkbcommon-dev`, `libgl1-mesa-dev`, `libdbus-1-dev`, `dpkg-dev`, Rust, and mdBook. Run:

```console
./scripts/package-deb.sh 2.2.1
sudo apt install ./dist/litman_2.2.1_amd64.deb
```

The DEB includes both binaries, the desktop entry, icon, licenses, and manuals. `packaging/icons/litman.svg` is the master; `scripts/generate-icons.ps1` regenerates checked-in PNG/ICO/ICNS assets without adding a runtime graphics dependency. Its runtime dependencies deliberately name X11, OpenGL, D-Bus, and `xdg-utils`.

On Windows, run `powershell -ExecutionPolicy Bypass -File scripts/smoke-pdf-safety.ps1`; on Unix-like builders run `./scripts/smoke-pdf-safety.sh`. Both hash fixtures around ordinary operations and verify the controlled replacement boundary.

## CentOS 7.9 x64 and glibc 2.17

CentOS 7 is end-of-life and its normal mirrors are retired. Install Docker or Podman on the build host, then run:

```console
docker build -t litman-centos7 -f packaging/centos7/Dockerfile .
docker run --rm -v "$PWD:/workspace" -v "$PWD/dist:/out" litman-centos7
```

The pinned `centos:7.9.2009` image rewrites repository definitions to `vault.centos.org`, installs the X11/OpenGL toolchain and Rust version from `rust-toolchain.toml`, vendors Cargo dependencies, and calls the conventional checked-in RPM spec through `rpmbuild`. The GUI is Glow/X11-only. Rust's [x86_64 GNU/Linux target baseline](https://doc.rust-lang.org/stable/nightly-rustc/src/rustc_target/spec/targets/x86_64_unknown_linux_gnu.rs.html) is compatible with glibc 2.17; `scripts/audit-glibc.sh` independently fails if either executable references anything newer. The container then performs a CLI/RPM smoke check.

Container success is not sufficient. Install the RPM in a real CentOS 7.9 VM with kernel 3.10 and an X11 desktop. If a desktop dependency is absent, enable the archived Base, Updates, and Extras Vault repositories shown in the Dockerfile, run `yum clean all`, then `yum localinstall`. Validate GUI launch, CLI, Chinese fonts/input, metadata, missing/error handling, and backup. Record `rpm -q`, `uname -r`, `ldd --version`, and audit output. Compatibility does not provide OS security support; use an isolated VM.

To sign an RPM, set up an isolated RPM macro/GPG key and run `rpm --addsign` on the finished artifact. Never bake private keys into the container.

## CI and release checklist

CI formats, lints, tests, builds both manuals, checks internal links, compiles Windows/macOS/Linux, and builds/audits the CentOS RPM. Hardware/VM smoke tests are recorded as protected release approvals.

Before release:

1. Freeze and audit dependencies and licenses; run the full workspace test suite.
2. Build both manuals and verify search and all internal links.
3. Produce unsigned packages, then sign/notarize from controlled builders if required.
4. Run Windows 10 acceptance first, then Windows 11, CentOS VM, Ubuntu, and both macOS architectures.
5. Copy a closed config/database pair among all platforms and change only the root.
6. Hash every fixture PDF before and after ordinary operations and compare; separately verify explicit replacement preserves displaced hashes in marked `LitMan-backups`, installs the staged publisher hash, and leaves unrelated PDFs unchanged.
7. Test backup/restore and upgrade from the previous database and installer.
8. Publish checksums, source archive, both user manuals, known limitations, and support matrix.

For build failures, first confirm the pinned Rust toolchain and lock file. On Windows use a VS Developer shell if `link.exe` is missing. On Linux install the named development libraries. On CentOS inspect the glibc audit rather than weakening its threshold. On macOS verify both target artifacts exist before `lipo`.
