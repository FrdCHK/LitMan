# LitMan manuals

LitMan manages papers already stored as PDF files. It scans embedded metadata, lets you correct or complete it without touching the PDF, and organizes papers with nested groups and a 1–5-star importance rating.

LitMan is written by Jingdong Zhang and licensed under the GNU General Public License version 3. The complete license is included in `LICENSE` and every release package.

These manuals are generated as searchable, offline HTML. The installer includes this English edition and the complete Simplified Chinese edition.

- Readers should start with the [user manual](user.md).
- Shell users can jump to the [CLI reference](cli.md).
- Contributors should read the [development manual](development.md).
- Release engineers should use the [build and release manual](building.md).

Ordinary LitMan operations do not modify, move, or delete PDFs. The one explicit exception is the separately warned and confirmed **Update PDF** action, which preserves displaced files in `LitMan-backups`. Editable information lives in the SQLite database beside the configuration file.
