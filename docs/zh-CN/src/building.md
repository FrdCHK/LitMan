# 构建与发布手册

## 可复现输入

通过 rustup 安装 Rust，使用已提交的 `rust-toolchain.toml` 和 `Cargo.lock`。发布命令使用 `--locked`；CentOS 构建还会把 crates 放入 vendor。安装 mdBook 以生成两种离线手册：

```console
cargo install mdbook --locked
mdbook build docs/en
mdbook build docs/zh-CN
cargo build --workspace --release --locked
```

默认生成未签名安装包。应记录源码修订、Rust 版本、打包工具版本、目标三元组和 SHA-256。请从干净源码包构建，并与产物一起保留生成的手册。

## Windows 10/11 x64（首要发布门槛）

准备 Windows 10 22H2 或 Windows 11 x64、Visual Studio 2022 Build Tools（“使用 C++ 的桌面开发”及 Windows SDK）、Rust MSVC x64、mdBook 和 WiX Toolset 3.14。在 Developer PowerShell 中运行：

```powershell
./scripts/package-windows.ps1 -Version 0.1.2
```

脚本会构建两个程序和两种手册，并生成 `dist/LitMan-0.1.2-x64.msi`、单文件 `dist/LitMan-0.1.2-portable-x64.exe`，以及包含 `LitMan.exe`、`litman-cli.exe`、许可证和离线手册的便携版 ZIP。MSI 包含 GUI、CLI、开始菜单快捷方式、本地手册、卸载信息，以及可选的 CLI PATH 组件。设置 `LITMAN_CERT_THUMBPRINT` 并传入 `-Sign` 可调用 `signtool`；时间戳地址由 `LITMAN_TIMESTAMP_URL` 控制。

MSI ProductCode 根据版本号和源码内容确定生成。WiX 允许同版本重大升级，因此源码发生变化后的重构建可以直接覆盖已安装版本，无需手工卸载；完全相同的源码仍能复现同一 ProductCode。所有版本必须保持 UpgradeCode 不变。

WiX 也可以免安装使用：下载官方 [`wix314-binaries.zip`](https://github.com/wixtoolset/wix3/releases/tag/wix3141rtm) 并解压，然后传入 `-WixBin C:\路径\wix314`。如果本地策略阻止脚本，请只对可信源码使用 `powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1 ...`，不要降低整台电脑的执行策略。

WiX 默认执行 Windows Installer ICE 验证。`-SkipValidation` 只用于无法访问 Windows Installer 服务的受限构建容器；由此生成的包必须在发布 VM 上重新构建或完成 ICE 验证后才能发布。

首先在全新 Windows 10 22H2 VM 中测试安装、从上一 MSI 升级、GUI 启动、新终端中的 CLI、中文输入/显示/搜索、备份和卸载；确认卸载后文献库数据仍在。之后在 Windows 11 重复。Windows 10 常规支持已于 2025 年 10 月结束，应记录隔离测试镜像和安全限制。

## macOS Universal 2

使用 macOS 12 或更高版本、Xcode 命令行工具、两个 Rust 目标和 mdBook：

```console
rustup target add x86_64-apple-darwin aarch64-apple-darwin
./scripts/package-macos.sh 0.1.2
```

脚本用 `lipo` 合并程序，创建 `LitMan.app`、DMG 和 PKG。PKG 安装应用以及 `/usr/local/bin/litman`。签名时设置 `LITMAN_APPLE_IDENTITY` 与 `LITMAN_INSTALLER_IDENTITY`；最终 DMG/PKG 使用 `xcrun notarytool` 和仓库外凭据公证，再用 `xcrun stapler` 附加票据。

在 Intel 与 Apple Silicon 上测试 Gatekeeper、打开 PDF、备份、复制库后的根目录迁移，以及删除应用和 CLI 的卸载流程。

## Ubuntu 22.04+ x64

安装 `build-essential`、`pkg-config`、`libx11-dev`、`libxkbcommon-dev`、`libgl1-mesa-dev`、`libdbus-1-dev`、`dpkg-dev`、Rust 和 mdBook，然后运行：

```console
./scripts/package-deb.sh 0.1.2
sudo apt install ./dist/litman_0.1.2_amd64.deb
```

DEB 包括两个程序、桌面入口、图标、许可证和手册。运行依赖明确列出 X11、OpenGL、D-Bus 和 `xdg-utils`。必须在最旧受支持 Ubuntu 和当前版本上测试。

## CentOS 7.9 x64 与 glibc 2.17

CentOS 7 已停止维护，普通镜像已下线。在构建主机安装 Docker 或 Podman，然后运行：

```console
docker build -t litman-centos7 -f packaging/centos7/Dockerfile .
docker run --rm -v "$PWD:/workspace" -v "$PWD/dist:/out" litman-centos7
```

固定的 `centos:7.9.2009` 镜像把软件源改到 `vault.centos.org`，安装 X11/OpenGL 工具链和 `rust-toolchain.toml` 指定的 Rust，对 Cargo 依赖进行 vendor，然后通过提交的传统 RPM spec 调用 `rpmbuild`。GUI 仅使用 Glow/X11。Rust 的 [x86_64 GNU/Linux 目标基线](https://doc.rust-lang.org/stable/nightly-rustc/src/rustc_target/spec/targets/x86_64_unknown_linux_gnu.rs.html)兼容 glibc 2.17；`scripts/audit-glibc.sh` 还会独立检查并在任一程序引用更高版本符号时失败。容器随后进行 CLI/RPM 冒烟检查。

仅通过容器还不够。必须在内核 3.10、带 X11 桌面的真实 CentOS 7.9 VM 中安装 RPM。如果桌面依赖缺失，启用 Dockerfile 所示的 Vault Base、Updates、Extras 源，执行 `yum clean all`，再运行 `yum localinstall`。验证 GUI 启动、CLI、中文字体/输入、元数据、缺失/错误处理和备份。记录 `rpm -q`、`uname -r`、`ldd --version` 及审计输出。兼容性不提供操作系统安全维护，请使用隔离 VM。

RPM 签名应在隔离环境配置 RPM 宏/GPG 密钥，再对产物运行 `rpm --addsign`。禁止把私钥写入容器。

## CI 与发布清单

CI 进行格式化、lint、测试、构建两种手册、检查内部链接、编译 Windows/macOS/Linux，并构建和审计 CentOS RPM。硬件/VM 冒烟结果作为受保护的发布审批。

发布前：

1. 冻结并审计依赖与许可证，运行完整 workspace 测试。
2. 构建两种手册，验证搜索和全部内部链接。
3. 先生成未签名包；如需要，再在受控构建机签名/公证。
4. 首先执行 Windows 10 验收，再执行 Windows 11、CentOS VM、Ubuntu 和两种 macOS 架构。
5. 在所有平台间复制已关闭的配置/数据库文件对，只更改根目录。
6. 比较整个矩阵执行前后每个 PDF 样例的哈希。
7. 测试备份/恢复以及从上一数据库和安装包升级。
8. 发布校验和、源码包、两种用户手册、已知限制和支持矩阵。

构建失败时先检查固定 Rust 工具链与锁文件。Windows 找不到 `link.exe` 时应使用 VS Developer shell。Linux 应安装上述开发库。CentOS 应分析 glibc 审计结果，不能放宽阈值。macOS 在运行 `lipo` 前应确认两个目标产物都存在。
