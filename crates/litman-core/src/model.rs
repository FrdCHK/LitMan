use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Present,
    Missing,
    Error,
}

impl FileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "missing" => Self::Missing,
            "error" => Self::Error,
            _ => Self::Present,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddedMetadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub abstract_text: Option<String>,
    pub publication_date: Option<String>,
    pub container_title: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub doi: Option<String>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub keywords: Vec<String>,
    pub page_count: Option<u32>,
    pub pdf_version: Option<String>,
    pub encrypted: bool,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<String>,
    pub modification_date: Option<String>,
    /// Original, un-reconciled PDF Info values for diagnostics and provenance.
    pub raw_info: BTreeMap<String, Vec<String>>,
    /// Original, un-reconciled XMP/DC/PRISM values for diagnostics and provenance.
    pub raw_xmp: BTreeMap<String, Vec<String>>,
    /// Effective embedded source for each bibliographic field (`xmp` or `pdf_info`).
    pub field_sources: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paper {
    pub id: String,
    pub relative_path: String,
    pub file_size: u64,
    pub modified_unix_ms: i64,
    pub content_hash: String,
    pub file_status: FileStatus,
    pub scan_error: Option<String>,
    pub duplicate_of: Option<String>,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub abstract_text: Option<String>,
    pub publication_date: Option<String>,
    pub container_title: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub doi: Option<String>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub keywords: Vec<String>,
    pub notes: Option<String>,
    /// Raw BibTeX returned by the ADS export API.
    #[serde(skip_serializing)]
    pub bibtex: Option<String>,
    /// Canonical ADS/SciXplorer bibcode, normally the BibTeX citation key.
    pub bibcode: Option<String>,
    /// Metadata fields most recently populated from `bibtex`.
    pub bibtex_fields: BTreeSet<String>,
    pub importance: Option<u8>,
    pub page_count: Option<u32>,
    pub pdf_version: Option<String>,
    pub encrypted: bool,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub embedded: EmbeddedMetadata,
    pub manual_overrides: BTreeSet<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Paper {
    pub fn display_title(&self) -> String {
        self.title.clone().unwrap_or_else(|| {
            PathBuf::from(&self.relative_path)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(&self.relative_path)
                .to_owned()
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PaperUpdate {
    pub title: Option<Option<String>>,
    pub authors: Option<Vec<String>>,
    pub abstract_text: Option<Option<String>>,
    pub publication_date: Option<Option<String>>,
    pub container_title: Option<Option<String>>,
    pub volume: Option<Option<String>>,
    pub issue: Option<Option<String>>,
    pub pages: Option<Option<String>>,
    pub doi: Option<Option<String>>,
    pub url: Option<Option<String>>,
    pub language: Option<Option<String>>,
    pub keywords: Option<Vec<String>>,
    pub notes: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScixplorerSearchField {
    Title,
    Doi,
    Bibcode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScixplorerRecord {
    pub bibcode: String,
    pub title: String,
    pub authors: Vec<String>,
    pub publication_date: Option<String>,
    pub doi: Option<String>,
    pub publication: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    pub refresh_metadata: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanEvent {
    Started { total: usize },
    Processing { current: usize, path: String },
    Warning { path: String, message: String },
    Finished(ScanReport),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub discovered: usize,
    pub added: usize,
    pub updated: usize,
    pub moved: usize,
    pub unchanged: usize,
    pub missing: usize,
    pub errors: usize,
    pub cancelled: bool,
}
