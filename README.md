# LitMan

LitMan is a local-first literature manager for existing PDF directories. It provides a native GUI and a scriptable CLI and stores bibliographic edits in a portable SQLite database. Ordinary operations never modify, move, or delete PDFs. An optional SciXplorer integration can search ADS, import BibTeX metadata, and replace a selected preprint with a validated publisher PDF while preserving displaced files.

The interface, CLI messages, and offline manuals support English and Simplified Chinese. Windows 10 22H2 x64 is the primary release target; Windows 11, Ubuntu 22.04+ are also supported. You can also build for your operating system.

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

LitMan has no telemetry or database server. It requires no network for ordinary library work. Explicit SciXplorer search/import sends the configured token only to the ADS API. Explicit **Update PDF** contacts SciXplorer's unauthenticated publisher gateway and the resolved publisher, never forwards the ADS token, and may open a browser for publisher authentication. A configured token is stored as plain text in the library TOML and its config/database backups.

Apart from the separately confirmed **Update PDF** action, PDFs remain read-only. Replacement stages and validates the download, moves every displaced file into the marked top-level `LitMan-backups` directory, updates only the selected database record, and uses a recovery manifest. Config/database backups do not include the PDF tree or `LitMan-backups`; back these up independently. Removing a LitMan record still does not remove its PDF.

LitMan is licensed under the GNU General Public License version 3; see `LICENSE`. The bundled Noto Sans CJK SC font has its own SIL Open Font License in `crates/litman-gui/assets/LICENSE-NOTO.txt`.
