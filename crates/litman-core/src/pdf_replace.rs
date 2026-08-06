use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ureq::ResponseExt;
use uuid::Uuid;

use crate::db::{Library, ScannedData};
use crate::metadata::extract_pdf_metadata;
use crate::model::{EmbeddedMetadata, FileStatus, Paper};
use crate::scan::hash_pdf;
use crate::scixplorer::{publisher_pdf_url, validate_bibcode};
use crate::{LitmanError, Result};

pub const BACKUP_DIRECTORY_NAME: &str = "LitMan-backups";
const BACKUP_MARKER_NAME: &str = ".litman-managed-backups-v1";
const MANIFEST_PREFIX: &str = ".litman-pdf-replacement-";
const MAX_PUBLISHER_PDF_SIZE: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfBackupMove {
    pub source_path: PathBuf,
    pub backup_path: PathBuf,
    pub expected_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfReplacementPlan {
    pub paper_id: String,
    pub bibcode: String,
    pub selected_source_path: PathBuf,
    pub displaced_target_path: Option<PathBuf>,
    pub active_path: PathBuf,
    pub backup_directory: PathBuf,
    pub backup_moves: Vec<PdfBackupMove>,
    pub gateway_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfReplacementResult {
    pub paper_id: String,
    pub bibcode: String,
    pub source_paths: Vec<PathBuf>,
    pub active_path: PathBuf,
    pub backup_paths: Vec<PathBuf>,
    pub gateway_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingManifest {
    version: u32,
    paper_id: String,
    old_relative_path: String,
    old_database_hash: String,
    active_relative_path: String,
    staged_relative_path: String,
    new_hash: String,
    moves: Vec<PendingMove>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingMove {
    source_relative_path: String,
    backup_relative_path: String,
    expected_hash: String,
}

struct StagedPdf {
    path: PathBuf,
    source_path: Option<PathBuf>,
    hash: String,
    embedded: EmbeddedMetadata,
}

struct TemporaryFileGuard(Option<PathBuf>);

impl TemporaryFileGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TemporaryFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for StagedPdf {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Library {
    pub fn pdf_replacement_plan(&self, paper_id: &str) -> Result<PdfReplacementPlan> {
        build_plan(self, paper_id)
    }

    pub fn replace_pdf_from_scixplorer(&mut self, paper_id: &str) -> Result<PdfReplacementResult> {
        let plan = self.pdf_replacement_plan(paper_id)?;
        self.replace_pdf_from_scixplorer_with_plan(&plan)
    }

    pub fn replace_pdf_from_file(
        &mut self,
        paper_id: &str,
        source_path: impl AsRef<Path>,
    ) -> Result<PdfReplacementResult> {
        let plan = self.pdf_replacement_plan(paper_id)?;
        self.replace_pdf_from_file_with_plan(&plan, source_path)
    }

    /// Execute the exact plan that was shown to a user. Any intervening path,
    /// owner, hash, or backup-name change aborts the operation.
    pub fn replace_pdf_from_scixplorer_with_plan(
        &mut self,
        plan: &PdfReplacementPlan,
    ) -> Result<PdfReplacementResult> {
        self.recover_pdf_replacements()?;
        require_unchanged_plan(self, plan)?;
        let staged = download_publisher_pdf(plan)?;
        self.commit_pdf_replacement(plan, staged)
    }

    /// Install a user-selected publisher PDF without moving or deleting the
    /// selected source download.
    pub fn replace_pdf_from_file_with_plan(
        &mut self,
        plan: &PdfReplacementPlan,
        source_path: impl AsRef<Path>,
    ) -> Result<PdfReplacementResult> {
        self.recover_pdf_replacements()?;
        require_unchanged_plan(self, plan)?;
        let staged = stage_local_pdf(plan, source_path.as_ref())?;
        self.commit_pdf_replacement(plan, staged)
    }

    pub fn recover_pdf_replacements(&mut self) -> Result<()> {
        let root_path = self.root_path();
        let Ok(root) = root_path.canonicalize() else {
            return Ok(());
        };
        let backup = root.join(BACKUP_DIRECTORY_NAME);
        if !backup.join(BACKUP_MARKER_NAME).is_file() {
            return Ok(());
        }
        for entry in fs::read_dir(&backup)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with(MANIFEST_PREFIX) && name.ends_with(".json") {
                let manifest: PendingManifest = serde_json::from_slice(&fs::read(&path)?)?;
                recover_manifest(self, &root, &path, &manifest)?;
            }
        }
        Ok(())
    }

    fn commit_pdf_replacement(
        &mut self,
        plan: &PdfReplacementPlan,
        staged: StagedPdf,
    ) -> Result<PdfReplacementResult> {
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let operation = (|| -> Result<(PendingManifest, PathBuf)> {
            require_unchanged_plan(self, plan)?;
            ensure_managed_backup_directory(&plan.backup_directory)?;

            let root = self.root_path().canonicalize()?;
            let paper = self.get_paper(&plan.paper_id)?;
            let manifest = manifest_from_plan(&root, plan, &paper, &staged)?;
            let manifest_path = plan
                .backup_directory
                .join(format!("{MANIFEST_PREFIX}{}.json", Uuid::new_v4()));
            write_manifest(&manifest_path, &manifest)?;

            for movement in &plan.backup_moves {
                move_without_overwrite(&movement.source_path, &movement.backup_path)?;
            }
            move_without_overwrite(&staged.path, &plan.active_path)?;

            let metadata = fs::metadata(&plan.active_path)?;
            let relative_path = relative_database_path(&root, &plan.active_path)?;
            let duplicate = self.present_id_by_hash(&staged.hash, Some(&plan.paper_id))?;
            self.update_scanned(
                &plan.paper_id,
                ScannedData {
                    relative_path: &relative_path,
                    file_size: metadata.len(),
                    modified_unix_ms: modified_unix_ms(&metadata),
                    content_hash: &staged.hash,
                    embedded: Some(&staged.embedded),
                    scan_error: None,
                    duplicate_of: duplicate.as_deref(),
                },
            )?;
            Ok((manifest, manifest_path))
        })();

        match operation {
            Ok((manifest, manifest_path)) => {
                if let Err(error) = self.connection.execute_batch("COMMIT") {
                    let _ = self.connection.execute_batch("ROLLBACK");
                    recover_manifest(
                        self,
                        &self.root_path().canonicalize()?,
                        &manifest_path,
                        &manifest,
                    )?;
                    return Err(error.into());
                }
                let _ = fs::remove_file(&manifest_path);
                Ok(PdfReplacementResult {
                    paper_id: plan.paper_id.clone(),
                    bibcode: plan.bibcode.clone(),
                    source_paths: staged
                        .source_path
                        .iter()
                        .cloned()
                        .chain(
                            plan.backup_moves
                                .iter()
                                .map(|movement| movement.source_path.clone()),
                        )
                        .collect(),
                    active_path: plan.active_path.clone(),
                    backup_paths: plan
                        .backup_moves
                        .iter()
                        .map(|movement| movement.backup_path.clone())
                        .collect(),
                    gateway_url: plan.gateway_url.clone(),
                })
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                let root = self.root_path().canonicalize()?;
                let manifests = pending_manifests(&plan.backup_directory)?;
                for (path, manifest) in manifests {
                    recover_manifest(self, &root, &path, &manifest)?;
                }
                Err(error)
            }
        }
    }
}

fn build_plan(library: &Library, paper_id: &str) -> Result<PdfReplacementPlan> {
    let paper = library.get_paper(paper_id)?;
    if paper.file_status != FileStatus::Present {
        return Err(replacement_error("the selected paper is not present"));
    }
    let bibcode = paper
        .bibcode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| replacement_error("the selected paper has no stored ADS bibcode"))?
        .to_owned();
    validate_bibcode(&bibcode)?;
    validate_portable_filename(&bibcode)?;

    let root_path = library.root_path();
    let root = root_path
        .canonicalize()
        .map_err(|_| LitmanError::RootUnavailable(root_path))?;
    let selected_source_path = resolve_record_path(&root, &paper.relative_path)?;
    if !selected_source_path.is_file() {
        return Err(replacement_error("the selected PDF is not a regular file"));
    }
    let parent = selected_source_path
        .parent()
        .ok_or_else(|| replacement_error("the selected PDF has no parent directory"))?;
    let active_path = parent.join(format!("{bibcode}.pdf"));
    ensure_lexical_child(&root, &active_path)?;

    let backup_directory = inspect_backup_directory(&root)?;
    let mut reserved = HashSet::new();
    let selected_backup = next_backup_path(&backup_directory, &bibcode, &mut reserved);
    let mut backup_moves = vec![PdfBackupMove {
        source_path: selected_source_path.clone(),
        backup_path: selected_backup,
        expected_hash: hash_file(&selected_source_path)?,
    }];

    let mut displaced_target_path = None;
    if active_path != selected_source_path && fs::symlink_metadata(&active_path).is_ok() {
        let metadata = fs::symlink_metadata(&active_path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(replacement_error(format!(
                "the active destination is not a regular file: {}",
                active_path.display()
            )));
        }
        let canonical_target = active_path.canonicalize()?;
        if !canonical_target.starts_with(&root) {
            return Err(replacement_error(
                "the active destination escapes the library root",
            ));
        }
        let relative_target = relative_database_path(&root, &canonical_target)?;
        if let Some(owner) = library.paper_by_path(&relative_target)?
            && owner.id != paper.id
        {
            return Err(LitmanError::PdfTargetOwnedByAnotherRecord {
                path: active_path,
                paper_id: owner.id,
            });
        }
        displaced_target_path = Some(active_path.clone());
        backup_moves.push(PdfBackupMove {
            source_path: active_path.clone(),
            backup_path: next_backup_path(&backup_directory, &bibcode, &mut reserved),
            expected_hash: hash_file(&active_path)?,
        });
    }

    Ok(PdfReplacementPlan {
        paper_id: paper.id,
        bibcode: bibcode.clone(),
        selected_source_path,
        displaced_target_path,
        active_path,
        backup_directory,
        backup_moves,
        gateway_url: publisher_pdf_url(&bibcode)?,
    })
}

fn require_unchanged_plan(library: &Library, confirmed: &PdfReplacementPlan) -> Result<()> {
    let current = build_plan(library, &confirmed.paper_id)?;
    if current != *confirmed {
        return Err(replacement_error(
            "files or ownership changed after confirmation; review the replacement again",
        ));
    }
    Ok(())
}

fn download_publisher_pdf(plan: &PdfReplacementPlan) -> Result<StagedPdf> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(8)
            .timeout_global(Some(Duration::from_secs(120)))
            .build(),
    );
    let response = agent.get(&plan.gateway_url).call().map_err(|error| {
        if matches!(error, ureq::Error::StatusCode(401 | 403)) {
            LitmanError::PublisherPdfBrowserRequired {
                gateway_url: plan.gateway_url.clone(),
            }
        } else {
            LitmanError::Scixplorer(format!("publisher PDF request failed: {error}"))
        }
    })?;
    if response.get_uri().scheme_str() != Some("https") {
        return Err(replacement_error("publisher redirected to a non-HTTPS URL"));
    }
    let html = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("text/html") || value.contains("application/xhtml")
        });
    if html {
        return Err(LitmanError::PublisherPdfBrowserRequired {
            gateway_url: plan.gateway_url.clone(),
        });
    }
    if response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > MAX_PUBLISHER_PDF_SIZE)
    {
        return Err(replacement_error("publisher PDF exceeds the 256 MiB limit"));
    }
    let (_, body) = response.into_parts();
    stage_reader(plan, body.into_reader(), true, None)
}

fn stage_local_pdf(plan: &PdfReplacementPlan, source_path: &Path) -> Result<StagedPdf> {
    let source = source_path.canonicalize()?;
    if !source.is_file() {
        return Err(replacement_error(
            "the selected publisher PDF is not a regular file",
        ));
    }
    let file = File::open(&source)?;
    stage_reader(plan, file, false, Some(source))
}

fn stage_reader(
    plan: &PdfReplacementPlan,
    mut reader: impl Read,
    non_pdf_is_browser_fallback: bool,
    source_path: Option<PathBuf>,
) -> Result<StagedPdf> {
    let parent = plan
        .active_path
        .parent()
        .ok_or_else(|| replacement_error("the active destination has no parent"))?;
    let path = parent.join(format!(".litman-pdf-replacement-{}.tmp", Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut temporary_guard = TemporaryFileGuard(Some(path.clone()));
    let copied = copy_bounded(&mut reader, &mut output, MAX_PUBLISHER_PDF_SIZE)?;
    output.sync_all()?;
    drop(output);
    if copied < 5 {
        return if non_pdf_is_browser_fallback {
            Err(LitmanError::PublisherPdfBrowserRequired {
                gateway_url: plan.gateway_url.clone(),
            })
        } else {
            Err(replacement_error("the selected file is not a PDF"))
        };
    }
    let hash = match hash_pdf(&path) {
        Ok(hash) => hash,
        Err(_) if non_pdf_is_browser_fallback => {
            return Err(LitmanError::PublisherPdfBrowserRequired {
                gateway_url: plan.gateway_url.clone(),
            });
        }
        Err(error) => {
            return Err(error);
        }
    };
    let embedded = match catch_unwind(AssertUnwindSafe(|| extract_pdf_metadata(&path))) {
        Ok(Ok(metadata)) => metadata,
        Ok(Err(error)) => {
            return Err(replacement_error(format!(
                "the staged PDF could not be parsed: {error}"
            )));
        }
        Err(_) => {
            return Err(replacement_error("the staged PDF parser panicked"));
        }
    };
    temporary_guard.disarm();
    Ok(StagedPdf {
        path,
        source_path,
        hash,
        embedded,
    })
}

fn copy_bounded(reader: &mut impl Read, writer: &mut impl Write, limit: u64) -> Result<u64> {
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > limit {
            return Err(replacement_error("publisher PDF exceeds the 256 MiB limit"));
        }
        writer.write_all(&buffer[..count])?;
    }
    Ok(total)
}

fn manifest_from_plan(
    root: &Path,
    plan: &PdfReplacementPlan,
    paper: &Paper,
    staged: &StagedPdf,
) -> Result<PendingManifest> {
    Ok(PendingManifest {
        version: 1,
        paper_id: plan.paper_id.clone(),
        old_relative_path: paper.relative_path.clone(),
        old_database_hash: paper.content_hash.clone(),
        active_relative_path: relative_database_path(root, &plan.active_path)?,
        staged_relative_path: relative_database_path(root, &staged.path)?,
        new_hash: staged.hash.clone(),
        moves: plan
            .backup_moves
            .iter()
            .map(|movement| {
                Ok(PendingMove {
                    source_relative_path: relative_database_path(root, &movement.source_path)?,
                    backup_relative_path: relative_database_path(root, &movement.backup_path)?,
                    expected_hash: movement.expected_hash.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    })
}

fn write_manifest(path: &Path, manifest: &PendingManifest) -> Result<()> {
    let temporary_path =
        path.with_file_name(format!(".litman-manifest-write-{}.tmp", Uuid::new_v4()));
    let mut temporary_guard = TemporaryFileGuard(Some(temporary_path.clone()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)?;
    serde_json::to_writer_pretty(&mut file, manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    move_without_overwrite(&temporary_path, path)?;
    temporary_guard.disarm();
    Ok(())
}

fn pending_manifests(directory: &Path) -> Result<Vec<(PathBuf, PendingManifest)>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut manifests = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(MANIFEST_PREFIX) && name.ends_with(".json") {
            manifests.push((path.clone(), serde_json::from_slice(&fs::read(path)?)?));
        }
    }
    Ok(manifests)
}

fn recover_manifest(
    library: &mut Library,
    root: &Path,
    manifest_path: &Path,
    manifest: &PendingManifest,
) -> Result<()> {
    if manifest.version != 1 {
        return Err(replacement_error(format!(
            "unsupported pending-operation manifest: {}",
            manifest_path.display()
        )));
    }
    let active = resolve_manifest_path(root, &manifest.active_relative_path)?;
    let staged = resolve_manifest_path(root, &manifest.staged_relative_path)?;
    let paper = library.get_paper(&manifest.paper_id)?;
    let active_hash = existing_hash(&active)?;
    if paper.relative_path == manifest.active_relative_path
        && paper.content_hash == manifest.new_hash
        && active_hash.as_deref() == Some(&manifest.new_hash)
    {
        if staged.is_file() && existing_hash(&staged)?.as_deref() == Some(&manifest.new_hash) {
            fs::remove_file(staged)?;
        }
        fs::remove_file(manifest_path)?;
        return Ok(());
    }
    if paper.relative_path != manifest.old_relative_path
        || paper.content_hash != manifest.old_database_hash
    {
        return Err(replacement_error(format!(
            "pending replacement does not match the database; preserved all files: {}",
            manifest_path.display()
        )));
    }

    // Validate the complete state before changing any file. Each original must
    // be at exactly its source or backup location, never both.
    for movement in &manifest.moves {
        let source = resolve_manifest_path(root, &movement.source_relative_path)?;
        let backup = resolve_manifest_path(root, &movement.backup_relative_path)?;
        let source_hash = existing_hash(&source)?;
        let backup_hash = existing_hash(&backup)?;
        let active_contains_new_pdf =
            source == active && source_hash.as_deref() == Some(&manifest.new_hash);
        let effective_source_hash = if active_contains_new_pdf {
            None
        } else {
            source_hash.as_deref()
        };
        let source_ok = effective_source_hash == Some(&movement.expected_hash);
        let backup_ok = backup_hash.as_deref() == Some(&movement.expected_hash);
        if (!source_ok && !backup_ok)
            || effective_source_hash.is_some_and(|hash| hash != movement.expected_hash)
            || backup_hash.is_some_and(|hash| hash != movement.expected_hash)
        {
            return Err(replacement_error(format!(
                "pending replacement files do not match their manifest; preserved all files: {}",
                manifest_path.display()
            )));
        }
    }
    if let Some(hash) = active_hash.as_deref()
        && hash != manifest.new_hash
        && !manifest.moves.iter().any(|movement| {
            movement.source_relative_path == manifest.active_relative_path
                && movement.expected_hash == hash
        })
    {
        return Err(replacement_error(format!(
            "pending replacement active PDF has an unexpected hash; preserved all files: {}",
            manifest_path.display()
        )));
    }

    if existing_hash(&active)?.as_deref() == Some(&manifest.new_hash) {
        fs::remove_file(&active)?;
    }
    for movement in manifest.moves.iter().rev() {
        let source = resolve_manifest_path(root, &movement.source_relative_path)?;
        let backup = resolve_manifest_path(root, &movement.backup_relative_path)?;
        if source.exists() && backup.exists() {
            fs::remove_file(backup)?;
        } else if !source.exists() {
            move_without_overwrite(&backup, &source)?;
        }
    }
    if staged.is_file() && existing_hash(&staged)?.as_deref() == Some(&manifest.new_hash) {
        fs::remove_file(staged)?;
    }
    fs::remove_file(manifest_path)?;
    Ok(())
}

fn inspect_backup_directory(root: &Path) -> Result<PathBuf> {
    let path = root.join(BACKUP_DIRECTORY_NAME);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(replacement_error(format!(
                "reserved backup path is not a directory: {}",
                path.display()
            )))
        }
        Ok(_) if path.join(BACKUP_MARKER_NAME).is_file() => Ok(path),
        Ok(_) if fs::read_dir(&path)?.next().is_none() => Ok(path),
        Ok(_) => Err(replacement_error(format!(
            "refusing to use nonempty unmarked reserved directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(error) => Err(error.into()),
    }
}

fn ensure_managed_backup_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir(path)?;
    }
    if path.join(BACKUP_MARKER_NAME).is_file() {
        return Ok(());
    }
    if fs::read_dir(path)?.next().is_some() {
        return Err(replacement_error(format!(
            "refusing to adopt nonempty unmarked reserved directory: {}",
            path.display()
        )));
    }
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.join(BACKUP_MARKER_NAME))?;
    marker.write_all(b"LitMan managed PDF replacement backups, version 1\n")?;
    marker.sync_all()?;
    Ok(())
}

pub(crate) fn is_managed_backup_directory(root: &Path, path: &Path) -> bool {
    path == root.join(BACKUP_DIRECTORY_NAME) && path.join(BACKUP_MARKER_NAME).is_file()
}

fn next_backup_path(directory: &Path, bibcode: &str, reserved: &mut HashSet<PathBuf>) -> PathBuf {
    for number in 1_u64.. {
        let suffix = if number == 1 {
            String::new()
        } else {
            format!("_{}", number)
        };
        let candidate = directory.join(format!("{bibcode}_bk{suffix}.pdf"));
        if !candidate.exists() && reserved.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn validate_portable_filename(value: &str) -> Result<()> {
    let upper = value.to_ascii_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if value.is_empty()
        || value.ends_with(['.', ' '])
        || value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*'
                )
        })
        || reserved.contains(&upper.as_str())
    {
        return Err(replacement_error("ADS bibcode is not a portable filename"));
    }
    Ok(())
}

fn resolve_record_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = resolve_manifest_path(root, relative)?;
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() {
        return Err(replacement_error(
            "selected PDF must not be a symbolic link",
        ));
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(replacement_error("selected PDF escapes the library root"));
    }
    Ok(canonical)
}

fn resolve_manifest_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    if relative.is_empty() {
        return Err(replacement_error("empty path in PDF replacement manifest"));
    }
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(value) => path.push(value),
            _ => return Err(replacement_error("unsafe path in PDF replacement manifest")),
        }
    }
    ensure_lexical_child(root, &path)?;
    Ok(path)
}

fn ensure_lexical_child(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root) {
        return Err(replacement_error(
            "PDF replacement path escapes the library root",
        ));
    }
    Ok(())
}

fn relative_database_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| replacement_error("cannot derive a library-relative PDF path"))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or_else(|| replacement_error("PDF path is not portable Unicode"))?
                    .to_owned(),
            ),
            _ => return Err(replacement_error("PDF path contains unsafe components")),
        }
    }
    Ok(parts.join("/"))
}

fn hash_file(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.write_all(&buffer[..count])?;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn existing_hash(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(replacement_error(format!(
                "unexpected non-file replacement path: {}",
                path.display()
            )))
        }
        Ok(_) => hash_file(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn modified_unix_ms(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn move_without_overwrite(source: &Path, destination: &Path) -> Result<()> {
    // Hard-link creation is atomic and fails if the destination appeared after
    // confirmation. Removing the old name then gives rename semantics without
    // Unix `rename` silently replacing a newly-created collision.
    fs::hard_link(source, destination)?;
    if let Err(error) = fs::remove_file(source) {
        let _ = fs::remove_file(destination);
        return Err(error.into());
    }
    Ok(())
}

fn replacement_error(message: impl Into<String>) -> LitmanError {
    LitmanError::PdfReplacement(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Document, Object, dictionary};
    use std::io::Cursor;
    use tempfile::TempDir;

    use crate::{Config, ListFilter, PaperUpdate, ScanOptions};

    const BIBCODE: &str = "2020ApJ...900....1A";

    fn write_pdf(path: &Path, title: &str) {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        let info_id = document.add_object(dictionary! {
            "Title" => Object::string_literal(title),
            "Author" => Object::string_literal("Embedded Author"),
        });
        document.trailer.set("Root", catalog_id);
        document.trailer.set("Info", info_id);
        document.compress();
        document.save(path).unwrap();
    }

    fn setup(relative: &str) -> (TempDir, Library, PathBuf, String) {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        let old = root.join(relative);
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        write_pdf(&old, "Preprint");
        let config_path = temporary.path().join("library.toml");
        let mut library = Library::init(&config_path, Config::new(root.clone())).unwrap();
        library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        let id = library
            .list_papers(&ListFilter::default())
            .unwrap()
            .remove(0)
            .id;
        library
            .store_bibtex(
                &id,
                &format!(
                    "@article{{{BIBCODE}, title={{Published title}}, author={{Bib, Alice and Tex, Bob}}, journal={{ApJ}}, year={{2020}}}}"
                ),
            )
            .unwrap();
        (temporary, library, root, id)
    }

    #[test]
    fn replaces_nested_unicode_pdf_and_preserves_record_organization() {
        let (temporary, mut library, root, id) = setup("嵌套/preprint.pdf");
        library
            .update_paper(
                &id,
                PaperUpdate {
                    title: Some(Some("Manual title".into())),
                    notes: Some(Some("Keep this note".into())),
                    ..Default::default()
                },
            )
            .unwrap();
        library.set_importance(&id, Some(5)).unwrap();
        library.create_group("项目/重要").unwrap();
        library
            .add_to_group("项目/重要", std::slice::from_ref(&id))
            .unwrap();
        let publisher = temporary.path().join("download.pdf");
        write_pdf(&publisher, "Publisher PDF");

        let result = library.replace_pdf_from_file(&id, &publisher).unwrap();
        assert_eq!(
            result.active_path,
            root.join("嵌套")
                .join(format!("{BIBCODE}.pdf"))
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            result.backup_paths,
            vec![
                root.join(BACKUP_DIRECTORY_NAME)
                    .join(format!("{BIBCODE}_bk.pdf"))
                    .canonicalize()
                    .unwrap()
            ]
        );
        assert!(
            publisher.is_file(),
            "selected download must remain untouched"
        );
        assert!(result.active_path.is_file());
        assert!(result.backup_paths[0].is_file());

        let paper = library.get_paper(&id).unwrap();
        assert_eq!(paper.relative_path, format!("嵌套/{BIBCODE}.pdf"));
        assert_eq!(paper.title.as_deref(), Some("Manual title"));
        assert_eq!(paper.notes.as_deref(), Some("Keep this note"));
        assert_eq!(paper.importance, Some(5));
        assert_eq!(paper.bibcode.as_deref(), Some(BIBCODE));
        assert!(paper.bibtex.is_some());
        assert_eq!(library.groups_for_paper(&id).unwrap().len(), 1);

        library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        assert_eq!(
            library.list_papers(&ListFilter::default()).unwrap().len(),
            1
        );
    }

    #[test]
    fn active_name_is_never_numbered_and_two_displaced_files_are_backed_up() {
        let (temporary, mut library, root, id) = setup("preprint.pdf");
        let occupied = root.join(format!("{BIBCODE}.pdf"));
        write_pdf(&occupied, "Untracked occupied target");
        let publisher = temporary.path().join("publisher.pdf");
        write_pdf(&publisher, "Publisher");

        let result = library.replace_pdf_from_file(&id, publisher).unwrap();
        assert_eq!(result.active_path, occupied.canonicalize().unwrap());
        assert_eq!(result.backup_paths.len(), 2);
        assert_eq!(
            result.backup_paths[0]
                .file_name()
                .unwrap()
                .to_string_lossy(),
            format!("{BIBCODE}_bk.pdf")
        );
        assert_eq!(
            result.backup_paths[1]
                .file_name()
                .unwrap()
                .to_string_lossy(),
            format!("{BIBCODE}_bk_2.pdf")
        );
        assert!(!root.join(format!("{BIBCODE}_2.pdf")).exists());
    }

    #[test]
    fn replaces_when_selected_pdf_already_has_the_active_name() {
        let (temporary, mut library, root, id) = setup(&format!("{BIBCODE}.pdf"));
        let publisher = temporary.path().join("publisher.pdf");
        write_pdf(&publisher, "Publisher");
        let result = library.replace_pdf_from_file(&id, publisher).unwrap();
        assert_eq!(result.source_paths.len(), 2);
        assert!(
            result
                .source_paths
                .iter()
                .any(|path| path == &result.active_path)
        );
        assert_eq!(
            result.active_path,
            root.join(format!("{BIBCODE}.pdf")).canonicalize().unwrap()
        );
        assert!(
            root.join(BACKUP_DIRECTORY_NAME)
                .join(format!("{BIBCODE}_bk.pdf"))
                .is_file()
        );
    }

    #[test]
    fn refuses_an_active_path_owned_by_another_record_without_changes() {
        let (_temporary, mut library, root, id) = setup("preprint.pdf");
        let occupied = root.join(format!("{BIBCODE}.pdf"));
        write_pdf(&occupied, "Tracked target");
        library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        let before = hash_file(&root.join("preprint.pdf")).unwrap();
        let occupied_before = hash_file(&occupied).unwrap();

        let error = library.pdf_replacement_plan(&id).unwrap_err();
        assert!(error.to_string().contains("another LitMan record"));
        assert_eq!(hash_file(&root.join("preprint.pdf")).unwrap(), before);
        assert_eq!(hash_file(&occupied).unwrap(), occupied_before);
        assert!(!root.join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn nonempty_unmarked_backup_directory_is_refused_and_still_scanned() {
        let (_temporary, mut library, root, id) = setup("preprint.pdf");
        let backup = root.join(BACKUP_DIRECTORY_NAME);
        fs::create_dir(&backup).unwrap();
        write_pdf(&backup.join("existing.pdf"), "Existing paper");
        let error = library.pdf_replacement_plan(&id).unwrap_err();
        assert!(error.to_string().contains("nonempty unmarked"));

        library.scan(ScanOptions::default(), None, |_| {}).unwrap();
        assert_eq!(
            library.list_papers(&ListFilter::default()).unwrap().len(),
            2
        );
    }

    #[test]
    fn empty_unmarked_backup_directory_is_adopted_and_numbering_skips_existing_backup() {
        let (temporary, mut library, root, id) = setup("preprint.pdf");
        let backup = root.join(BACKUP_DIRECTORY_NAME);
        fs::create_dir(&backup).unwrap();
        let publisher = temporary.path().join("publisher.pdf");
        write_pdf(&publisher, "Publisher one");
        library.replace_pdf_from_file(&id, &publisher).unwrap();
        assert!(backup.join(BACKUP_MARKER_NAME).is_file());

        write_pdf(&publisher, "Publisher two");
        let result = library.replace_pdf_from_file(&id, publisher).unwrap();
        assert_eq!(
            result.backup_paths[0]
                .file_name()
                .unwrap()
                .to_string_lossy(),
            format!("{BIBCODE}_bk_2.pdf")
        );
        assert_eq!(
            result.active_path,
            root.join(format!("{BIBCODE}.pdf")).canonicalize().unwrap()
        );
    }

    #[test]
    fn rejects_non_pdf_local_source_without_moving_anything() {
        let (temporary, mut library, root, id) = setup("preprint.pdf");
        let invalid = temporary.path().join("not.pdf");
        fs::write(&invalid, b"not a PDF").unwrap();
        let before = hash_file(&root.join("preprint.pdf")).unwrap();
        assert!(library.replace_pdf_from_file(&id, invalid).is_err());
        assert_eq!(hash_file(&root.join("preprint.pdf")).unwrap(), before);
        assert!(!root.join(format!("{BIBCODE}.pdf")).exists());
    }

    #[test]
    fn portable_names_and_bounded_streams_are_enforced() {
        assert!(validate_portable_filename(BIBCODE).is_ok());
        assert!(validate_portable_filename("CON").is_err());
        assert!(validate_portable_filename("bad:name").is_err());
        let mut output = Vec::new();
        assert!(copy_bounded(&mut Cursor::new(vec![0_u8; 11]), &mut output, 10).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_active_target_is_refused() {
        use std::os::unix::fs::symlink;

        let (_temporary, library, root, id) = setup("preprint.pdf");
        symlink("preprint.pdf", root.join(format!("{BIBCODE}.pdf"))).unwrap();
        assert!(library.pdf_replacement_plan(&id).is_err());
        assert!(!root.join(BACKUP_DIRECTORY_NAME).exists());
    }

    #[test]
    fn startup_rolls_back_interrupted_two_source_replacement() {
        let (temporary, library, root, id) = setup("preprint.pdf");
        let active = root.join(format!("{BIBCODE}.pdf"));
        write_pdf(&active, "Untracked target");
        let selected_hash = hash_file(&root.join("preprint.pdf")).unwrap();
        let occupied_hash = hash_file(&active).unwrap();
        let publisher = temporary.path().join("publisher.pdf");
        write_pdf(&publisher, "Publisher");

        let plan = library.pdf_replacement_plan(&id).unwrap();
        let staged = stage_local_pdf(&plan, &publisher).unwrap();
        ensure_managed_backup_directory(&plan.backup_directory).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        let paper = library.get_paper(&id).unwrap();
        let manifest = manifest_from_plan(&canonical_root, &plan, &paper, &staged).unwrap();
        let manifest_path = plan
            .backup_directory
            .join(format!("{MANIFEST_PREFIX}interrupted.json"));
        write_manifest(&manifest_path, &manifest).unwrap();
        for movement in &plan.backup_moves {
            fs::rename(&movement.source_path, &movement.backup_path).unwrap();
        }
        fs::rename(&staged.path, &plan.active_path).unwrap();
        let config_path = library.config_path.clone();
        drop(library);

        let recovered = Library::open(config_path).unwrap();
        assert_eq!(
            hash_file(&root.join("preprint.pdf")).unwrap(),
            selected_hash
        );
        assert_eq!(hash_file(&active).unwrap(), occupied_hash);
        assert!(!manifest_path.exists());
        assert_eq!(
            recovered.get_paper(&id).unwrap().relative_path,
            "preprint.pdf"
        );
    }
}
