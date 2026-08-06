use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use walkdir::WalkDir;

use crate::db::{Library, ScannedData};
use crate::metadata::extract_pdf_metadata;
use crate::model::{ScanEvent, ScanOptions, ScanReport};
use crate::pdf_replace::is_managed_backup_directory;
use crate::{LitmanError, Result};

#[derive(Debug)]
struct DiscoveredFile {
    absolute_path: PathBuf,
    relative_path: String,
    file_size: u64,
    modified_unix_ms: i64,
}

impl Library {
    pub fn scan<F>(
        &mut self,
        options: ScanOptions,
        cancellation: Option<&AtomicBool>,
        mut emit: F,
    ) -> Result<ScanReport>
    where
        F: FnMut(ScanEvent),
    {
        self.recover_pdf_replacements()?;
        let root_path = self.root_path();
        let root = root_path
            .canonicalize()
            .map_err(|_| LitmanError::RootUnavailable(root_path))?;
        if !root.is_dir() {
            return Err(LitmanError::RootUnavailable(root));
        }

        let mut report = ScanReport::default();
        let mut files = Vec::new();
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !is_managed_backup_directory(&root, entry.path()))
        {
            if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                report.cancelled = true;
                emit(ScanEvent::Finished(report.clone()));
                return Ok(report);
            }
            match entry {
                Ok(entry) if entry.file_type().is_file() && is_pdf(entry.path()) => {
                    match discovered_file(&root, entry.path()) {
                        Ok(file) => files.push(file),
                        Err(error) => {
                            report.errors += 1;
                            emit(ScanEvent::Warning {
                                path: entry.path().display().to_string(),
                                message: error.to_string(),
                            });
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    report.errors += 1;
                    emit(ScanEvent::Warning {
                        path: error
                            .path()
                            .map(|path| path.display().to_string())
                            .unwrap_or_default(),
                        message: error.to_string(),
                    });
                }
            }
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        report.discovered = files.len();
        emit(ScanEvent::Started { total: files.len() });

        let discovered = files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect::<HashSet<_>>();
        report.missing = self.mark_missing_not_in(&discovered)?;

        for (index, file) in files.into_iter().enumerate() {
            if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                report.cancelled = true;
                break;
            }
            emit(ScanEvent::Processing {
                current: index + 1,
                path: file.relative_path.clone(),
            });

            let existing = self.paper_by_path(&file.relative_path)?;
            let unchanged = existing.as_ref().is_some_and(|paper| {
                paper.file_size == file.file_size
                    && paper.modified_unix_ms == file.modified_unix_ms
                    && !paper.content_hash.is_empty()
            });
            if unchanged && !options.refresh_metadata {
                let paper = existing.expect("checked above");
                if paper.file_status.as_str() == "missing" {
                    self.update_scanned(
                        &paper.id,
                        ScannedData {
                            relative_path: &file.relative_path,
                            file_size: file.file_size,
                            modified_unix_ms: file.modified_unix_ms,
                            content_hash: &paper.content_hash,
                            embedded: None,
                            scan_error: paper.scan_error.as_deref(),
                            duplicate_of: paper.duplicate_of.as_deref(),
                        },
                    )?;
                }
                report.unchanged += 1;
                continue;
            }

            let hash = match hash_pdf(&file.absolute_path) {
                Ok(hash) => hash,
                Err(error) => {
                    report.errors += 1;
                    emit(ScanEvent::Warning {
                        path: file.relative_path.clone(),
                        message: error.to_string(),
                    });
                    if let Some(paper) = existing {
                        self.update_scanned(
                            &paper.id,
                            ScannedData {
                                relative_path: &file.relative_path,
                                file_size: file.file_size,
                                modified_unix_ms: file.modified_unix_ms,
                                content_hash: &paper.content_hash,
                                embedded: None,
                                scan_error: Some(&error.to_string()),
                                duplicate_of: paper.duplicate_of.as_deref(),
                            },
                        )?;
                        report.updated += 1;
                    } else {
                        let id = self.insert_scanned(ScannedData {
                            relative_path: &file.relative_path,
                            file_size: file.file_size,
                            modified_unix_ms: file.modified_unix_ms,
                            content_hash: "",
                            embedded: None,
                            scan_error: Some(&error.to_string()),
                            duplicate_of: None,
                        })?;
                        report.added += 1;
                        report.added_ids.push(id);
                    }
                    continue;
                }
            };

            let extracted = catch_unwind(AssertUnwindSafe(|| {
                extract_pdf_metadata(&file.absolute_path)
            }));
            let (embedded, scan_error) = match extracted {
                Ok(Ok(metadata)) => (Some(metadata), None),
                Ok(Err(error)) => (None, Some(error.to_string())),
                Err(_) => (None, Some("PDF metadata parser panicked".to_owned())),
            };
            if let Some(message) = scan_error.as_deref() {
                report.errors += 1;
                emit(ScanEvent::Warning {
                    path: file.relative_path.clone(),
                    message: message.to_owned(),
                });
            }

            if let Some(paper) = existing {
                let duplicate = self.present_id_by_hash(&hash, Some(&paper.id))?;
                self.update_scanned(
                    &paper.id,
                    ScannedData {
                        relative_path: &file.relative_path,
                        file_size: file.file_size,
                        modified_unix_ms: file.modified_unix_ms,
                        content_hash: &hash,
                        embedded: embedded.as_ref(),
                        scan_error: scan_error.as_deref(),
                        duplicate_of: duplicate.as_deref(),
                    },
                )?;
                report.updated += 1;
                continue;
            }

            let missing_matches = self.missing_ids_by_hash(&hash)?;
            if missing_matches.len() == 1 {
                let id = &missing_matches[0];
                let duplicate = self.present_id_by_hash(&hash, Some(id))?;
                self.update_scanned(
                    id,
                    ScannedData {
                        relative_path: &file.relative_path,
                        file_size: file.file_size,
                        modified_unix_ms: file.modified_unix_ms,
                        content_hash: &hash,
                        embedded: embedded.as_ref(),
                        scan_error: scan_error.as_deref(),
                        duplicate_of: duplicate.as_deref(),
                    },
                )?;
                report.moved += 1;
            } else {
                let duplicate = self.present_id_by_hash(&hash, None)?;
                let id = self.insert_scanned(ScannedData {
                    relative_path: &file.relative_path,
                    file_size: file.file_size,
                    modified_unix_ms: file.modified_unix_ms,
                    content_hash: &hash,
                    embedded: embedded.as_ref(),
                    scan_error: scan_error.as_deref(),
                    duplicate_of: duplicate.as_deref(),
                })?;
                report.added += 1;
                report.added_ids.push(id);
            }
        }

        emit(ScanEvent::Finished(report.clone()));
        Ok(report)
    }
}

fn discovered_file(root: &Path, path: &Path) -> Result<DiscoveredFile> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(LitmanError::InvalidConfig(format!(
            "PDF path escapes library root: {}",
            path.display()
        )));
    }
    let relative = canonical
        .strip_prefix(root)
        .map_err(|_| LitmanError::InvalidConfig("cannot derive relative PDF path".into()))?;
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => components.push(
                value
                    .to_str()
                    .ok_or_else(|| {
                        LitmanError::InvalidConfig(
                            "PDF path is not valid Unicode and cannot be portable".into(),
                        )
                    })?
                    .to_owned(),
            ),
            _ => {
                return Err(LitmanError::InvalidConfig(
                    "PDF path contains unsupported components".into(),
                ));
            }
        }
    }
    let metadata = fs::metadata(&canonical)?;
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default();
    Ok(DiscoveredFile {
        absolute_path: canonical,
        relative_path: components.join("/"),
        file_size: metadata.len(),
        modified_unix_ms,
    })
}

pub(crate) fn hash_pdf(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut prefix = [0_u8; 5];
    file.read_exact(&mut prefix)?;
    if &prefix != b"%PDF-" {
        return Err(LitmanError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file has a .pdf extension but no PDF header",
        )));
    }
    let mut hasher = blake3::Hasher::new();
    hasher.write_all(&prefix)?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.write_all(&buffer[..read])?;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, PaperUpdate};
    use tempfile::TempDir;

    #[test]
    fn invalid_pdf_is_recorded_without_stopping_scan() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("broken.pdf"), b"not a pdf").unwrap();
        let config_path = temporary.path().join("library.toml");
        let mut library = Library::init(&config_path, Config::new(root)).unwrap();
        let report = library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.errors, 1);
        let papers = library.list_papers(&Default::default()).unwrap();
        assert_eq!(papers[0].file_status.as_str(), "error");
    }

    #[test]
    fn moves_duplicates_missing_files_and_manual_overrides_are_reconciled() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let original = root.join("原始.pdf");
        let moved = root.join("nested").join("moved.pdf");
        let bytes = b"%PDF-1.7\nmalformed but hashable fixture";
        fs::write(&original, bytes).unwrap();
        let config_path = temporary.path().join("library.toml");
        let mut library = Library::init(&config_path, Config::new(root.clone())).unwrap();

        let initial = library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        let paper = library.list_papers(&Default::default()).unwrap().remove(0);
        assert_eq!(initial.added_ids, vec![paper.id.clone()]);
        library
            .update_paper(
                &paper.id,
                PaperUpdate {
                    title: Some(Some("手工题名".into())),
                    ..Default::default()
                },
            )
            .unwrap();

        fs::create_dir(root.join("nested")).unwrap();
        fs::rename(&original, &moved).unwrap();
        let report = library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        assert_eq!(report.moved, 1);
        assert!(report.added_ids.is_empty());
        let moved_paper = library.get_paper(&paper.id).unwrap();
        assert_eq!(moved_paper.relative_path, "nested/moved.pdf");
        assert_eq!(moved_paper.title.as_deref(), Some("手工题名"));
        assert_eq!(moved_paper.file_status.as_str(), "error");

        let copy = root.join("copy.pdf");
        fs::write(&copy, bytes).unwrap();
        library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        let papers = library.list_papers(&Default::default()).unwrap();
        assert_eq!(papers.len(), 2);
        assert!(papers.iter().any(|paper| paper.duplicate_of.is_some()));

        fs::remove_file(copy).unwrap();
        let report = library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        assert_eq!(report.missing, 1);
        assert_eq!(
            library
                .list_papers(&Default::default())
                .unwrap()
                .iter()
                .filter(|paper| paper.file_status.as_str() == "missing")
                .count(),
            1
        );

        let reset = library.reset_field(&paper.id, "title").unwrap();
        assert_eq!(reset.title, None);
        assert!(!reset.manual_overrides.contains("title"));
    }
}
