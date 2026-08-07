use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ureq::ResponseExt;
use uuid::Uuid;

use crate::db::{Library, ScannedData};
use crate::metadata::extract_pdf_metadata;
use crate::model::{
    EmbeddedMetadata, RemoteIdentifier, RemoteImportProvider, RemoteImportResult, RemotePdfSource,
    RemoteProvider,
};
use crate::scan::hash_pdf;
use crate::scixplorer::{
    ads_pdf_url, eprint_pdf_url, parse_bibtex, publisher_pdf_url, validate_bibcode,
};
use crate::{LitmanError, Result};

const ARXIV_API_URL: &str = "https://export.arxiv.org/api/query";
const MAX_METADATA_SIZE: u64 = 2 * 1024 * 1024;
const MAX_PDF_SIZE: u64 = 256 * 1024 * 1024;
const IMPORT_MANIFEST_PREFIX: &str = ".litman-remote-import-";

struct RemoteEndpoints {
    ads_api_base: String,
    arxiv_api_url: String,
    test_pdf_base: Option<String>,
    allow_http: bool,
}

impl RemoteEndpoints {
    fn production() -> Self {
        Self {
            ads_api_base: "https://api.adsabs.harvard.edu/v1".into(),
            arxiv_api_url: ARXIV_API_URL.into(),
            test_pdf_base: None,
            allow_http: false,
        }
    }

    fn publisher_pdf_url(&self, bibcode: &str) -> Result<String> {
        self.test_pdf_base
            .as_ref()
            .map(|base| format!("{base}/pub/{bibcode}"))
            .map(Ok)
            .unwrap_or_else(|| publisher_pdf_url(bibcode))
    }

    fn eprint_pdf_url(&self, bibcode: &str) -> Result<String> {
        self.test_pdf_base
            .as_ref()
            .map(|base| format!("{base}/eprint/{bibcode}"))
            .map(Ok)
            .unwrap_or_else(|| eprint_pdf_url(bibcode))
    }

    fn ads_pdf_url(&self, bibcode: &str) -> Result<String> {
        self.test_pdf_base
            .as_ref()
            .map(|base| format!("{base}/ads/{bibcode}"))
            .map(Ok)
            .unwrap_or_else(|| ads_pdf_url(bibcode))
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ArxivMetadata {
    pub(crate) arxiv_id: String,
    pub(crate) title: Option<String>,
    pub(crate) authors: Option<Vec<String>>,
    pub(crate) abstract_text: Option<String>,
    pub(crate) publication_date: Option<String>,
    pub(crate) container_title: Option<String>,
    pub(crate) doi: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) keywords: Option<Vec<String>>,
    pdf_url: String,
}

impl ArxivMetadata {
    pub(crate) fn populated_fields(&self) -> BTreeSet<String> {
        [
            ("title", self.title.is_some()),
            ("authors", self.authors.is_some()),
            ("abstract_text", self.abstract_text.is_some()),
            ("publication_date", self.publication_date.is_some()),
            ("container_title", self.container_title.is_some()),
            ("doi", self.doi.is_some()),
            ("url", self.url.is_some()),
            ("keywords", self.keywords.is_some()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(field, _)| field.to_owned())
        .collect()
    }
}

enum ImportMetadata {
    Ads {
        bibtex: String,
    },
    Arxiv {
        metadata: Box<ArxivMetadata>,
        atom: String,
    },
}

struct StagedPdf {
    path: PathBuf,
    hash: String,
    embedded: EmbeddedMetadata,
}

impl Drop for StagedPdf {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

enum DownloadFailure {
    Unavailable,
    Error(LitmanError),
}

#[derive(Debug, Serialize, Deserialize)]
struct ImportManifest {
    version: u32,
    relative_path: String,
    staged_name: String,
    hash: String,
}

pub fn parse_remote_identifier(
    input: &str,
    provider: RemoteImportProvider,
) -> Result<RemoteIdentifier> {
    let input = input.trim();
    if input.is_empty() || input.len() > 2048 || input.chars().any(char::is_control) {
        return Err(import_error(
            "identifier must contain between 1 and 2048 safe characters",
        ));
    }
    let parsed_from_url = if input.contains("://") {
        Some(parse_remote_url(input)?)
    } else {
        None
    };
    match provider {
        RemoteImportProvider::Auto => {
            if let Some(identifier) = parsed_from_url {
                return Ok(identifier);
            }
            if let Ok(source_id) = normalize_arxiv_id(input) {
                return Ok(RemoteIdentifier {
                    provider: RemoteProvider::Arxiv,
                    source_id,
                });
            }
            validate_remote_bibcode(input)?;
            Ok(RemoteIdentifier {
                provider: RemoteProvider::Scixplorer,
                source_id: input.to_owned(),
            })
        }
        RemoteImportProvider::Scixplorer => {
            if let Some(identifier) = parsed_from_url {
                if identifier.provider != RemoteProvider::Scixplorer {
                    return Err(import_error("the URL is not an ADS/SciXplorer URL"));
                }
                return Ok(identifier);
            }
            validate_remote_bibcode(input)?;
            Ok(RemoteIdentifier {
                provider: RemoteProvider::Scixplorer,
                source_id: input.to_owned(),
            })
        }
        RemoteImportProvider::Arxiv => {
            if let Some(identifier) = parsed_from_url {
                if identifier.provider != RemoteProvider::Arxiv {
                    return Err(import_error("the URL is not an arXiv URL"));
                }
                return Ok(identifier);
            }
            Ok(RemoteIdentifier {
                provider: RemoteProvider::Arxiv,
                source_id: normalize_arxiv_id(input)?,
            })
        }
    }
}

impl Library {
    pub fn import_remote(
        &mut self,
        input: &str,
        provider: RemoteImportProvider,
        local_pdf: Option<&Path>,
        cancellation: Option<&AtomicBool>,
    ) -> Result<RemoteImportResult> {
        self.import_remote_with_endpoints(
            input,
            provider,
            local_pdf,
            cancellation,
            &RemoteEndpoints::production(),
        )
    }

    fn import_remote_with_endpoints(
        &mut self,
        input: &str,
        provider: RemoteImportProvider,
        local_pdf: Option<&Path>,
        cancellation: Option<&AtomicBool>,
        endpoints: &RemoteEndpoints,
    ) -> Result<RemoteImportResult> {
        let identifier = parse_remote_identifier(input, provider)?;
        if local_pdf.is_some() && identifier.provider != RemoteProvider::Scixplorer {
            return Err(import_error(
                "--file and selected browser downloads are supported only for ADS/SciXplorer imports",
            ));
        }
        check_cancelled(cancellation)?;
        self.ensure_source_id_available(&identifier)?;

        let root_path = self.root_path();
        let root = root_path
            .canonicalize()
            .map_err(|_| LitmanError::RootUnavailable(root_path))?;
        if !root.is_dir() {
            return Err(LitmanError::RootUnavailable(root));
        }
        let filename = destination_filename(&identifier)?;
        let relative_path = filename.clone();
        let destination = root.join(&filename);
        ensure_new_destination(&root, &destination)?;

        let (metadata, remote_pdfs) = match identifier.provider {
            RemoteProvider::Scixplorer => {
                let token = self
                    .config()
                    .scixplorer_api_token
                    .clone()
                    .ok_or(LitmanError::MissingScixplorerToken)?;
                let client = crate::ScixplorerClient::with_api_base(
                    token,
                    &endpoints.ads_api_base,
                    endpoints.allow_http,
                )?;
                let sources = client.pdf_sources(&identifier.source_id)?;
                let bibtex = client.bibtex(&identifier.source_id)?;
                let parsed = parse_bibtex(&bibtex)?;
                if parsed.bibcode != identifier.source_id {
                    return Err(import_error(
                        "ADS BibTeX did not match the requested bibcode",
                    ));
                }
                let mut candidates = Vec::with_capacity(3);
                if sources.pub_pdf {
                    candidates.push((
                        endpoints.publisher_pdf_url(&identifier.source_id)?,
                        RemotePdfSource::PubPdf,
                    ));
                }
                if sources.eprint_pdf {
                    candidates.push((
                        endpoints.eprint_pdf_url(&identifier.source_id)?,
                        RemotePdfSource::EprintPdf,
                    ));
                }
                if sources.ads_pdf {
                    candidates.push((
                        endpoints.ads_pdf_url(&identifier.source_id)?,
                        RemotePdfSource::AdsPdf,
                    ));
                }
                (ImportMetadata::Ads { bibtex }, candidates)
            }
            RemoteProvider::Arxiv => {
                let (metadata, atom) = fetch_arxiv_metadata(&identifier.source_id, endpoints)?;
                let pdf_url = metadata.pdf_url.clone();
                (
                    ImportMetadata::Arxiv {
                        metadata: Box::new(metadata),
                        atom,
                    },
                    vec![(pdf_url, RemotePdfSource::ArxivPdf)],
                )
            }
        };
        check_cancelled(cancellation)?;

        let (staged, pdf_source) = if let Some(path) = local_pdf {
            (
                stage_local_pdf(&root, path, cancellation)?,
                RemotePdfSource::LocalFile,
            )
        } else {
            if remote_pdfs.is_empty() {
                return Err(import_error(
                    "none of PUB_PDF, EPRINT_PDF, or ADS_PDF is available for this ADS record",
                ));
            }
            let mut downloaded = None;
            for (url, source) in remote_pdfs {
                match stage_remote_pdf(
                    &root,
                    &url,
                    source == RemotePdfSource::PubPdf,
                    cancellation,
                    endpoints.allow_http,
                ) {
                    Ok(staged) => {
                        downloaded = Some((staged, source));
                        break;
                    }
                    Err(DownloadFailure::Unavailable) => {}
                    Err(failure) => return Err(download_error(failure)),
                }
            }
            downloaded.ok_or_else(|| download_error(DownloadFailure::Unavailable))?
        };

        check_cancelled(cancellation)?;
        self.ensure_import_available(&identifier, &destination, &staged.hash)?;
        let manifest_path = import_manifest_path(&self.config_path);
        let manifest = ImportManifest {
            version: 1,
            relative_path: relative_path.clone(),
            staged_name: staged
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| import_error("staged PDF name is not portable Unicode"))?
                .to_owned(),
            hash: staged.hash.clone(),
        };
        write_import_manifest(&manifest_path, &manifest)?;

        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let installed = (|| -> Result<RemoteImportResult> {
            self.ensure_import_available(&identifier, &destination, &staged.hash)?;
            fs::rename(&staged.path, &destination)?;
            let file_metadata = fs::metadata(&destination)?;
            let id = self.insert_scanned(ScannedData {
                relative_path: &relative_path,
                file_size: file_metadata.len(),
                modified_unix_ms: modified_unix_ms(&file_metadata),
                content_hash: &staged.hash,
                embedded: Some(&staged.embedded),
                scan_error: None,
                duplicate_of: None,
            })?;
            let paper = match metadata {
                ImportMetadata::Ads { bibtex } => self.store_bibtex(&id, &bibtex)?,
                ImportMetadata::Arxiv { metadata, atom } => {
                    self.store_arxiv_metadata(&id, *metadata, &atom)?
                }
            };
            Ok(RemoteImportResult {
                paper,
                provider: identifier.provider,
                source_id: identifier.source_id.clone(),
                pdf_source,
                relative_path: relative_path.clone(),
            })
        })();

        match installed {
            Ok(result) => {
                if let Err(error) = self.connection.execute_batch("COMMIT") {
                    let _ = self.connection.execute_batch("ROLLBACK");
                    cleanup_import_file(&destination, &staged.hash)?;
                    let _ = fs::remove_file(&manifest_path);
                    return Err(error.into());
                }
                fs::remove_file(manifest_path)?;
                Ok(result)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                cleanup_import_file(&destination, &staged.hash)?;
                let _ = fs::remove_file(&manifest_path);
                Err(error)
            }
        }
    }

    pub(crate) fn recover_remote_imports(&mut self) -> Result<()> {
        let root_path = self.root_path();
        let Ok(root) = root_path.canonicalize() else {
            return Ok(());
        };
        let directory = self.config_path.parent().unwrap_or_else(|| Path::new("."));
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with(IMPORT_MANIFEST_PREFIX) || !name.ends_with(".json") {
                continue;
            }
            let manifest: ImportManifest = serde_json::from_slice(&fs::read(&path)?)?;
            recover_import_manifest(self, &root, &path, &manifest)?;
        }
        Ok(())
    }

    fn ensure_source_id_available(&self, identifier: &RemoteIdentifier) -> Result<()> {
        let (column, value) = match identifier.provider {
            RemoteProvider::Scixplorer => ("bibcode", &identifier.source_id),
            RemoteProvider::Arxiv => ("arxiv_id", &identifier.source_id),
        };
        let existing = self
            .connection
            .query_row(
                &format!("SELECT id FROM papers WHERE {column} = ?1 LIMIT 1"),
                params![value],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Err(import_error(format!(
                "{} is already imported as paper {id}",
                identifier.source_id
            )));
        }
        Ok(())
    }

    fn ensure_import_available(
        &self,
        identifier: &RemoteIdentifier,
        destination: &Path,
        hash: &str,
    ) -> Result<()> {
        self.ensure_source_id_available(identifier)?;
        ensure_new_destination(&self.root_path().canonicalize()?, destination)?;
        if let Some(id) = self.present_id_by_hash(hash, None)? {
            return Err(import_error(format!(
                "the downloaded PDF duplicates existing paper {id}"
            )));
        }
        Ok(())
    }
}

fn fetch_arxiv_metadata(
    source_id: &str,
    endpoints: &RemoteEndpoints,
) -> Result<(ArxivMetadata, String)> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .https_only(!endpoints.allow_http)
            .max_redirects(4)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("LitMan/", env!("CARGO_PKG_VERSION")))
            .build(),
    );
    let mut response = agent
        .get(&endpoints.arxiv_api_url)
        .query("id_list", source_id)
        .query("max_results", "1")
        .call()
        .map_err(|error| import_error(format!("arXiv API request failed: {error}")))?;
    if !endpoints.allow_http && response.get_uri().scheme_str() != Some("https") {
        return Err(import_error(
            "arXiv metadata request redirected to a non-HTTPS URL",
        ));
    }
    let atom = response
        .body_mut()
        .with_config()
        .limit(MAX_METADATA_SIZE)
        .read_to_string()
        .map_err(|error| import_error(format!("cannot read arXiv metadata: {error}")))?;
    let metadata = parse_arxiv_atom(&atom, source_id, endpoints)?;
    Ok((metadata, atom))
}

fn parse_arxiv_atom(
    atom: &str,
    source_id: &str,
    endpoints: &RemoteEndpoints,
) -> Result<ArxivMetadata> {
    let mut reader = Reader::from_str(atom);
    reader.config_mut().trim_text(true);
    let mut stack = Vec::<String>::new();
    let mut metadata = ArxivMetadata {
        arxiv_id: source_id.to_owned(),
        url: Some(format!("https://arxiv.org/abs/{source_id}")),
        ..Default::default()
    };
    let mut authors = Vec::new();
    let mut current_author = None;
    let mut categories = Vec::new();
    let mut entry_seen = false;
    let mut api_error_entry = false;
    let mut returned_id = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" {
                    entry_seen = true;
                } else if name == "author" && stack.iter().any(|item| item == "entry") {
                    current_author = None;
                }
                handle_atom_attributes(&reader, &event, &name, &mut metadata, &mut categories)?;
                stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(event.name().as_ref());
                handle_atom_attributes(&reader, &event, &name, &mut metadata, &mut categories)?;
            }
            Ok(Event::Text(text)) => {
                let value = text
                    .decode()
                    .map_err(|error| import_error(format!("invalid arXiv XML text: {error}")))?;
                let value = clean_xml_text(&value);
                if value.is_empty() || !stack.iter().any(|item| item == "entry") {
                    continue;
                }
                match stack.last().map(String::as_str) {
                    Some("title") => {
                        api_error_entry |= value.eq_ignore_ascii_case("error");
                        metadata.title = Some(value);
                    }
                    Some("summary") => metadata.abstract_text = Some(value),
                    Some("published") => {
                        metadata.publication_date = Some(value.chars().take(10).collect())
                    }
                    Some("name") if stack.iter().any(|item| item == "author") => {
                        current_author = Some(value)
                    }
                    Some("journal_ref") => metadata.container_title = Some(value),
                    Some("doi") => metadata.doi = Some(value),
                    Some("id") => {
                        if value.contains("/api/errors#") {
                            api_error_entry = true;
                        } else if let Some(value) = value.rsplit("/abs/").next()
                            && let Ok(value) = normalize_arxiv_id(value)
                        {
                            returned_id = Some(value);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "author"
                    && let Some(author) = current_author.take()
                {
                    authors.push(author);
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(import_error(format!("invalid arXiv Atom XML: {error}"))),
        }
    }
    if !entry_seen || api_error_entry {
        return Err(import_error(format!(
            "arXiv did not return a paper for {source_id}"
        )));
    }
    if !returned_id
        .as_deref()
        .is_some_and(|returned| arxiv_ids_match(source_id, returned))
    {
        return Err(import_error(format!(
            "arXiv returned a record that does not match {source_id}"
        )));
    }
    if metadata.title.is_none() || metadata.pdf_url.is_empty() {
        return Err(import_error(
            "arXiv response is missing a title or PDF link",
        ));
    }
    if !authors.is_empty() {
        metadata.authors = Some(authors);
    }
    let mut seen = BTreeSet::new();
    categories.retain(|value| seen.insert(value.clone()));
    if !categories.is_empty() {
        metadata.keywords = Some(categories);
    }
    metadata.pdf_url = secure_arxiv_pdf_url(&metadata.pdf_url, endpoints)?;
    Ok(metadata)
}

fn handle_atom_attributes(
    reader: &Reader<&[u8]>,
    event: &BytesStart<'_>,
    name: &str,
    metadata: &mut ArxivMetadata,
    categories: &mut Vec<String>,
) -> Result<()> {
    let mut term = None;
    let mut href = None;
    let mut title = None;
    let mut content_type = None;
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| import_error(error.to_string()))?;
        let key = local_name(attribute.key.as_ref());
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| import_error(error.to_string()))?
            .into_owned();
        match key.as_str() {
            "term" => term = Some(value),
            "href" => href = Some(value),
            "title" => title = Some(value),
            "type" => content_type = Some(value),
            _ => {}
        }
    }
    if matches!(name, "category" | "primary_category")
        && let Some(term) = term
        && !term.trim().is_empty()
    {
        if name == "primary_category" {
            categories.insert(0, term);
        } else {
            categories.push(term);
        }
    }
    if name == "link"
        && title.as_deref() == Some("pdf")
        && content_type.as_deref() == Some("application/pdf")
        && let Some(href) = href
    {
        metadata.pdf_url = href;
    }
    Ok(())
}

fn stage_remote_pdf(
    root: &Path,
    url: &str,
    browser_fallback: bool,
    cancellation: Option<&AtomicBool>,
    allow_http: bool,
) -> std::result::Result<StagedPdf, DownloadFailure> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .https_only(!allow_http)
            .max_redirects(8)
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(concat!("LitMan/", env!("CARGO_PKG_VERSION")))
            .build(),
    );
    let response = agent.get(url).call().map_err(|error| match error {
        ureq::Error::StatusCode(404 | 410) => DownloadFailure::Unavailable,
        ureq::Error::StatusCode(401 | 403) if browser_fallback => {
            DownloadFailure::Error(LitmanError::PublisherPdfBrowserRequired {
                gateway_url: url.to_owned(),
            })
        }
        error => DownloadFailure::Error(import_error(format!("PDF request failed: {error}"))),
    })?;
    if !allow_http && response.get_uri().scheme_str() != Some("https") {
        return Err(DownloadFailure::Error(import_error(
            "PDF request redirected to a non-HTTPS URL",
        )));
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
        return if browser_fallback {
            Err(DownloadFailure::Error(
                LitmanError::PublisherPdfBrowserRequired {
                    gateway_url: url.to_owned(),
                },
            ))
        } else {
            Err(DownloadFailure::Unavailable)
        };
    }
    if response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > MAX_PDF_SIZE)
    {
        return Err(DownloadFailure::Error(import_error(
            "PDF exceeds the 256 MiB limit",
        )));
    }
    let (_, body) = response.into_parts();
    stage_reader(root, body.into_reader(), cancellation).map_err(DownloadFailure::Error)
}

fn stage_local_pdf(
    root: &Path,
    source: &Path,
    cancellation: Option<&AtomicBool>,
) -> Result<StagedPdf> {
    let source = source.canonicalize()?;
    if !source.is_file() {
        return Err(import_error("selected PDF is not a regular file"));
    }
    stage_reader(root, File::open(source)?, cancellation)
}

fn stage_reader(
    root: &Path,
    mut reader: impl Read,
    cancellation: Option<&AtomicBool>,
) -> Result<StagedPdf> {
    let path = root.join(format!(".litman-remote-import-{}.tmp", Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let copied = match copy_bounded(&mut reader, &mut output, cancellation) {
        Ok(copied) => copied,
        Err(error) => {
            drop(output);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
    };
    if let Err(error) = output.sync_all() {
        drop(output);
        let _ = fs::remove_file(&path);
        return Err(error.into());
    }
    drop(output);
    if copied < 5 {
        let _ = fs::remove_file(&path);
        return Err(import_error("downloaded file is not a PDF"));
    }
    let hash = hash_pdf(&path).map_err(|error| {
        let _ = fs::remove_file(&path);
        import_error(format!("downloaded file is not a valid PDF: {error}"))
    })?;
    let embedded = match catch_unwind(AssertUnwindSafe(|| extract_pdf_metadata(&path))) {
        Ok(Ok(metadata)) => metadata,
        Ok(Err(error)) => {
            let _ = fs::remove_file(&path);
            return Err(import_error(format!(
                "downloaded PDF cannot be parsed: {error}"
            )));
        }
        Err(_) => {
            let _ = fs::remove_file(&path);
            return Err(import_error("downloaded PDF parser panicked"));
        }
    };
    Ok(StagedPdf {
        path,
        hash,
        embedded,
    })
}

fn copy_bounded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    cancellation: Option<&AtomicBool>,
) -> Result<u64> {
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancelled(cancellation)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > MAX_PDF_SIZE {
            return Err(import_error("PDF exceeds the 256 MiB limit"));
        }
        writer.write_all(&buffer[..count])?;
    }
    Ok(total)
}

fn parse_remote_url(input: &str) -> Result<RemoteIdentifier> {
    let remainder = input
        .strip_prefix("https://")
        .ok_or_else(|| import_error("remote paper URLs must use HTTPS"))?;
    let (authority, rest) = remainder.split_once('/').unwrap_or((remainder, ""));
    if authority.is_empty() || authority.contains(['@', ':']) {
        return Err(import_error("URL authority is not allowed"));
    }
    let host = authority.to_ascii_lowercase();
    let (path_and_query, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let path = path_and_query.split('?').next().unwrap_or_default();
    if matches!(host.as_str(), "ui.adsabs.harvard.edu" | "scixplorer.org") {
        let route = if path.is_empty() && !fragment.is_empty() {
            fragment.split('?').next().unwrap_or_default()
        } else {
            path
        };
        let bibcode = route
            .strip_prefix("abs/")
            .and_then(|value| value.strip_suffix("/abstract"))
            .ok_or_else(|| import_error("unrecognized ADS/SciXplorer abstract URL"))?;
        let bibcode = percent_decode(bibcode)?;
        if bibcode.contains('/') {
            return Err(import_error(
                "ADS bibcode URL contains extra path components",
            ));
        }
        validate_remote_bibcode(&bibcode)?;
        return Ok(RemoteIdentifier {
            provider: RemoteProvider::Scixplorer,
            source_id: bibcode,
        });
    }
    if matches!(host.as_str(), "arxiv.org" | "www.arxiv.org") {
        let identifier = path
            .strip_prefix("abs/")
            .or_else(|| path.strip_prefix("pdf/"))
            .ok_or_else(|| import_error("unrecognized arXiv URL"))?;
        let identifier = percent_decode(identifier)?;
        let identifier = identifier.strip_suffix(".pdf").unwrap_or(&identifier);
        return Ok(RemoteIdentifier {
            provider: RemoteProvider::Arxiv,
            source_id: normalize_arxiv_id(identifier)?,
        });
    }
    Err(import_error("URL host is not supported"))
}

fn normalize_arxiv_id(input: &str) -> Result<String> {
    let input = input.trim();
    let input = input
        .strip_prefix("arXiv:")
        .or_else(|| input.strip_prefix("arxiv:"))
        .unwrap_or(input);
    if input.is_empty()
        || input.len() > 80
        || input
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(import_error("invalid arXiv identifier"));
    }
    let (base, version) = split_arxiv_version(input)?;
    let normalized_base = if let Some((date, number)) = base.split_once('.') {
        (date.len() == 4
            && date.bytes().all(|byte| byte.is_ascii_digit())
            && (4..=5).contains(&number.len())
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && valid_month(&date[2..]))
        .then(|| base.to_owned())
    } else if let Some((archive, number)) = base.rsplit_once('/') {
        (!archive.is_empty()
            && archive
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            && number.len() == 7
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && valid_month(&number[2..4]))
        .then(|| format!("{}/{number}", archive.to_ascii_lowercase()))
    } else {
        None
    };
    normalized_base
        .map(|base| format!("{base}{version}"))
        .ok_or_else(|| import_error("invalid arXiv identifier"))
}

fn validate_remote_bibcode(bibcode: &str) -> Result<()> {
    validate_bibcode(bibcode)?;
    if bibcode.len() != 19 {
        return Err(import_error(
            "ADS bibcodes must contain exactly 19 characters",
        ));
    }
    Ok(())
}

fn split_arxiv_version(input: &str) -> Result<(&str, &str)> {
    let Some(index) = input.rfind('v') else {
        return Ok((input, ""));
    };
    let suffix = &input[index + 1..];
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok((input, ""));
    }
    if suffix.starts_with('0') {
        return Err(import_error("arXiv version must be a positive integer"));
    }
    Ok((&input[..index], &input[index..]))
}

fn arxiv_ids_match(requested: &str, returned: &str) -> bool {
    if requested == returned {
        return true;
    }
    let (requested_base, requested_version) =
        split_arxiv_version(requested).unwrap_or((requested, ""));
    let (returned_base, _) = split_arxiv_version(returned).unwrap_or((returned, ""));
    requested_version.is_empty() && requested_base == returned_base
}

fn valid_month(value: &str) -> bool {
    value
        .parse::<u8>()
        .is_ok_and(|month| (1..=12).contains(&month))
}

fn destination_filename(identifier: &RemoteIdentifier) -> Result<String> {
    match identifier.provider {
        RemoteProvider::Scixplorer => {
            validate_portable_filename(&identifier.source_id)?;
            Ok(format!("{}.pdf", identifier.source_id))
        }
        RemoteProvider::Arxiv => {
            let portable = identifier.source_id.replace('/', "_");
            validate_portable_filename(&portable)?;
            Ok(format!("arXiv-{portable}.pdf"))
        }
    }
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
        return Err(import_error(
            "identifier cannot be used as a portable PDF filename",
        ));
    }
    Ok(())
}

fn secure_arxiv_pdf_url(url: &str, endpoints: &RemoteEndpoints) -> Result<String> {
    if endpoints.allow_http
        && endpoints
            .test_pdf_base
            .as_ref()
            .is_some_and(|base| url.starts_with(&format!("{base}/")))
    {
        return Ok(url.to_owned());
    }
    let secure = url
        .strip_prefix("http://arxiv.org/")
        .map(|suffix| format!("https://arxiv.org/{suffix}"))
        .unwrap_or_else(|| url.to_owned());
    if !secure.starts_with("https://arxiv.org/pdf/")
        && !secure.starts_with("https://www.arxiv.org/pdf/")
    {
        return Err(import_error("arXiv returned an unexpected PDF URL"));
    }
    Ok(secure)
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(import_error("URL contains an invalid percent escape"));
            }
            let high = hex_digit(bytes[index + 1])?;
            let low = hex_digit(bytes[index + 2])?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| import_error("URL path is not valid UTF-8"))
}

fn hex_digit(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(import_error("URL contains an invalid percent escape")),
    }
}

fn ensure_new_destination(root: &Path, destination: &Path) -> Result<()> {
    if !destination.starts_with(root) {
        return Err(import_error("import destination escapes the library root"));
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(import_error(format!(
            "import destination already exists: {}",
            destination.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn import_manifest_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{IMPORT_MANIFEST_PREFIX}{}.json", Uuid::new_v4()))
}

fn write_import_manifest(path: &Path, manifest: &ImportManifest) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    serde_json::to_writer_pretty(&mut file, manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)?;
    Ok(())
}

fn recover_import_manifest(
    library: &mut Library,
    root: &Path,
    path: &Path,
    manifest: &ImportManifest,
) -> Result<()> {
    if manifest.version != 1 {
        return Err(import_error(format!(
            "unsupported remote-import manifest: {}",
            path.display()
        )));
    }
    let destination = resolve_relative(root, &manifest.relative_path)?;
    let staged = resolve_relative(root, &manifest.staged_name)?;
    if let Some(paper) = library.paper_by_path(&manifest.relative_path)? {
        if paper.content_hash != manifest.hash
            || existing_hash(&destination)?.as_deref() != Some(&manifest.hash)
        {
            return Err(import_error(format!(
                "completed remote import does not match its manifest; preserved files: {}",
                path.display()
            )));
        }
        cleanup_import_file(&staged, &manifest.hash)?;
        fs::remove_file(path)?;
        return Ok(());
    }
    cleanup_import_file(&destination, &manifest.hash)?;
    cleanup_import_file(&staged, &manifest.hash)?;
    fs::remove_file(path)?;
    Ok(())
}

fn resolve_relative(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(value) => path.push(value),
            _ => return Err(import_error("unsafe path in remote-import manifest")),
        }
    }
    if path == root || !path.starts_with(root) {
        return Err(import_error(
            "remote-import manifest escapes the library root",
        ));
    }
    Ok(path)
}

fn cleanup_import_file(path: &Path, expected_hash: &str) -> Result<()> {
    match existing_hash(path)? {
        Some(hash) if hash == expected_hash => fs::remove_file(path).map_err(Into::into),
        Some(_) => Err(import_error(format!(
            "import cleanup found unexpected file content; preserved {}",
            path.display()
        ))),
        None => Ok(()),
    }
}

fn existing_hash(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(import_error(format!(
                "unexpected non-file import path: {}",
                path.display()
            )))
        }
        Ok(_) => hash_pdf(path).map(Some),
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

fn local_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name)
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn clean_xml_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn check_cancelled(cancellation: Option<&AtomicBool>) -> Result<()> {
    if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err(import_error("operation was cancelled"))
    } else {
        Ok(())
    }
}

fn download_error(failure: DownloadFailure) -> LitmanError {
    match failure {
        DownloadFailure::Unavailable => import_error("requested PDF source is unavailable"),
        DownloadFailure::Error(error) => error,
    }
}

fn import_error(message: impl Into<String>) -> LitmanError {
    LitmanError::RemoteImport(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use lopdf::{Document, Object, dictionary};
    use tempfile::TempDir;

    type MockServer = (
        String,
        Arc<Mutex<Vec<(String, bool)>>>,
        thread::JoinHandle<()>,
    );

    fn pdf_bytes(title: &str) -> Vec<u8> {
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
        let catalog_id =
            document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        let info_id = document.add_object(dictionary! { "Title" => Object::string_literal(title) });
        document.trailer.set("Root", catalog_id);
        document.trailer.set("Info", info_id);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        bytes
    }

    fn spawn_mock_server(
        request_count: usize,
        handler: impl Fn(&str, &str, &str) -> (u16, &'static str, Vec<u8>) + Send + 'static,
    ) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let thread_base = base.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = requests.clone();
        let handle = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut received = Vec::new();
                loop {
                    let mut buffer = [0_u8; 4096];
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    received.extend_from_slice(&buffer[..count]);
                    let Some(header_end) = received.windows(4).position(|part| part == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&received[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if received.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&received);
                let request_line = request.lines().next().unwrap_or_default();
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let method = request_line.split_whitespace().next().unwrap_or("GET");
                let authorized = request.lines().any(|line| {
                    line.to_ascii_lowercase()
                        .starts_with("authorization: bearer ")
                });
                thread_requests
                    .lock()
                    .unwrap()
                    .push((path.to_owned(), authorized));
                let (status, content_type, body) = handler(method, path, &thread_base);
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    410 => "Gone",
                    _ => "Error",
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        (base, requests, handle)
    }

    fn mock_endpoints(base: &str) -> RemoteEndpoints {
        RemoteEndpoints {
            ads_api_base: format!("{base}/v1"),
            arxiv_api_url: format!("{base}/api/query"),
            test_pdf_base: Some(base.into()),
            allow_http: true,
        }
    }

    #[test]
    fn supplied_urls_and_bare_identifiers_are_parsed() {
        let ads = parse_remote_identifier(
            "https://ui.adsabs.harvard.edu/abs/2003ApJ...587..208R/abstract",
            RemoteImportProvider::Auto,
        )
        .unwrap();
        assert_eq!(ads.provider, RemoteProvider::Scixplorer);
        assert_eq!(ads.source_id, "2003ApJ...587..208R");

        for input in [
            "0908.3637",
            "https://arxiv.org/abs/0908.3637",
            "https://arxiv.org/pdf/0908.3637",
            "https://arxiv.org/pdf/0908.3637.pdf?download=1",
        ] {
            let parsed = parse_remote_identifier(input, RemoteImportProvider::Auto).unwrap();
            assert_eq!(parsed.provider, RemoteProvider::Arxiv);
            assert_eq!(parsed.source_id, "0908.3637");
        }
        assert_eq!(
            parse_remote_identifier("Astro-PH/9901234v2", RemoteImportProvider::Auto)
                .unwrap()
                .source_id,
            "astro-ph/9901234v2"
        );
        assert_eq!(
            parse_remote_identifier("2003ApJ...587..208R", RemoteImportProvider::Auto)
                .unwrap()
                .provider,
            RemoteProvider::Scixplorer
        );
    }

    #[test]
    fn malformed_or_untrusted_urls_are_rejected() {
        for input in [
            "http://arxiv.org/abs/0908.3637",
            "https://example.com/abs/0908.3637",
            "https://arxiv.org/abs/../../secret",
            "https://ui.adsabs.harvard.edu/abs/../../settings/abstract",
            "not-a-bibcode",
            "0908.3637/../../secret",
            "https://arxiv.org/abs/0908%2e3637%2f..%2fsecret",
        ] {
            assert!(parse_remote_identifier(input, RemoteImportProvider::Auto).is_err());
        }
    }

    #[test]
    fn arxiv_atom_metadata_and_pdf_link_are_decoded() {
        let atom = r#"<?xml version="1.0"?>
        <feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
          <entry>
            <id>http://arxiv.org/abs/0908.3637</id>
            <published>2009-08-25T00:00:00Z</published>
            <title>  A Chinese 中文 title </title>
            <summary>An abstract.</summary>
            <author><name>First Author</name></author>
            <author><name>Second Author</name></author>
            <arxiv:journal_ref>Example Journal 1, 2</arxiv:journal_ref>
            <arxiv:doi>10.1000/example</arxiv:doi>
            <arxiv:primary_category term="astro-ph.CO" scheme="http://arxiv.org/schemas/atom"/>
            <category term="gr-qc" scheme="http://arxiv.org/schemas/atom"/>
            <link title="pdf" href="http://arxiv.org/pdf/0908.3637v2" rel="related" type="application/pdf"/>
          </entry>
        </feed>"#;
        let metadata = parse_arxiv_atom(atom, "0908.3637", &RemoteEndpoints::production()).unwrap();
        assert_eq!(metadata.title.as_deref(), Some("A Chinese 中文 title"));
        assert_eq!(
            metadata.authors.unwrap(),
            vec!["First Author", "Second Author"]
        );
        assert_eq!(metadata.publication_date.as_deref(), Some("2009-08-25"));
        assert_eq!(metadata.pdf_url, "https://arxiv.org/pdf/0908.3637v2");
        assert_eq!(metadata.keywords.unwrap(), vec!["astro-ph.CO", "gr-qc"]);
    }

    #[test]
    fn ads_import_is_transactional_and_keeps_token_off_pdf_request() {
        const BIBCODE: &str = "2003ApJ...587..208R";
        let pdf = pdf_bytes("Embedded ADS title");
        let (base, requests, server) = spawn_mock_server(3, move |_method, path, _base| {
            if path.starts_with("/v1/search/query?") {
                (
                    200,
                    "application/json",
                    format!(
                        r#"{{"response":{{"docs":[{{"bibcode":"{BIBCODE}","esources":["PUB_PDF","EPRINT_PDF"]}}]}}}}"#
                    )
                    .into_bytes(),
                )
            } else if path == "/v1/export/bibtex" {
                (
                    200,
                    "application/json",
                    format!(
                        r#"{{"export":"@ARTICLE{{{BIBCODE}, title={{Remote ADS title}}, author={{First Author and Second Author}}, year={{2003}}}}"}}"#
                    )
                    .into_bytes(),
                )
            } else if path == format!("/pub/{BIBCODE}") {
                (200, "application/pdf", pdf.clone())
            } else {
                panic!("unexpected mock request: {path}");
            }
        });
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let config_path = temporary.path().join("library.toml");
        let mut config = crate::Config::new(root.clone());
        config.scixplorer_api_token = Some("secret-token".into());
        let mut library = Library::init(&config_path, config).unwrap();
        let result = library
            .import_remote_with_endpoints(
                BIBCODE,
                RemoteImportProvider::Auto,
                None,
                None,
                &mock_endpoints(&base),
            )
            .unwrap();
        server.join().unwrap();

        assert_eq!(result.provider, RemoteProvider::Scixplorer);
        assert_eq!(result.pdf_source, RemotePdfSource::PubPdf);
        assert_eq!(result.relative_path, format!("{BIBCODE}.pdf"));
        assert_eq!(result.paper.title.as_deref(), Some("Remote ADS title"));
        assert!(result.paper.bibtex_fields.contains("title"));
        assert!(root.join(format!("{BIBCODE}.pdf")).is_file());
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].1 && requests[1].1);
        assert!(!requests[2].1, "ADS token leaked to the PDF gateway");

        let duplicate = library.import_remote_with_endpoints(
            BIBCODE,
            RemoteImportProvider::Auto,
            None,
            None,
            &mock_endpoints(&base),
        );
        assert!(matches!(duplicate, Err(LitmanError::RemoteImport(_))));
    }

    #[test]
    fn arxiv_import_stores_atom_and_has_stable_result_shape() {
        let pdf = pdf_bytes("Embedded arXiv title");
        let (base, requests, server) = spawn_mock_server(2, move |_method, path, base| {
            if path.starts_with("/api/query?") {
                (
                    200,
                    "application/atom+xml",
                    format!(
                        r#"<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom"><entry><id>http://arxiv.org/abs/0908.3637</id><published>2009-08-25T00:00:00Z</published><title>远程 arXiv 标题</title><summary>摘要</summary><author><name>第一作者</name></author><arxiv:doi>10.1000/arxiv</arxiv:doi><category term="astro-ph.CO"/><link title="pdf" href="{base}/arxiv.pdf" type="application/pdf"/></entry></feed>"#
                    )
                    .into_bytes(),
                )
            } else if path == "/arxiv.pdf" {
                (200, "application/pdf", pdf.clone())
            } else {
                panic!("unexpected mock request: {path}");
            }
        });
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let config_path = temporary.path().join("library.toml");
        let mut library = Library::init(&config_path, crate::Config::new(root.clone())).unwrap();
        let result = library
            .import_remote_with_endpoints(
                "https://arxiv.org/pdf/0908.3637",
                RemoteImportProvider::Auto,
                None,
                None,
                &mock_endpoints(&base),
            )
            .unwrap();
        server.join().unwrap();

        assert_eq!(result.provider, RemoteProvider::Arxiv);
        assert_eq!(result.pdf_source, RemotePdfSource::ArxivPdf);
        assert_eq!(result.paper.title.as_deref(), Some("远程 arXiv 标题"));
        assert_eq!(result.paper.arxiv_id.as_deref(), Some("0908.3637"));
        assert!(
            result
                .paper
                .arxiv_atom_xml
                .as_deref()
                .unwrap()
                .contains("摘要")
        );
        assert!(result.paper.arxiv_fields.contains("title"));
        assert!(root.join("arXiv-0908.3637.pdf").is_file());
        assert!(requests.lock().unwrap().iter().all(|request| !request.1));
        let json = serde_json::to_value(&result).unwrap();
        let object = json.as_object().unwrap();
        for key in [
            "paper",
            "provider",
            "source_id",
            "pdf_source",
            "relative_path",
        ] {
            assert!(object.contains_key(key));
        }
    }

    #[test]
    fn ads_uses_eprint_only_after_confirmed_pub_unavailability() {
        const BIBCODE: &str = "2003ApJ...587..208R";
        let pdf = pdf_bytes("E-print");
        let (base, requests, server) = spawn_mock_server(4, move |_method, path, _base| {
            if path.starts_with("/v1/search/query?") {
                (
                    200,
                    "application/json",
                    format!(
                        r#"{{"response":{{"docs":[{{"bibcode":"{BIBCODE}","esources":["PUB_PDF","EPRINT_PDF"]}}]}}}}"#
                    )
                    .into_bytes(),
                )
            } else if path == "/v1/export/bibtex" {
                (
                    200,
                    "application/json",
                    format!(r#"{{"export":"@ARTICLE{{{BIBCODE}, title={{Paper}}}}"}}"#)
                        .into_bytes(),
                )
            } else if path == format!("/pub/{BIBCODE}") {
                (404, "text/plain", b"unavailable".to_vec())
            } else if path == format!("/eprint/{BIBCODE}") {
                (200, "application/pdf", pdf.clone())
            } else {
                panic!("unexpected mock request: {path}");
            }
        });
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let mut config = crate::Config::new(root);
        config.scixplorer_api_token = Some("secret-token".into());
        let mut library = Library::init(temporary.path().join("library.toml"), config).unwrap();
        let result = library
            .import_remote_with_endpoints(
                BIBCODE,
                RemoteImportProvider::Auto,
                None,
                None,
                &mock_endpoints(&base),
            )
            .unwrap();
        server.join().unwrap();
        assert_eq!(result.pdf_source, RemotePdfSource::EprintPdf);
        let requests = requests.lock().unwrap();
        assert!(requests[2].0.starts_with("/pub/"));
        assert!(requests[3].0.starts_with("/eprint/"));
        assert!(!requests[2].1 && !requests[3].1);
    }

    #[test]
    fn ads_uses_ads_pdf_after_earlier_sources_are_unavailable() {
        const BIBCODE: &str = "2003ApJ...587..208R";
        let pdf = pdf_bytes("ADS-hosted PDF");
        let (base, requests, server) = spawn_mock_server(5, move |_method, path, _base| {
            if path.starts_with("/v1/search/query?") {
                (
                    200,
                    "application/json",
                    format!(
                        r#"{{"response":{{"docs":[{{"bibcode":"{BIBCODE}","esources":["PUB_PDF","EPRINT_PDF","ADS_PDF"]}}]}}}}"#
                    )
                    .into_bytes(),
                )
            } else if path == "/v1/export/bibtex" {
                (
                    200,
                    "application/json",
                    format!(r#"{{"export":"@ARTICLE{{{BIBCODE}, title={{Paper}}}}"}}"#)
                        .into_bytes(),
                )
            } else if path == format!("/pub/{BIBCODE}") {
                (404, "text/plain", b"unavailable".to_vec())
            } else if path == format!("/eprint/{BIBCODE}") {
                (410, "text/plain", b"gone".to_vec())
            } else if path == format!("/ads/{BIBCODE}") {
                (200, "application/pdf", pdf.clone())
            } else {
                panic!("unexpected mock request: {path}");
            }
        });
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let mut config = crate::Config::new(root);
        config.scixplorer_api_token = Some("secret-token".into());
        let mut library = Library::init(temporary.path().join("library.toml"), config).unwrap();
        let result = library
            .import_remote_with_endpoints(
                BIBCODE,
                RemoteImportProvider::Auto,
                None,
                None,
                &mock_endpoints(&base),
            )
            .unwrap();
        server.join().unwrap();

        assert_eq!(result.pdf_source, RemotePdfSource::AdsPdf);
        assert_eq!(serde_json::to_value(result.pdf_source).unwrap(), "ads_pdf");
        let requests = requests.lock().unwrap();
        assert!(requests[2].0.starts_with("/pub/"));
        assert!(requests[3].0.starts_with("/eprint/"));
        assert!(requests[4].0.starts_with("/ads/"));
        assert!(requests[2..].iter().all(|request| !request.1));
    }

    #[test]
    fn publisher_html_requires_browser_and_leaves_library_unchanged() {
        const BIBCODE: &str = "2003ApJ...587..208R";
        let (base, _requests, server) = spawn_mock_server(3, move |_method, path, _base| {
            if path.starts_with("/v1/search/query?") {
                (
                    200,
                    "application/json",
                    format!(
                        r#"{{"response":{{"docs":[{{"bibcode":"{BIBCODE}","esources":["PUB_PDF","EPRINT_PDF"]}}]}}}}"#
                    )
                    .into_bytes(),
                )
            } else if path == "/v1/export/bibtex" {
                (
                    200,
                    "application/json",
                    format!(r#"{{"export":"@ARTICLE{{{BIBCODE}, title={{Paper}}}}"}}"#)
                        .into_bytes(),
                )
            } else if path == format!("/pub/{BIBCODE}") {
                (200, "text/html", b"<html>login</html>".to_vec())
            } else {
                panic!("unexpected mock request: {path}");
            }
        });
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let mut config = crate::Config::new(root.clone());
        config.scixplorer_api_token = Some("secret-token".into());
        let mut library = Library::init(temporary.path().join("library.toml"), config).unwrap();
        let result = library.import_remote_with_endpoints(
            BIBCODE,
            RemoteImportProvider::Auto,
            None,
            None,
            &mock_endpoints(&base),
        );
        server.join().unwrap();
        assert!(matches!(
            result,
            Err(LitmanError::PublisherPdfBrowserRequired { .. })
        ));
        assert!(!root.join(format!("{BIBCODE}.pdf")).exists());
        assert!(library.list_papers(&Default::default()).unwrap().is_empty());
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(IMPORT_MANIFEST_PREFIX)
        }));
    }

    #[test]
    fn selected_browser_pdf_is_copied_and_remains_byte_identical() {
        const BIBCODE: &str = "2003ApJ...587..208R";
        let (base, _requests, server) = spawn_mock_server(2, move |_method, path, _base| {
            if path.starts_with("/v1/search/query?") {
                (
                    200,
                    "application/json",
                    format!(
                        r#"{{"response":{{"docs":[{{"bibcode":"{BIBCODE}","esources":["PUB_PDF"]}}]}}}}"#
                    )
                    .into_bytes(),
                )
            } else if path == "/v1/export/bibtex" {
                (
                    200,
                    "application/json",
                    format!(r#"{{"export":"@ARTICLE{{{BIBCODE}, title={{Paper}}}}"}}"#)
                        .into_bytes(),
                )
            } else {
                panic!("unexpected mock request: {path}");
            }
        });
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let selected = temporary.path().join("browser-download.pdf");
        let selected_bytes = pdf_bytes("Browser download");
        fs::write(&selected, &selected_bytes).unwrap();
        let mut config = crate::Config::new(root.clone());
        config.scixplorer_api_token = Some("secret-token".into());
        let mut library = Library::init(temporary.path().join("library.toml"), config).unwrap();
        let result = library
            .import_remote_with_endpoints(
                BIBCODE,
                RemoteImportProvider::Auto,
                Some(&selected),
                None,
                &mock_endpoints(&base),
            )
            .unwrap();
        server.join().unwrap();
        assert_eq!(result.pdf_source, RemotePdfSource::LocalFile);
        assert_eq!(fs::read(&selected).unwrap(), selected_bytes);
        assert_eq!(
            fs::read(root.join(format!("{BIBCODE}.pdf"))).unwrap(),
            selected_bytes
        );
    }

    #[test]
    fn cancellation_and_destination_collision_stop_before_network() {
        const BIBCODE: &str = "2003ApJ...587..208R";
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let mut config = crate::Config::new(root.clone());
        config.scixplorer_api_token = Some("secret-token".into());
        let mut library = Library::init(temporary.path().join("library.toml"), config).unwrap();
        let endpoints = mock_endpoints("http://127.0.0.1:1");
        let cancelled = AtomicBool::new(true);
        assert!(
            library
                .import_remote_with_endpoints(
                    BIBCODE,
                    RemoteImportProvider::Auto,
                    None,
                    Some(&cancelled),
                    &endpoints,
                )
                .is_err()
        );
        fs::write(root.join(format!("{BIBCODE}.pdf")), b"reserved").unwrap();
        let collision = library.import_remote_with_endpoints(
            BIBCODE,
            RemoteImportProvider::Auto,
            None,
            None,
            &endpoints,
        );
        assert!(matches!(collision, Err(LitmanError::RemoteImport(_))));
        assert_eq!(
            fs::read(root.join(format!("{BIBCODE}.pdf"))).unwrap(),
            b"reserved"
        );
        assert!(library.list_papers(&Default::default()).unwrap().is_empty());
    }

    #[test]
    fn cancellation_during_staging_removes_the_partial_file() {
        let temporary = TempDir::new().unwrap();
        let cancellation = AtomicBool::new(true);
        let result = stage_reader(
            temporary.path(),
            std::io::Cursor::new(pdf_bytes("Cancelled")),
            Some(&cancellation),
        );
        assert!(result.is_err());
        assert!(fs::read_dir(temporary.path()).unwrap().next().is_none());
    }
}
