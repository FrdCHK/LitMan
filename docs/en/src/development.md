# Development manual

## Architecture

LitMan is one Cargo workspace with three crates. `litman-core` owns all policy and data access; neither interface writes SQL or parses PDF metadata directly. `litman-cli` maps stable commands to core services. `litman-gui` uses eframe/egui, embeds Noto Sans CJK SC, and runs scans on a worker thread.

The dependency policy favors pure Rust and bundled components. SQLite is bundled through `rusqlite`; `lopdf` reads PDFs; `quick-xml` parses XMP; `ureq` and Rustls provide the optional ADS/SciXplorer HTTPS client. There is no Python, Node.js, JVM, database server, or telemetry. New dependencies require a license, maintenance, minimum-platform, binary-size, and security review.

## Configuration and paths

`Config::load` requires schema version 1 and a database filename resolving directly beside the config. Relative `library_root` values resolve from the config directory. `relative_pdf_path` canonicalizes each file, confirms it is inside the canonical root, and serializes components with `/`. Scanner traversal does not follow symlinks.

Never write to a path obtained from a PDF record without first resolving it through the configured root. PDFs are an immutable input boundary.

## Data model and migrations

SQLite connections enable foreign keys, a write busy timeout, and `journal_mode=DELETE` so a closed database is a single portable file. `PRAGMA user_version` identifies migrations. Each migration executes in a transaction; add forward migrations rather than editing version 1.

`papers` stores a UUID, normalized relative path, size, nanosecond modification time, BLAKE3 hash, present/missing/error status, scan diagnostics, timestamps, effective fields, raw embedded metadata JSON, the manual-field set, optional raw BibTeX/bibcode and BibTeX field-provenance set, and optional importance constrained to 1–5. Authors and keywords are ordered JSON arrays. `groups` is an adjacency list with sibling-name uniqueness. `paper_groups` implements many-to-many membership and cascades when a record or group is removed.

Schema changes must preserve databases copied from every supported OS. Add a migration test that starts at the previous `user_version`, upgrades, and verifies foreign keys and constraints.

## Metadata and reconciliation

Extraction reads the PDF trailer Info dictionary and XMP metadata streams. Recognized namespaces include XMP, Dublin Core, and PRISM; unknown raw values are retained in imported metadata for diagnostics. BibTeX import parses the ADS citation key and common bibliographic fields while retaining the raw export unchanged. Per field, reconciliation uses the most recent manual or BibTeX override, then XMP, PDF Info, and blank. Manual-field membership—not merely a nonempty value—protects both user and BibTeX values from rescans; the separate BibTeX field set provides accurate provenance and is cleared for a field after a later manual edit.

An incremental scan compares size and modification time, hashes changed files, and extracts metadata when necessary. A unique missing record with the same hash is treated as a move. Multiple same-hash records are marked as duplicate copies and never merged. Files not observed become missing while retaining metadata. Parsing failure affects only that record.

## Search and localization

Search builds a denormalized haystack from effective fields, arrays, notes, path, groups, and importance. Query and haystack use Unicode NFKC and lowercase conversion, then literal substring matching. Do not add language-dependent tokenization in v1.

The shared `Locale` maps human-facing keys to English and Simplified Chinese. JSON models, database enums, field names, and command names remain English. Add both translations in the same change. Test Chinese input and rendered glyphs; the GUI font is loaded before the first frame.

## GUI threading and accessibility

The GUI owns one `Library` on the UI thread. A scan worker opens its own connection from the config and reports bounded progress messages over a channel. SciXplorer workers receive only a cloned API client and query/bibcode; they return data over a channel, while SQLite writes remain on the UI thread. HTTPS calls use a 30-second global timeout. Cancellation is an atomic flag checked between files. Keep labels attached to editors, retain keyboard navigation and focus indication, and expose actions as text rather than color alone.

Windows builds include WGPU and Glow. Startup prefers WGPU and retries once with Glow. Linux release builds use X11/Glow to maintain the CentOS baseline.

## Tests and fixtures

Run:

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Unit tests cover config/path rules, migrations, metadata precedence, manual overrides, ratings, nested groups, Unicode/Chinese search, and scan reconciliation. Fixture PDFs must cover Info, XMP/PRISM, Chinese fields and filenames, no metadata, conflicts, malformed and encrypted files, duplicates, moves, and missing files. CLI integration tests assert JSON keys and localized tables. GUI tests should isolate view-model behavior from rendering, then use platform smoke tests for launch, keyboard access, cancellation, and system PDF opening.

Generate the deterministic fixture set with `cargo run -p litman-core --example generate_fixtures -- target/litman-fixtures`. `SCENARIOS.txt` describes the rename/removal steps for move and missing-file tests; the encrypted fixture password is recorded there.

Every acceptance run hashes fixture PDFs before and after all operations; any difference is release-blocking.

## Security and contributions

Treat PDF contents, metadata, BibTeX, and web responses as hostile. Keep parser limits, avoid shell interpolation, validate paths and bibcodes, parameterize SQL, bound web calls, and display errors as text. Never log or display the configured bearer token. The token is deliberately stored as plain text only in the portable TOML configuration, so documentation must warn users about config copies, backups, shell history, and source control. Report vulnerabilities privately to the maintainers listed by the distribution channel.

Contributions use a focused branch, formatted code, tests, both translations, and documentation where behavior changes. Review migrations, filesystem boundaries, JSON compatibility, supported-platform dependencies, and PDF immutability before merge.
