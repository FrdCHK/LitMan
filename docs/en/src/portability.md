# Portability, privacy, and recovery

A portable LitMan library is the TOML configuration plus the SQLite file named by it. Keep these two files together and keep `database` as a plain adjacent filename. The PDF directory is independent and may be large, read-only, removable, or synchronized by another tool.

Relative paths in the database always use `/`. At open time LitMan combines them with the current `library_root`; it never stores a Windows drive letter for an item when that item is inside the root. This lets one closed database move between Windows, macOS, Ubuntu, and CentOS. After copying, change only the root and scan.

SQLite uses DELETE journaling, foreign keys, and a busy timeout. Manual filesystem copying is safe only after all LitMan processes close. Online backup is safe while running because it uses SQLite's backup API. Preserve the config and database from the same backup operation.

LitMan sends nothing over the network. PDF metadata, notes, paths, groups, and ratings remain local. The only external program invoked at runtime is the operating system's configured viewer or browser for a local PDF/manual.

For disaster recovery, preserve multiple dated backup pairs, verify them periodically on a separate machine, and include the PDF tree in an independent backup policy. Restoring a database cannot recreate a missing PDF. Conversely, rescanning PDFs cannot recreate manual notes or grouping from a lost database.
