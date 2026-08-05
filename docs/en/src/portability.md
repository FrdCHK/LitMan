# Portability, privacy, and recovery

A portable LitMan library is the TOML configuration plus the SQLite file named by it. Keep these two files together and keep `database` as a plain adjacent filename. The PDF directory is independent and may be large, removable, or synchronized by another tool. It may be read-only for ordinary operation, but explicit PDF replacement requires write access to the selected paper's directory and library root.

Relative paths in the database always use `/`. At open time LitMan combines them with the current `library_root`; it never stores a Windows drive letter for an item when that item is inside the root. This lets one closed database move between Windows, macOS, Ubuntu, and CentOS. After copying, change only the root and scan.

SQLite uses DELETE journaling, foreign keys, and a busy timeout. Manual filesystem copying is safe only after all LitMan processes close. Online backup is safe while running because it uses SQLite's backup API. Preserve the config and database from the same backup operation.

LitMan has no telemetry. PDF metadata, notes, paths, groups, and ratings remain local. Optional search/import sends the configured bearer token only to the ADS API. Explicit PDF replacement contacts SciXplorer's gateway and the resolved publisher without forwarding that token; login fallback opens the system browser. The browser is also used for local manuals, PDFs, and stored SciXplorer links.

For disaster recovery, preserve multiple dated config/database pairs, verify them periodically on a separate machine, and include the PDF tree in an independent backup policy. The online config/database backup does not include the PDF tree or the replacement directory `LitMan-backups`. Replacement backups are unmanaged and require manual recovery. Restoring a database cannot recreate a missing PDF; rescanning PDFs cannot recreate manual notes or grouping from a lost database.
