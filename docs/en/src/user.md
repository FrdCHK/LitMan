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
# Optional; obtain this personal token from the ADS user settings page.
scixplorer_api_token = "your-personal-token"
```

The `scixplorer_api_token` line is optional and is omitted from newly created configurations. The database filename must remain beside the config. A relative root is resolved from the config directory; an absolute root is also allowed. A library can cover nested directories.

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
2. ADS/SciXplorer BibTeX or arXiv Atom value;
3. XMP value;
4. PDF Info value;
5. blank.

A rescan never overwrites a manual or externally imported value. **Reset to embedded PDF** removes the selected field's manual and external provenance and reveals the current embedded value. For a PDF without metadata, its filename appears only as a visual placeholder; it is not saved as the title until you enter one.

## Import a paper from ADS/SciXplorer or arXiv

Choose **Import paper** and enter one identifier or URL. LitMan auto-detects ADS bibcodes such as `2003ApJ...587..208R`, modern or legacy arXiv IDs such as `0908.3637` or `astro-ph/9901234v2`, ADS abstract URLs, and arXiv `/abs/` or `/pdf/` URLs. arXiv import needs no credentials. ADS import is enabled only when a personal ADS token is configured under **SciXplorer settings**.

An ADS import retrieves exact-record BibTeX and its `esources` list. It tries advertised sources in the order `PUB_PDF`, `EPRINT_PDF`, then `ADS_PDF`, moving to the next source only when the earlier one is explicitly unavailable. A publisher login or HTML page opens the existing browser fallback: download the publisher PDF, then choose **Select downloaded PDF**; the selected source file remains unchanged. An arXiv import retrieves the Atom record and PDF together, including ordered authors, abstract, date, DOI, journal reference, categories, canonical URL, and raw Atom response.

Successful files are created directly under the library root as `BIBCODE.pdf` or `arXiv-ID.pdf` (legacy `/` becomes `_`). LitMan never overwrites or invents a numbered name: an existing normalized provider ID, destination path, or content hash refuses the complete import. Metadata and PDF commit together; cancellation or failure leaves no record or destination. Network downloads are HTTPS-only, bounded to 256 MiB, checked for `%PDF-`, parsed, and hashed before installation. ADS tokens are sent only to ADS API requests and never to publisher gateways, arXiv, or redirects.

## Optional SciXplorer metadata and BibTeX

LitMan can use the [ADS Developer API](https://github.com/adsabs/adsabs-dev-api) that supplies SciXplorer data. This feature is completely optional. With no token configured, LitMan performs no SciXplorer/ADS requests and the metadata sidebar's **SciXplorer** button is disabled.

Generate a personal API token in the [ADS token settings](https://ui.adsabs.harvard.edu/#user/settings/token). In the GUI choose **SciXplorer settings**, enter the token, and press **Save token**. The token is stored as plain text in this library's `library.toml`; it is also included when the configuration is backed up or copied. Protect that file like a password and do not commit or share it. **Remove token** disables new searches and imports without removing BibTeX already stored in the database.

Select a paper and click **SciXplorer** in the metadata sidebar. Search by title, DOI, or ADS/SciXplorer bibcode. LitMan shows up to 20 matching records. Clicking **Use** downloads that record's BibTeX, stores the unmodified BibTeX in SQLite, and fills available title, ordered authors, abstract, publication date, journal or conference, volume, issue, pages, DOI, URL, language, and keywords. Imported values replace the corresponding current values; fields absent from the new BibTeX return to their embedded PDF values if they came from a previous BibTeX import. Later manual edits take precedence and are marked as manual provenance.

When BibTeX is stored, **BibTeX** copies it to the system clipboard and displays a confirmation. **Open SciXplorer** opens `https://scixplorer.org/abs/BIBCODE/abstract` in the default web browser. Both buttons remain available without an API token because they use data already stored locally.

### Replace a preprint with the publisher PDF

For one present paper with a stored bibcode, **Update PDF** remains available even after the API token is removed. It uses `https://scixplorer.org/link_gateway/BIBCODE/PUB_PDF`; the ADS bearer token is never attached to that request or publisher redirects. The centered warning lists the selected PDF and backup, any separate untracked `BIBCODE.pdf` and its additional backup, and the final `BIBCODE.pdf`. Close PDF viewers, read the warning, tick the acknowledgment, then click the red **Replace PDF** button. Cancel starts no download and makes no filesystem or database change.

LitMan downloads to a same-filesystem temporary file, enforces HTTPS, redirect/time and 256 MiB limits, checks the `%PDF-` header, parses the PDF, and hashes it before moving anything. The active file is always exactly `BIBCODE.pdf`, never a numbered variant. If that name is an untracked file, it is preserved too; if another LitMan record owns it, replacement is refused without changes. A publisher login/HTML response keeps the dialog open and offers **Open publisher link**, **Select downloaded PDF**, and **Cancel**. A selected external download is copied and validated; its source remains untouched.

Displaced files go to the library root's `LitMan-backups`: first `BIBCODE_bk.pdf`, then `BIBCODE_bk_2.pdf`, and so on. LitMan marks directories it creates or adopts while empty and excludes only a marked backup directory from scans. A nonempty unmarked directory with that reserved name blocks replacement and remains scannable. Backups are unmanaged and must be recovered manually. A pending manifest lets startup, scans, and later replacements safely roll back a pre-commit interruption or finish cleanup after a committed database update; unexpected hashes are preserved and reported rather than guessed.

## Find and organize papers

The search box matches literal text in metadata, authors, keywords, notes, filename/path, and group names. Text is Unicode NFKC-normalized and case-folded. Chinese searches work as substring searches without requiring word segmentation.

Create nested group paths such as `Research/Imaging`. A duplicate path is rejected with a warning, while successful creation is reported in the notification area. A paper may belong to several groups. In the center list, click a row to select it or use Ctrl/Command-click to select several. In the always-visible **Assign selected papers** section on the left, choose the target group and click **Add to group** or **Remove from group**. Clicking a group in the group tree filters the list; it does not assign papers. Select a group and click **Rename group** to enter a new name in a small window; names must be unique among groups under the same parent. Renaming preserves nested groups and assignments. To delete the selected group and all groups nested below it, click **Delete group** and confirm. Group assignments are removed, but paper records and PDFs are not deleted.

Drag a divider between column headings to adjust the literature-list column widths. The adjusted widths remain in effect for the current LitMan session.

Importance is optional. Choose one through five stars, where five means most important; `×` clears it. Multi-selection changes all selected papers. The English label is **Importance** and the Simplified Chinese label is **重要程度**.

Status filters show present, missing, or error records. Importance and group filters can be combined with search.

**Newly added** (**本次新增**) is an independent GUI filter that combines with search, group, importance, and status filters. It contains records first created by scans or successful online imports since this LitMan process started. Existing records, recognized moves, metadata-only SciXplorer updates, and PDF updates are not included. The set is kept separately for each library while the process runs and is discarded when LitMan exits.

## Open and remove

**Open** asks the operating system to open the PDF in its default viewer. **Remove database record** only removes LitMan's record after confirmation. All ordinary actions keep PDFs read-only; only the explicit confirmed **Update PDF** workflow moves and replaces them as described above.

## Backup, copy, and restore

Use **Backup** or `litman backup DESTINATION` while LitMan is running. This uses SQLite's online backup API and writes a consistent config/database pair. It does not include the PDF tree or `LitMan-backups`; protect both with an independent file-backup policy. For a manual copy, close all LitMan processes first, then copy the TOML and SQLite files together.

On another computer, copy or mount the PDF directory, open the copied TOML, and use **Relocate root** (or `litman root set DIR`). Relative PDF paths use `/`, so only the root changes. Do not separately combine a config from one backup with a database from another.

Restore by closing LitMan and replacing both files with a matching backup pair. Keep an extra copy until the restored library opens and scans successfully.

## Upgrades and errors

Opening a newer LitMan database with an older program may be refused. Before upgrading, create a backup. Database migrations run transactionally and do not change PDFs.

If a file is missing, confirm the root and scan again. For a parse error, the diagnostic remains visible; enter metadata manually if needed. If the database is busy, close other LitMan processes and retry. If a GUI renderer fails on Windows, LitMan retries with OpenGL automatically.

LitMan performs no telemetry. All ordinary library, PDF, search, organization, and backup work stays local. SciXplorer search/import sends the configured token and query/bibcode to the ADS API. arXiv import sends the requested ID to `export.arxiv.org` and downloads the PDF from `arxiv.org`. Online import and explicit publisher replacement contact the selected remote sources without forwarding the ADS token; browser fallback also contacts those sites through the system browser.
