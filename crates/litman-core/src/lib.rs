mod config;
mod db;
mod error;
mod i18n;
mod metadata;
mod model;
mod pdf_replace;
mod remote_import;
mod scan;
mod scixplorer;

pub use config::{Config, Language, default_config_path};
pub use db::{Library, ListFilter};
pub use error::{LitmanError, Result};
pub use i18n::{Locale, tr};
pub use metadata::extract_pdf_metadata;
pub use model::{
    EmbeddedMetadata, FileStatus, Group, Paper, PaperUpdate, RemoteIdentifier,
    RemoteImportProvider, RemoteImportResult, RemotePdfSource, RemoteProvider, ScanEvent,
    ScanOptions, ScanReport, ScixplorerRecord, ScixplorerSearchField,
};
pub use pdf_replace::{PdfBackupMove, PdfReplacementPlan, PdfReplacementResult};
pub use remote_import::parse_remote_identifier;
pub use scixplorer::{ScixplorerClient, publisher_pdf_url, scixplorer_url};
