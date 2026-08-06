# LitMan

LitMan is a local-first literature manager for PDF directories. It provides a native GUI and scriptable CLI and stores bibliographic edits in a portable SQLite database. It can import a PDF plus metadata from ADS/SciXplorer or arXiv, and its independent **Newly added** filter tracks papers created during the current GUI session. Existing PDFs remain read-only except for the separately confirmed publisher-update workflow.

The interface, CLI messages, and offline manuals support English and Simplified Chinese. Windows 10 22H2 x64 is the primary release target; Windows 11, Ubuntu 22.04+, CentOS 7.9 x64, and macOS 12+ are also supported as documented.

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

LitMan has no telemetry or database server. It requires no network for ordinary library work. SciXplorer search/import sends the configured token only to the ADS API; arXiv import needs no token. Remote imports use HTTPS, fixed collision-refusing destinations, staged PDF validation, and an all-or-nothing database/file commit. Publisher requests and arXiv never receive the ADS token. A configured token is stored as plain text in the library TOML and its config/database backups.

Online import may create one new PDF; it never changes an existing PDF or a browser-selected source file. Apart from the separately confirmed **Update PDF** action, existing PDFs remain read-only. Config/database backups do not include the PDF tree or `LitMan-backups`; back these up independently. Removing a LitMan record still does not remove its PDF.

LitMan is licensed under the GNU General Public License version 3; see `LICENSE`. The bundled Noto Sans CJK SC font has its own SIL Open Font License in `crates/litman-gui/assets/LICENSE-NOTO.txt`.
