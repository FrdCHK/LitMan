# Development manual

## Architecture

LitMan is one Cargo workspace with three crates. `litman-core` owns all policy and data access; neither interface writes SQL or parses PDF metadata directly. `litman-cli` maps stable commands to core services. `litman-gui` uses eframe/egui, embeds Noto Sans CJK SC, and runs scans on a worker thread.

The dependency policy favors pure Rust and bundled components. SQLite is bundled through `rusqlite`; `lopdf` reads PDFs; `quick-xml` parses XMP and arXiv Atom; `ureq` and Rustls provide the optional ADS/SciXplorer and arXiv HTTPS clients. There is no Python, Node.js, JVM, database server, or telemetry. New dependencies require a license, maintenance, minimum-platform, binary-size, and security review.

## Configuration and paths

`Config::load` requires schema version 1 and a database filename resolving directly beside the config. Relative `library_root` values resolve from the config directory. `relative_pdf_path` canonicalizes each file, confirms it is inside the canonical root, and serializes components with `/`. Scanner traversal does not follow symlinks.

Never write to a path obtained from a PDF record without first resolving it through the configured root. Ordinary operations treat PDFs as an immutable input boundary. The sole mutation boundary is explicit confirmed PDF replacement: validate a portable bibcode and canonical containment, stage and parse/hash the new PDF with limits, recheck ownership/collisions, journal every move, update the existing record transactionally, and roll back or recover without guessing.

## Data model and migrations

SQLite connections enable foreign keys, a write busy timeout, and `journal_mode=DELETE` so a closed database is a single portable file. `PRAGMA user_version` identifies migrations. Each migration executes in a transaction; add forward migrations rather than editing version 1.

`papers` stores a UUID, normalized relative path, size, modification time, BLAKE3 hash, present/missing/error status, scan diagnostics, timestamps, effective fields, raw embedded metadata JSON, the manual-field set, optional raw BibTeX/bibcode and BibTeX field provenance, optional arXiv ID/raw Atom/arXiv field provenance, and optional importance constrained to 1–5. Authors and keywords are ordered JSON arrays. `groups` is an adjacency list with sibling-name uniqueness. `paper_groups` implements many-to-many membership and cascades when a record or group is removed. Migration v3 adds the arXiv columns and removes legacy BibTeX fields from the manual-provenance set.

Schema changes must preserve databases copied from every supported OS. Add a migration test that starts at the previous `user_version`, upgrades, and verifies foreign keys and constraints.

## Metadata and reconciliation

Extraction reads the PDF trailer Info dictionary and XMP metadata streams. Recognized namespaces include XMP, Dublin Core, and PRISM; unknown raw values are retained for diagnostics. ADS BibTeX and arXiv Atom import retain their raw responses and field-level provenance. Per field, reconciliation uses manual, external ADS/arXiv, XMP, PDF Info, then blank. A later manual edit clears that field's external provenance; resetting clears manual/external provenance and restores embedded data.

`remote_import` owns strict provider/identifier parsing, metadata retrieval, staged PDF validation, collision checks, and the transaction/manifest boundary. ADS tokens are attached only by `ScixplorerClient` to `api.adsabs.harvard.edu` requests. Publisher and arXiv downloads use separate token-free HTTPS agents. Fixed provider filenames and canonical-root checks prevent overwrite/traversal; content-hash duplicate refusal happens again inside the write transaction. A local browser-fallback PDF is copied, never moved or modified.

An incremental scan compares size and modification time, hashes changed files, and extracts metadata when necessary. A unique missing record with the same hash is treated as a move. Multiple same-hash records are marked as duplicate copies and never merged. Files not observed become missing while retaining metadata. Parsing failure affects only that record.

## Search and localization

Search builds a denormalized haystack from effective fields, arrays, notes, path, groups, and importance. Query and haystack use Unicode NFKC and lowercase conversion, then literal substring matching. Do not add language-dependent tokenization in v1.

The shared `Locale` maps human-facing keys to English and Simplified Chinese. JSON models, database enums, field names, and command names remain English. Add both translations in the same change. Test Chinese input and rendered glyphs; the GUI font is loaded before the first frame.

## GUI threading and accessibility

The GUI owns one `Library` on the UI thread. Scan, remote-import, and PDF-replacement workers open their own connection from the config and report messages over channels. The session-added ID sets live only in the GUI view model and are keyed by canonical config path; they are never serialized. Remote import and scans use atomic cancellation flags. Keep labels attached to editors, retain keyboard navigation and focus indication, and expose actions as text rather than color alone.

Windows builds include WGPU and Glow. Startup prefers WGPU and retries once with Glow. Linux release builds use X11/Glow to maintain the CentOS baseline.

## Tests and fixtures

Run:

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Unit tests cover config/path rules, migrations, identifier parsing, metadata precedence, manual overrides, ratings, nested groups, Unicode/Chinese search, and scan reconciliation. Remote tests use deterministic local endpoints for ADS JSON/BibTeX/esources, arXiv Atom, redirects, unavailable/login responses, limits, malformed data, cancellation, and duplicate refusal. CLI integration tests assert stable JSON and localized tables. GUI tests isolate session-filter logic before platform smoke tests.

Generate the deterministic fixture set with `cargo run -p litman-core --example generate_fixtures -- target/litman-fixtures`. `SCENARIOS.txt` describes the rename/removal steps for move and missing-file tests; the encrypted fixture password is recorded there.

Every acceptance run hashes fixture PDFs before and after ordinary operations; any difference is release-blocking. Explicit replacement tests are the controlled exception: only the selected active path may change, every displaced hash must reappear under marked `LitMan-backups`, the installed hash must match the staged publisher fixture, and unrelated PDFs must remain unchanged.

## Security and contributions

Treat PDF contents, metadata, BibTeX, and web responses as hostile. Keep parser limits, avoid shell interpolation, validate paths and bibcodes, parameterize SQL, bound web calls, and display errors as text. Never log or display the configured bearer token. The token is deliberately stored as plain text only in the portable TOML configuration, so documentation must warn users about config copies, backups, shell history, and source control. Report vulnerabilities privately to the maintainers listed by the distribution channel.

Contributions use a focused branch, formatted code, tests, both translations, and documentation where behavior changes. Review migrations, filesystem boundaries, JSON compatibility, supported-platform dependencies, the ordinary PDF-read-only boundary, and the replacement confirmation/staging/rollback boundary before merge.
