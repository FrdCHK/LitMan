# User manual

## Install and uninstall

On Windows, run the MSI and choose whether the `litman` CLI is added to `PATH`. A newer MSI, including a changed same-version rebuild, upgrades the installed copy without a manual uninstall. Alternatively, run the standalone portable EXE without installation; its **Manual** button falls back to an embedded, searchable English or Chinese user manual when no adjacent manual directory exists. Use the portable ZIP when you also want the CLI and complete offline mdBook manuals. Launch an installed copy from the Start Menu. Remove it from **Installed apps**; uninstalling the program does not remove libraries or PDFs. [Windows 10 standard support ended in October 2025](https://learn.microsoft.com/en-us/lifecycle/announcements/windows-10-22h2-end-of-support-update), but LitMan continues to test Windows 10 22H2 as its primary target.

Choose **About** in the toolbar to see the software version, author Jingdong Zhang, and GNU GPLv3 license information.

On macOS, open the DMG and copy LitMan to Applications, or use the PKG to install both the application and `/usr/local/bin/litman`. On Ubuntu, install with `sudo apt install ./litman_VERSION_amd64.deb`. On CentOS 7, install with `sudo yum localinstall litman-VERSION.x86_64.rpm`. [CentOS 7 reached end of life in June 2024](https://www.centos.org/centos-linux/); LitMan compatibility cannot make that operating system secure.

## Create and open a library

Choose **New library**, select a location for `library.toml`, then choose the directory containing PDFs. The config and its database are written together:

```toml
schema_version = 1
database = "literature.sqlite3"
library_root = "../pdfs"
language = "system"
```

The database filename must remain beside the config. A relative root is resolved from the config directory; an absolute root is also allowed. A library can cover nested directories.

## Scan

LitMan starts an incremental scan after opening a library. **Scan** repeats it, while **Refresh metadata** also rereads embedded metadata for unchanged files. Scanning:

- does not follow directory symlinks and rejects paths outside the canonical root;
- skips unchanged files by size and modification time;
- hashes new and changed files and recognizes unambiguous moves;
- retains records and metadata for missing files;
- flags duplicate copies instead of merging them;
- records a malformed or encrypted file as an error without stopping other files.

The GUI remains usable during a scan and offers **Cancel**. Cancellation preserves completed records and can be followed by another scan.

## Metadata and manual corrections

Select a row to edit title, ordered authors, abstract, date, journal or conference, volume, issue, pages, DOI, URL, language, ordered keywords, and notes. Authors and keywords use semicolons in the GUI. Press **Save**.

LitMan reads PDF Info and common XMP, Dublin Core, and PRISM fields. The effective value is chosen in this order:

1. manual value;
2. XMP value;
3. PDF Info value;
4. blank.

A rescan never overwrites a manual value. **Reset to PDF** removes the selected field's manual override and reveals the current embedded value. For a PDF without metadata, its filename appears only as a visual placeholder; it is not saved as the title until you enter one.

## Find and organize papers

The search box matches literal text in metadata, authors, keywords, notes, filename/path, and group names. Text is Unicode NFKC-normalized and case-folded. Chinese searches work as substring searches without requiring word segmentation.

Create nested group paths such as `Research/Imaging`. A duplicate path is rejected with a warning, while successful creation is reported in the notification area. A paper may belong to several groups. In the center list, click a row to select it or use Ctrl/Command-click to select several. In the always-visible **Assign selected papers** section on the left, choose the target group and click **Add to group** or **Remove from group**. Clicking a group in the group tree filters the list; it does not assign papers. Select a group and click **Rename group** to enter a new name in a small window; names must be unique among groups under the same parent. Renaming preserves nested groups and assignments. To delete the selected group and all groups nested below it, click **Delete group** and confirm. Group assignments are removed, but paper records and PDFs are not deleted.

Drag a divider between column headings to adjust the literature-list column widths. The adjusted widths remain in effect for the current LitMan session.

Importance is optional. Choose one through five stars, where five means most important; `×` clears it. Multi-selection changes all selected papers. The English label is **Importance** and the Simplified Chinese label is **重要程度**.

Status filters show present, missing, or error records. Importance and group filters can be combined with search.

## Open and remove

**Open** asks the operating system to open the PDF in its default viewer. **Remove database record** only removes LitMan's record after confirmation. LitMan v1 has no command that edits, moves, or deletes a PDF.

## Backup, copy, and restore

Use **Backup** or `litman backup DESTINATION` while LitMan is running. This uses SQLite's online backup API and writes a consistent config/database pair. For a manual copy, close all LitMan processes first, then copy the TOML and SQLite files together.

On another computer, copy or mount the PDF directory, open the copied TOML, and use **Relocate root** (or `litman root set DIR`). Relative PDF paths use `/`, so only the root changes. Do not separately combine a config from one backup with a database from another.

Restore by closing LitMan and replacing both files with a matching backup pair. Keep an extra copy until the restored library opens and scans successfully.

## Upgrades and errors

Opening a newer LitMan database with an older program may be refused. Before upgrading, create a backup. Migrations run transactionally and never change PDFs.

If a file is missing, confirm the root and scan again. For a parse error, the diagnostic remains visible; enter metadata manually if needed. If the database is busy, close other LitMan processes and retry. If a GUI renderer fails on Windows, LitMan retries with OpenGL automatically.

All data stays local. LitMan performs no telemetry and needs no network connection at runtime.
