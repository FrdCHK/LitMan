# LitMan

LitMan is a local-first literature manager for existing PDF directories. It provides a native GUI and a scriptable CLI, stores bibliographic edits in a portable SQLite database, and never modifies, moves, or deletes a PDF. An optional personal-token integration can search SciXplorer through the ADS Developer API, import BibTeX metadata, and retain citations locally.

Author: Jingdong Zhang

The interface, CLI messages, and offline manuals support English and Simplified Chinese. Windows 10 22H2 x64 is the primary release target; Windows 11, macOS 12+, Ubuntu 22.04+, and CentOS 7.9 are also supported.

## Quick start

```console
cargo build --workspace
cargo run -p litman-cli -- init --config library.toml --root D:/papers
cargo run -p litman-cli -- --config library.toml scan
cargo run -p litman-gui -- --config library.toml
```

Use `litman --config FILE manual` for the bundled manual and `litman --help` for command syntax. Configuration and database files are portable together; PDF paths are stored relative to `library_root` with `/` separators.

## Project layout

- `crates/litman-core`: configuration, SQLite, migrations, metadata, scanning, search, groups, ratings, and backup.
- `crates/litman-cli`: the `litman` command.
- `crates/litman-gui`: the `litman-gui` native desktop program.
- `docs`: English and Simplified Chinese mdBook sources.
- `packaging`: WiX, macOS, Debian, and RPM definitions.
- `scripts`: reproducible package-build entry points.

Build and release prerequisites are documented in [the English build manual](docs/en/src/building.md) and [简体中文构建手册](docs/zh-CN/src/building.md).

## Safety and privacy

LitMan has no telemetry or database server. It requires no network for ordinary library work; only explicit optional SciXplorer searches/imports contact the ADS API, and opening a stored SciXplorer link uses the system browser. A configured personal API token is stored as plain text in the library TOML and its backups. LitMan opens PDFs through the operating system viewer but writes only its TOML configuration, SQLite database, and requested backups. Removing a LitMan record does not remove its PDF.

Copyright © 2026 Jingdong Zhang. LitMan is licensed under the GNU General Public License version 3; see `LICENSE`. The bundled Noto Sans CJK SC font has its own SIL Open Font License in `crates/litman-gui/assets/LICENSE-NOTO.txt`.
