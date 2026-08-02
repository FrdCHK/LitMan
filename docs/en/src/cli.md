# CLI reference

The global config defaults to `library.toml` in the current directory:

```console
litman --config PATH [--language system|en|zh-CN] COMMAND
```

Human output, help, prompts, and errors are localized. JSON keys and enum values remain English and stable for automation. A paper may be named by its full UUID or an unambiguous UUID prefix.

The human-readable output of `list` and `search` is intentionally compact: it contains the short **ID**, **Title**, **First author**, and **Year**. When a paper has multiple authors, the author cell uses the first name followed by `et al.`. It does not display importance or file availability; use `show` for one paper's full details or `--format json` for all stable fields.

```text
litman init --config FILE --root DIR [--language system|en|zh-CN]
litman scan [--refresh-metadata]
litman list [--group PATH] [--importance N] [--status present|missing|error] [--format table|json]
litman search QUERY [--group PATH] [--min-importance N] [--format table|json]
litman show PAPER_ID [--format table|json]
litman edit PAPER_ID [metadata options] [--author VALUE ...] [--keyword VALUE ...] [--interactive] [--clear FIELD ...]
litman rate PAPER_ID 1|2|3|4|5|clear
litman group list
litman group create PATH
litman group rename PATH NAME
litman group move PATH [--parent NEW_PARENT|--root]
litman group delete PATH
litman group add PATH PAPER_ID ...
litman group remove PATH PAPER_ID ...
litman root set DIR
litman open PAPER_ID
litman backup DESTINATION
litman manual
```

Repeated `--author` and `--keyword` values preserve order. `--clear title`, for example, creates an explicit blank manual value. Use the GUI's **Reset to PDF** action to remove an override and return to embedded metadata. Use `--interactive` to fill fields in a terminal.

Examples:

```console
litman --config D:/library/library.toml scan
litman --config D:/library/library.toml search "机器学习" --min-importance 3 --format json
litman --config D:/library/library.toml edit 1b3a --title "Correct title" --author "Li Wei" --author "Ada Smith"
litman --config D:/library/library.toml group create "Thesis/Methods"
litman --config D:/library/library.toml group add "Thesis/Methods" 1b3a 57c2
litman --config D:/library/library.toml rate 1b3a 5
```

Exit status is zero on success, 2 for command-line syntax errors, 3 when an ID is missing or ambiguous, and 4 for other library or I/O failures.
