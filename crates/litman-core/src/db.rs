use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::backup::Backup;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::config::{Config, Language};
use crate::model::{EmbeddedMetadata, FileStatus, Group, Paper, PaperUpdate};
use crate::remote_import::ArxivMetadata;
use crate::scixplorer::parse_bibtex;
use crate::{LitmanError, Result, ScixplorerClient, scixplorer_url};

const DATABASE_SCHEMA_VERSION: i64 = 3;

const PAPER_COLUMNS: &str = "
    id, relative_path, file_size, modified_unix_ms, content_hash, file_status,
    scan_error, duplicate_of, title, authors_json, abstract_text, publication_date,
    container_title, volume, issue, pages, doi, url, language, keywords_json, notes,
    importance, page_count, pdf_version, encrypted, creator, producer,
    embedded_json, manual_overrides_json, created_at, updated_at,
    bibtex, bibcode, bibtex_fields_json, arxiv_id, arxiv_atom_xml, arxiv_fields_json";

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub query: Option<String>,
    pub group_path: Option<String>,
    pub importance: Option<u8>,
    pub min_importance: Option<u8>,
    pub unrated: bool,
    pub status: Option<FileStatus>,
    pub limit: Option<usize>,
}

pub(crate) struct ScannedData<'a> {
    pub relative_path: &'a str,
    pub file_size: u64,
    pub modified_unix_ms: i64,
    pub content_hash: &'a str,
    pub embedded: Option<&'a EmbeddedMetadata>,
    pub scan_error: Option<&'a str>,
    pub duplicate_of: Option<&'a str>,
}

pub struct Library {
    pub(crate) config_path: PathBuf,
    pub(crate) config: Config,
    pub(crate) connection: Connection,
}

impl Library {
    pub fn init(config_path: impl AsRef<Path>, config: Config) -> Result<Self> {
        let config_path = absolute_lexical(config_path.as_ref())?;
        config.save(&config_path)?;
        Self::open(config_path)
    }

    pub fn open(config_path: impl AsRef<Path>) -> Result<Self> {
        let config_path = absolute_lexical(config_path.as_ref())?;
        let config = Config::load(&config_path)?;
        let database_path = config.database_path(&config_path);
        if let Some(parent) = database_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(database_path)?;
        configure_connection(&connection)?;
        migrate(&connection)?;
        let mut library = Self {
            config_path,
            config,
            connection,
        };
        library.recover_remote_imports()?;
        library.recover_pdf_replacements()?;
        Ok(library)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn root_path(&self) -> PathBuf {
        self.config.root_path(&self.config_path)
    }

    pub fn set_root(&mut self, root: PathBuf) -> Result<()> {
        if root.as_os_str().is_empty() {
            return Err(LitmanError::InvalidConfig(
                "library root cannot be empty".into(),
            ));
        }
        self.config.library_root = root;
        self.config.save(&self.config_path)
    }

    pub fn set_language(&mut self, language: Language) -> Result<()> {
        self.config.language = language;
        self.config.save(&self.config_path)
    }

    pub fn set_scixplorer_api_token(&mut self, token: Option<String>) -> Result<()> {
        let mut config = self.config.clone();
        config.scixplorer_api_token = token.map(|token| token.trim().to_owned());
        config.save(&self.config_path)?;
        self.config = config;
        Ok(())
    }

    pub fn scixplorer_client(&self) -> Result<ScixplorerClient> {
        let token = self
            .config
            .scixplorer_api_token
            .as_deref()
            .ok_or(LitmanError::MissingScixplorerToken)?;
        ScixplorerClient::new(token)
    }

    pub fn list_papers(&self, filter: &ListFilter) -> Result<Vec<Paper>> {
        validate_importance(filter.importance)?;
        validate_importance(filter.min_importance)?;

        let mut sql = format!("SELECT {PAPER_COLUMNS} FROM papers p WHERE 1=1");
        let mut values = Vec::<Value>::new();

        if let Some(status) = filter.status {
            sql.push_str(" AND p.file_status = ?");
            values.push(status.as_str().to_owned().into());
        }
        if let Some(importance) = filter.importance {
            sql.push_str(" AND p.importance = ?");
            values.push(i64::from(importance).into());
        }
        if let Some(minimum) = filter.min_importance {
            sql.push_str(" AND p.importance >= ?");
            values.push(i64::from(minimum).into());
        }
        if filter.unrated {
            sql.push_str(" AND p.importance IS NULL");
        }
        if let Some(path) = filter.group_path.as_deref() {
            let group_id = self.resolve_group_path(path)?;
            sql.push_str(
                " AND EXISTS (
                    WITH RECURSIVE descendants(id) AS (
                        SELECT ? UNION ALL
                        SELECT g.id FROM groups g JOIN descendants d ON g.parent_id = d.id
                    )
                    SELECT 1 FROM paper_groups pg
                    WHERE pg.paper_id = p.id AND pg.group_id IN descendants
                )",
            );
            values.push(group_id.into());
        }
        if let Some(query) = filter.query.as_deref() {
            for term in search_terms(query) {
                sql.push_str(" AND instr(p.search_text, ?) > 0");
                values.push(term.into());
            }
        }
        sql.push_str(" ORDER BY COALESCE(p.importance, 0) DESC, COALESCE(p.title, p.relative_path) COLLATE NOCASE");
        if let Some(limit) = filter.limit {
            sql.push_str(" LIMIT ?");
            values.push((limit as i64).into());
        }

        let mut statement = self.connection.prepare(&sql)?;
        let papers = statement
            .query_map(params_from_iter(values.iter()), row_to_paper)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(papers)
    }

    pub fn get_paper(&self, id_or_prefix: &str) -> Result<Paper> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {PAPER_COLUMNS} FROM papers WHERE id = ?1 OR id LIKE ?2 ORDER BY id LIMIT 2"
        ))?;
        let prefix = format!("{}%", id_or_prefix.trim());
        let mut papers = statement
            .query_map(params![id_or_prefix.trim(), prefix], row_to_paper)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if papers.is_empty() {
            return Err(LitmanError::PaperNotFound(id_or_prefix.into()));
        }
        if papers.len() > 1 && !papers.iter().any(|paper| paper.id == id_or_prefix) {
            return Err(LitmanError::AmbiguousPaperId(id_or_prefix.into()));
        }
        if let Some(position) = papers.iter().position(|paper| paper.id == id_or_prefix) {
            Ok(papers.swap_remove(position))
        } else {
            Ok(papers.remove(0))
        }
    }

    pub fn update_paper(&mut self, id_or_prefix: &str, update: PaperUpdate) -> Result<Paper> {
        let mut paper = self.get_paper(id_or_prefix)?;
        apply_update(&mut paper, update);
        paper.updated_at = now();
        self.save_metadata(&paper)?;
        self.rebuild_search_text(&paper.id)?;
        self.get_paper(&paper.id)
    }

    pub fn set_importance(&mut self, id_or_prefix: &str, importance: Option<u8>) -> Result<()> {
        validate_importance(importance)?;
        let paper = self.get_paper(id_or_prefix)?;
        self.connection.execute(
            "UPDATE papers SET importance = ?1, updated_at = ?2 WHERE id = ?3",
            params![importance.map(i64::from), now(), paper.id],
        )?;
        self.rebuild_search_text(&paper.id)
    }

    pub fn reset_field(&mut self, id_or_prefix: &str, field: &str) -> Result<Paper> {
        let mut paper = self.get_paper(id_or_prefix)?;
        if !reset_from_embedded(&mut paper, field) {
            return Err(LitmanError::InvalidField(field.into()));
        }
        paper.manual_overrides.remove(field);
        paper.bibtex_fields.remove(field);
        paper.arxiv_fields.remove(field);
        paper.updated_at = now();
        self.save_metadata(&paper)?;
        self.rebuild_search_text(&paper.id)?;
        self.get_paper(&paper.id)
    }

    pub fn store_bibtex(&mut self, id_or_prefix: &str, bibtex: &str) -> Result<Paper> {
        let metadata = parse_bibtex(bibtex)?;
        let bibcode = metadata.bibcode.clone();
        let mut paper = self.get_paper(id_or_prefix)?;
        let populated = metadata.populated_fields();
        let previous_fields = paper.bibtex_fields.clone();
        let mut applied_fields = BTreeSet::new();

        for field in previous_fields.clone() {
            if !populated.contains(&field) {
                reset_from_embedded(&mut paper, &field);
                paper.manual_overrides.remove(&field);
            }
        }

        macro_rules! import_scalar {
            ($name:literal, $field:ident) => {
                if let Some(value) = metadata.$field {
                    if !paper.manual_overrides.contains($name) || previous_fields.contains($name) {
                        paper.$field = Some(value);
                        paper.manual_overrides.remove($name);
                        paper.arxiv_fields.remove($name);
                        applied_fields.insert($name.into());
                    }
                }
            };
        }
        import_scalar!("title", title);
        import_scalar!("abstract_text", abstract_text);
        import_scalar!("publication_date", publication_date);
        import_scalar!("container_title", container_title);
        import_scalar!("volume", volume);
        import_scalar!("issue", issue);
        import_scalar!("pages", pages);
        import_scalar!("doi", doi);
        import_scalar!("url", url);
        import_scalar!("language", language);
        if let Some(authors) = metadata.authors
            && (!paper.manual_overrides.contains("authors") || previous_fields.contains("authors"))
        {
            paper.authors = authors;
            paper.manual_overrides.remove("authors");
            paper.arxiv_fields.remove("authors");
            applied_fields.insert("authors".into());
        }
        if let Some(keywords) = metadata.keywords
            && (!paper.manual_overrides.contains("keywords")
                || previous_fields.contains("keywords"))
        {
            paper.keywords = keywords;
            paper.manual_overrides.remove("keywords");
            paper.arxiv_fields.remove("keywords");
            applied_fields.insert("keywords".into());
        }
        paper.bibtex = Some(bibtex.to_owned());
        paper.bibcode = Some(bibcode);
        paper.bibtex_fields = applied_fields;
        paper.updated_at = now();
        self.save_metadata(&paper)?;
        self.rebuild_search_text(&paper.id)?;
        self.get_paper(&paper.id)
    }

    pub(crate) fn store_arxiv_metadata(
        &mut self,
        id_or_prefix: &str,
        metadata: ArxivMetadata,
        raw_atom: &str,
    ) -> Result<Paper> {
        let mut paper = self.get_paper(id_or_prefix)?;
        let populated = metadata.populated_fields();
        let previous_fields = paper.arxiv_fields.clone();
        let mut applied_fields = BTreeSet::new();

        for field in previous_fields.clone() {
            if !populated.contains(&field) {
                reset_from_embedded(&mut paper, &field);
                paper.manual_overrides.remove(&field);
            }
        }

        macro_rules! import_scalar {
            ($name:literal, $field:ident) => {
                if let Some(value) = metadata.$field {
                    if !paper.manual_overrides.contains($name) || previous_fields.contains($name) {
                        paper.$field = Some(value);
                        paper.manual_overrides.remove($name);
                        paper.bibtex_fields.remove($name);
                        applied_fields.insert($name.into());
                    }
                }
            };
        }
        import_scalar!("title", title);
        import_scalar!("abstract_text", abstract_text);
        import_scalar!("publication_date", publication_date);
        import_scalar!("container_title", container_title);
        import_scalar!("doi", doi);
        import_scalar!("url", url);
        if let Some(authors) = metadata.authors
            && (!paper.manual_overrides.contains("authors") || previous_fields.contains("authors"))
        {
            paper.authors = authors;
            paper.manual_overrides.remove("authors");
            paper.bibtex_fields.remove("authors");
            applied_fields.insert("authors".into());
        }
        if let Some(keywords) = metadata.keywords
            && (!paper.manual_overrides.contains("keywords")
                || previous_fields.contains("keywords"))
        {
            paper.keywords = keywords;
            paper.manual_overrides.remove("keywords");
            paper.bibtex_fields.remove("keywords");
            applied_fields.insert("keywords".into());
        }
        paper.arxiv_id = Some(metadata.arxiv_id);
        paper.arxiv_atom_xml = Some(raw_atom.to_owned());
        paper.arxiv_fields = applied_fields;
        paper.updated_at = now();
        self.save_metadata(&paper)?;
        self.rebuild_search_text(&paper.id)?;
        self.get_paper(&paper.id)
    }

    pub fn paper_bibtex(&self, id_or_prefix: &str) -> Result<String> {
        let paper = self.get_paper(id_or_prefix)?;
        paper
            .bibtex
            .ok_or_else(|| LitmanError::BibtexNotFound(paper.id))
    }

    pub fn open_scixplorer(&self, id_or_prefix: &str) -> Result<()> {
        let paper = self.get_paper(id_or_prefix)?;
        let bibcode = paper
            .bibcode
            .ok_or_else(|| LitmanError::BibtexNotFound(paper.id.clone()))?;
        let url = scixplorer_url(&bibcode)?;
        open::that_detached(url).map_err(|error| {
            LitmanError::Io(std::io::Error::other(format!(
                "cannot open SciXplorer: {error}"
            )))
        })?;
        Ok(())
    }

    pub fn remove_paper(&mut self, id_or_prefix: &str) -> Result<()> {
        let paper = self.get_paper(id_or_prefix)?;
        self.connection
            .execute("DELETE FROM papers WHERE id = ?1", params![paper.id])?;
        Ok(())
    }

    pub fn open_pdf(&self, id_or_prefix: &str) -> Result<()> {
        let paper = self.get_paper(id_or_prefix)?;
        let root = self
            .root_path()
            .canonicalize()
            .map_err(|_| LitmanError::RootUnavailable(self.root_path()))?;
        let path = root.join(path_from_database(&paper.relative_path));
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(&root) {
            return Err(LitmanError::InvalidConfig(
                "paper path escapes the configured root".into(),
            ));
        }
        open::that_detached(canonical).map_err(|error| {
            LitmanError::Io(std::io::Error::other(format!("cannot open PDF: {error}")))
        })?;
        Ok(())
    }

    pub fn backup(&self, destination: impl AsRef<Path>) -> Result<PathBuf> {
        let destination = destination.as_ref();
        fs::create_dir_all(destination)?;
        let database_path = destination.join(&self.config.database);
        let config_path = destination.join(
            self.config_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("library.toml")),
        );
        if database_path.exists() || config_path.exists() {
            return Err(LitmanError::InvalidConfig(format!(
                "backup destination already contains a library: {}",
                destination.display()
            )));
        }
        let mut target = Connection::open(&database_path)?;
        let backup = Backup::new(&self.connection, &mut target)?;
        backup.run_to_completion(64, Duration::from_millis(10), None)?;
        drop(backup);
        let mut config = self.config.clone();
        config.library_root = self
            .root_path()
            .canonicalize()
            .unwrap_or_else(|_| self.root_path());
        config.save(&config_path)?;
        Ok(config_path)
    }

    pub fn list_groups(&self) -> Result<Vec<Group>> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, parent_id FROM groups ORDER BY parent_id, name COLLATE NOCASE",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(Group {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    parent_id: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn group_exists(&self, path: &str) -> Result<bool> {
        match self.resolve_group_path(path) {
            Ok(_) => Ok(true),
            Err(LitmanError::GroupNotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn create_group(&mut self, path: &str) -> Result<Group> {
        let components = group_components(path)?;
        let transaction = self.connection.transaction()?;
        let mut parent_id = None;
        let mut current_id = 0;
        for name in components {
            let normalized = normalize(&name);
            let existing = transaction
                .query_row(
                    "SELECT id FROM groups WHERE normalized_name = ?1 AND parent_key = ?2",
                    params![normalized, parent_id.unwrap_or(0)],
                    |row| row.get(0),
                )
                .optional()?;
            current_id = if let Some(id) = existing {
                id
            } else {
                transaction.execute(
                    "INSERT INTO groups(name, normalized_name, parent_id, parent_key, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![name, normalized, parent_id, parent_id.unwrap_or(0), now()],
                )?;
                transaction.last_insert_rowid()
            };
            parent_id = Some(current_id);
        }
        transaction.commit()?;
        self.group_by_id(current_id)
    }

    pub fn rename_group(&mut self, path: &str, new_name: &str) -> Result<Group> {
        if new_name.trim().is_empty() || new_name.contains('/') {
            return Err(LitmanError::InvalidConfig("invalid group name".into()));
        }
        let id = self.resolve_group_path(path)?;
        let changed = self.connection.execute(
            "UPDATE groups SET name = ?1, normalized_name = ?2 WHERE id = ?3",
            params![new_name.trim(), normalize(new_name), id],
        );
        match changed {
            Ok(_) => {
                self.rebuild_search_for_group(id)?;
                self.group_by_id(id)
            }
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(LitmanError::DuplicateGroup)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn move_group(&mut self, path: &str, new_parent: Option<&str>) -> Result<Group> {
        let id = self.resolve_group_path(path)?;
        let parent_id = new_parent
            .map(|path| self.resolve_group_path(path))
            .transpose()?;
        let below_itself = parent_id
            .map(|candidate| self.is_descendant(candidate, id))
            .transpose()?
            .unwrap_or(false);
        if parent_id == Some(id) || below_itself {
            return Err(LitmanError::InvalidConfig(
                "a group cannot be moved below itself".into(),
            ));
        }
        let result = self.connection.execute(
            "UPDATE groups SET parent_id = ?1, parent_key = ?2 WHERE id = ?3",
            params![parent_id, parent_id.unwrap_or(0), id],
        );
        match result {
            Ok(_) => {
                self.rebuild_search_for_group(id)?;
                self.group_by_id(id)
            }
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(LitmanError::DuplicateGroup)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn delete_group(&mut self, path: &str) -> Result<()> {
        let id = self.resolve_group_path(path)?;
        let paper_ids = self.paper_ids_for_group_tree(id)?;
        self.connection
            .execute("DELETE FROM groups WHERE id = ?1", params![id])?;
        for paper_id in paper_ids {
            self.rebuild_search_text(&paper_id)?;
        }
        Ok(())
    }

    pub fn add_to_group(&mut self, group_path: &str, paper_ids: &[String]) -> Result<()> {
        let group_id = self.resolve_group_path(group_path)?;
        let resolved = paper_ids
            .iter()
            .map(|id| self.get_paper(id).map(|paper| paper.id))
            .collect::<Result<Vec<_>>>()?;
        let transaction = self.connection.transaction()?;
        for paper_id in &resolved {
            transaction.execute(
                "INSERT OR IGNORE INTO paper_groups(paper_id, group_id) VALUES (?1, ?2)",
                params![paper_id, group_id],
            )?;
        }
        transaction.commit()?;
        for paper_id in resolved {
            self.rebuild_search_text(&paper_id)?;
        }
        Ok(())
    }

    pub fn remove_from_group(&mut self, group_path: &str, paper_ids: &[String]) -> Result<()> {
        let group_id = self.resolve_group_path(group_path)?;
        let resolved = paper_ids
            .iter()
            .map(|id| self.get_paper(id).map(|paper| paper.id))
            .collect::<Result<Vec<_>>>()?;
        let transaction = self.connection.transaction()?;
        for paper_id in &resolved {
            transaction.execute(
                "DELETE FROM paper_groups WHERE paper_id = ?1 AND group_id = ?2",
                params![paper_id, group_id],
            )?;
        }
        transaction.commit()?;
        for paper_id in resolved {
            self.rebuild_search_text(&paper_id)?;
        }
        Ok(())
    }

    pub fn groups_for_paper(&self, id_or_prefix: &str) -> Result<Vec<Group>> {
        let paper = self.get_paper(id_or_prefix)?;
        let mut statement = self.connection.prepare(
            "SELECT g.id, g.name, g.parent_id FROM groups g
             JOIN paper_groups pg ON pg.group_id = g.id
             WHERE pg.paper_id = ?1 ORDER BY g.name COLLATE NOCASE",
        )?;
        Ok(statement
            .query_map(params![paper.id], |row| {
                Ok(Group {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    parent_id: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn group_path(&self, id: i64) -> Result<String> {
        let mut components = Vec::new();
        let mut cursor = Some(id);
        while let Some(group_id) = cursor {
            let group = self.group_by_id(group_id)?;
            components.push(group.name);
            cursor = group.parent_id;
        }
        components.reverse();
        Ok(components.join("/"))
    }

    pub(crate) fn paper_by_path(&self, relative_path: &str) -> Result<Option<Paper>> {
        self.connection
            .query_row(
                &format!("SELECT {PAPER_COLUMNS} FROM papers WHERE relative_path = ?1"),
                params![relative_path],
                row_to_paper,
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn mark_missing_not_in(&mut self, discovered: &HashSet<String>) -> Result<usize> {
        let mut statement = self
            .connection
            .prepare("SELECT id, relative_path FROM papers WHERE file_status != 'missing'")?;
        let candidates = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut count = 0;
        for (id, path) in candidates {
            if !discovered.contains(&path) {
                self.connection.execute(
                    "UPDATE papers SET file_status = 'missing', updated_at = ?1 WHERE id = ?2",
                    params![now(), id],
                )?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub(crate) fn missing_ids_by_hash(&self, hash: &str) -> Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            "SELECT id FROM papers WHERE content_hash = ?1 AND file_status = 'missing' ORDER BY id",
        )?;
        Ok(statement
            .query_map(params![hash], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn present_id_by_hash(
        &self,
        hash: &str,
        excluding: Option<&str>,
    ) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT id FROM papers WHERE content_hash = ?1 AND file_status != 'missing'
                 AND (?2 IS NULL OR id != ?2) ORDER BY id LIMIT 1",
                params![hash, excluding],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn insert_scanned(&mut self, scan: ScannedData<'_>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        let status = if scan.scan_error.is_some() {
            "error"
        } else {
            "present"
        };
        let default_embedded = EmbeddedMetadata::default();
        let embedded = scan.embedded.unwrap_or(&default_embedded);
        self.connection.execute(
            "INSERT INTO papers(
                id, relative_path, file_size, modified_unix_ms, content_hash, file_status,
                scan_error, duplicate_of, title, authors_json, abstract_text, publication_date,
                container_title, volume, issue, pages, doi, url, language, keywords_json,
                page_count, pdf_version, encrypted, creator, producer, embedded_json,
                manual_overrides_json, search_text, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
                '[]', '', ?27, ?27
             )",
            params![
                id,
                scan.relative_path,
                scan.file_size as i64,
                scan.modified_unix_ms,
                scan.content_hash,
                status,
                scan.scan_error,
                scan.duplicate_of,
                embedded.title,
                serde_json::to_string(&embedded.authors)?,
                embedded.abstract_text,
                embedded.publication_date,
                embedded.container_title,
                embedded.volume,
                embedded.issue,
                embedded.pages,
                embedded.doi,
                embedded.url,
                embedded.language,
                serde_json::to_string(&embedded.keywords)?,
                embedded.page_count.map(i64::from),
                embedded.pdf_version,
                embedded.encrypted,
                embedded.creator,
                embedded.producer,
                serde_json::to_string(embedded)?,
                timestamp,
            ],
        )?;
        self.rebuild_search_text(&id)?;
        Ok(id)
    }

    pub(crate) fn update_scanned(&mut self, id: &str, scan: ScannedData<'_>) -> Result<()> {
        let mut paper = self.get_paper(id)?;
        paper.relative_path = scan.relative_path.into();
        paper.file_size = scan.file_size;
        paper.modified_unix_ms = scan.modified_unix_ms;
        paper.content_hash = scan.content_hash.into();
        paper.file_status = if scan.scan_error.is_some() {
            FileStatus::Error
        } else {
            FileStatus::Present
        };
        paper.scan_error = scan.scan_error.map(ToOwned::to_owned);
        paper.duplicate_of = scan.duplicate_of.map(ToOwned::to_owned);
        if let Some(embedded) = scan.embedded {
            paper.embedded = embedded.clone();
            apply_embedded_to_unmodified(&mut paper);
        }
        paper.updated_at = now();
        self.connection.execute(
            "UPDATE papers SET relative_path=?1, file_size=?2, modified_unix_ms=?3,
             content_hash=?4, file_status=?5, scan_error=?6, duplicate_of=?7,
             page_count=?8, pdf_version=?9, encrypted=?10, creator=?11, producer=?12,
             embedded_json=?13, updated_at=?14 WHERE id=?15",
            params![
                paper.relative_path,
                paper.file_size as i64,
                paper.modified_unix_ms,
                paper.content_hash,
                paper.file_status.as_str(),
                paper.scan_error,
                paper.duplicate_of,
                paper.embedded.page_count.map(i64::from),
                paper.embedded.pdf_version,
                paper.embedded.encrypted,
                paper.embedded.creator,
                paper.embedded.producer,
                serde_json::to_string(&paper.embedded)?,
                paper.updated_at,
                paper.id,
            ],
        )?;
        self.save_metadata(&paper)?;
        self.rebuild_search_text(&paper.id)
    }

    pub(crate) fn save_metadata(&self, paper: &Paper) -> Result<()> {
        self.connection.execute(
            "UPDATE papers SET title=?1, authors_json=?2, abstract_text=?3,
             publication_date=?4, container_title=?5, volume=?6, issue=?7, pages=?8,
             doi=?9, url=?10, language=?11, keywords_json=?12, notes=?13,
             manual_overrides_json=?14, updated_at=?15, bibtex=?16, bibcode=?17,
             bibtex_fields_json=?18, arxiv_id=?19, arxiv_atom_xml=?20,
             arxiv_fields_json=?21 WHERE id=?22",
            params![
                paper.title,
                serde_json::to_string(&paper.authors)?,
                paper.abstract_text,
                paper.publication_date,
                paper.container_title,
                paper.volume,
                paper.issue,
                paper.pages,
                paper.doi,
                paper.url,
                paper.language,
                serde_json::to_string(&paper.keywords)?,
                paper.notes,
                serde_json::to_string(&paper.manual_overrides)?,
                paper.updated_at,
                paper.bibtex,
                paper.bibcode,
                serde_json::to_string(&paper.bibtex_fields)?,
                paper.arxiv_id,
                paper.arxiv_atom_xml,
                serde_json::to_string(&paper.arxiv_fields)?,
                paper.id,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn rebuild_search_text(&self, paper_id: &str) -> Result<()> {
        let paper = self.get_paper(paper_id)?;
        let group_names = self
            .groups_for_paper(paper_id)?
            .into_iter()
            .map(|group| self.group_path(group.id))
            .collect::<Result<Vec<_>>>()?;
        let pieces = [
            vec![paper.relative_path],
            paper.title.into_iter().collect(),
            paper.authors,
            paper.abstract_text.into_iter().collect(),
            paper.publication_date.into_iter().collect(),
            paper.container_title.into_iter().collect(),
            paper.volume.into_iter().collect(),
            paper.issue.into_iter().collect(),
            paper.pages.into_iter().collect(),
            paper.doi.into_iter().collect(),
            paper.url.into_iter().collect(),
            paper.language.into_iter().collect(),
            paper.keywords,
            paper.notes.into_iter().collect(),
            paper.bibcode.into_iter().collect(),
            paper.arxiv_id.into_iter().collect(),
            paper.creator.into_iter().collect(),
            paper.producer.into_iter().collect(),
            group_names,
        ]
        .concat()
        .join("\n");
        self.connection.execute(
            "UPDATE papers SET search_text = ?1 WHERE id = ?2",
            params![normalize(&pieces), paper_id],
        )?;
        Ok(())
    }

    fn resolve_group_path(&self, path: &str) -> Result<i64> {
        let components = group_components(path)?;
        let mut parent_id = None;
        for name in components {
            let id = self
                .connection
                .query_row(
                    "SELECT id FROM groups WHERE normalized_name = ?1 AND parent_key = ?2",
                    params![normalize(&name), parent_id.unwrap_or(0)],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| LitmanError::GroupNotFound(path.into()))?;
            parent_id = Some(id);
        }
        parent_id.ok_or_else(|| LitmanError::GroupNotFound(path.into()))
    }

    fn group_by_id(&self, id: i64) -> Result<Group> {
        self.connection
            .query_row(
                "SELECT id, name, parent_id FROM groups WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Group {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        parent_id: row.get(2)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| LitmanError::GroupNotFound(id.to_string()))
    }

    fn is_descendant(&self, candidate: i64, ancestor: i64) -> Result<bool> {
        let found: Option<i64> = self
            .connection
            .query_row(
                "WITH RECURSIVE descendants(id) AS (
                    SELECT id FROM groups WHERE parent_id = ?1
                    UNION ALL SELECT g.id FROM groups g JOIN descendants d ON g.parent_id = d.id
                 ) SELECT id FROM descendants WHERE id = ?2 LIMIT 1",
                params![ancestor, candidate],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    fn paper_ids_for_group_tree(&self, id: i64) -> Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE descendants(id) AS (
                SELECT ?1 UNION ALL SELECT g.id FROM groups g JOIN descendants d ON g.parent_id=d.id
             ) SELECT DISTINCT paper_id FROM paper_groups WHERE group_id IN descendants",
        )?;
        Ok(statement
            .query_map(params![id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn rebuild_search_for_group(&self, id: i64) -> Result<()> {
        for paper_id in self.paper_ids_for_group_tree(id)? {
            self.rebuild_search_text(&paper_id)?;
        }
        Ok(())
    }
}

fn configure_connection(connection: &Connection) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}

fn migrate(connection: &Connection) -> Result<()> {
    let mut version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > DATABASE_SCHEMA_VERSION {
        return Err(LitmanError::InvalidConfig(format!(
            "database schema {version} is newer than this LitMan build"
        )));
    }
    if version == 0 {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(
            "CREATE TABLE papers(
                id TEXT PRIMARY KEY,
                relative_path TEXT NOT NULL UNIQUE,
                file_size INTEGER NOT NULL DEFAULT 0,
                modified_unix_ms INTEGER NOT NULL DEFAULT 0,
                content_hash TEXT NOT NULL DEFAULT '',
                file_status TEXT NOT NULL DEFAULT 'present' CHECK(file_status IN ('present','missing','error')),
                scan_error TEXT,
                duplicate_of TEXT REFERENCES papers(id) ON DELETE SET NULL,
                title TEXT,
                authors_json TEXT NOT NULL DEFAULT '[]',
                abstract_text TEXT,
                publication_date TEXT,
                container_title TEXT,
                volume TEXT,
                issue TEXT,
                pages TEXT,
                doi TEXT,
                url TEXT,
                language TEXT,
                keywords_json TEXT NOT NULL DEFAULT '[]',
                notes TEXT,
                importance INTEGER CHECK(importance BETWEEN 1 AND 5),
                page_count INTEGER,
                pdf_version TEXT,
                encrypted INTEGER NOT NULL DEFAULT 0,
                creator TEXT,
                producer TEXT,
                embedded_json TEXT NOT NULL DEFAULT '{}',
                manual_overrides_json TEXT NOT NULL DEFAULT '[]',
                search_text TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                bibtex TEXT,
                bibcode TEXT,
                 bibtex_fields_json TEXT NOT NULL DEFAULT '[]',
                 arxiv_id TEXT,
                 arxiv_atom_xml TEXT,
                 arxiv_fields_json TEXT NOT NULL DEFAULT '[]'
             );
             CREATE INDEX papers_hash_idx ON papers(content_hash);
             CREATE INDEX papers_status_idx ON papers(file_status);
             CREATE INDEX papers_importance_idx ON papers(importance);
             CREATE TABLE groups(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                normalized_name TEXT NOT NULL,
                parent_id INTEGER REFERENCES groups(id) ON DELETE CASCADE,
                parent_key INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                UNIQUE(parent_key, normalized_name)
             );
             CREATE TABLE paper_groups(
                paper_id TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
                group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
                PRIMARY KEY(paper_id, group_id)
             );
             PRAGMA user_version = 3;",
        )?;
        transaction.commit()?;
        return Ok(());
    }
    if version == 1 {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(
            "ALTER TABLE papers ADD COLUMN bibtex TEXT;
             ALTER TABLE papers ADD COLUMN bibcode TEXT;
             ALTER TABLE papers ADD COLUMN bibtex_fields_json TEXT NOT NULL DEFAULT '[]';
             PRAGMA user_version = 2;",
        )?;
        transaction.commit()?;
        version = 2;
    }
    if version == 2 {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(
            "ALTER TABLE papers ADD COLUMN arxiv_id TEXT;
             ALTER TABLE papers ADD COLUMN arxiv_atom_xml TEXT;
             ALTER TABLE papers ADD COLUMN arxiv_fields_json TEXT NOT NULL DEFAULT '[]';
             PRAGMA user_version = 3;",
        )?;
        normalize_legacy_bibtex_overrides(&transaction)?;
        transaction.commit()?;
    }
    Ok(())
}

fn normalize_legacy_bibtex_overrides(connection: &Connection) -> Result<()> {
    let columns = {
        let mut statement = connection.prepare("PRAGMA table_info(papers)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<BTreeSet<_>, _>>()?
    };
    if !columns.contains("manual_overrides_json") || !columns.contains("bibtex_fields_json") {
        return Ok(());
    }
    let rows = {
        let mut statement = connection
            .prepare("SELECT id, manual_overrides_json, bibtex_fields_json FROM papers")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, manual_json, bibtex_json) in rows {
        let mut manual: BTreeSet<String> = serde_json::from_str(&manual_json).unwrap_or_default();
        let bibtex: BTreeSet<String> = serde_json::from_str(&bibtex_json).unwrap_or_default();
        manual.retain(|field| !bibtex.contains(field));
        connection.execute(
            "UPDATE papers SET manual_overrides_json = ?1 WHERE id = ?2",
            params![serde_json::to_string(&manual)?, id],
        )?;
    }
    Ok(())
}

fn row_to_paper(row: &Row<'_>) -> rusqlite::Result<Paper> {
    let authors_json: String = row.get(9)?;
    let keywords_json: String = row.get(19)?;
    let embedded_json: String = row.get(27)?;
    let overrides_json: String = row.get(28)?;
    Ok(Paper {
        id: row.get(0)?,
        relative_path: row.get(1)?,
        file_size: row.get::<_, i64>(2)?.max(0) as u64,
        modified_unix_ms: row.get(3)?,
        content_hash: row.get(4)?,
        file_status: FileStatus::parse(&row.get::<_, String>(5)?),
        scan_error: row.get(6)?,
        duplicate_of: row.get(7)?,
        title: row.get(8)?,
        authors: serde_json::from_str(&authors_json).unwrap_or_default(),
        abstract_text: row.get(10)?,
        publication_date: row.get(11)?,
        container_title: row.get(12)?,
        volume: row.get(13)?,
        issue: row.get(14)?,
        pages: row.get(15)?,
        doi: row.get(16)?,
        url: row.get(17)?,
        language: row.get(18)?,
        keywords: serde_json::from_str(&keywords_json).unwrap_or_default(),
        notes: row.get(20)?,
        bibtex: row.get(31)?,
        bibcode: row.get(32)?,
        bibtex_fields: serde_json::from_str(&row.get::<_, String>(33)?).unwrap_or_default(),
        arxiv_id: row.get(34)?,
        arxiv_atom_xml: row.get(35)?,
        arxiv_fields: serde_json::from_str(&row.get::<_, String>(36)?).unwrap_or_default(),
        importance: row.get::<_, Option<i64>>(21)?.map(|value| value as u8),
        page_count: row.get::<_, Option<i64>>(22)?.map(|value| value as u32),
        pdf_version: row.get(23)?,
        encrypted: row.get(24)?,
        creator: row.get(25)?,
        producer: row.get(26)?,
        embedded: serde_json::from_str(&embedded_json).unwrap_or_default(),
        manual_overrides: serde_json::from_str(&overrides_json).unwrap_or_default(),
        created_at: row.get(29)?,
        updated_at: row.get(30)?,
    })
}

fn apply_update(paper: &mut Paper, update: PaperUpdate) {
    macro_rules! scalar {
        ($field:ident) => {
            if let Some(value) = update.$field {
                paper.$field = value.map(clean);
                paper.manual_overrides.insert(stringify!($field).into());
                paper.bibtex_fields.remove(stringify!($field));
                paper.arxiv_fields.remove(stringify!($field));
            }
        };
    }
    scalar!(title);
    scalar!(abstract_text);
    scalar!(publication_date);
    scalar!(container_title);
    scalar!(volume);
    scalar!(issue);
    scalar!(pages);
    scalar!(doi);
    scalar!(url);
    scalar!(language);
    scalar!(notes);
    if let Some(authors) = update.authors {
        paper.authors = clean_list(authors);
        paper.manual_overrides.insert("authors".into());
        paper.bibtex_fields.remove("authors");
        paper.arxiv_fields.remove("authors");
    }
    if let Some(keywords) = update.keywords {
        paper.keywords = clean_list(keywords);
        paper.manual_overrides.insert("keywords".into());
        paper.bibtex_fields.remove("keywords");
        paper.arxiv_fields.remove("keywords");
    }
}

fn apply_embedded_to_unmodified(paper: &mut Paper) {
    macro_rules! copy {
        ($field:ident) => {
            if !paper.manual_overrides.contains(stringify!($field))
                && !paper.bibtex_fields.contains(stringify!($field))
                && !paper.arxiv_fields.contains(stringify!($field))
            {
                paper.$field = paper.embedded.$field.clone();
            }
        };
    }
    copy!(title);
    copy!(authors);
    copy!(abstract_text);
    copy!(publication_date);
    copy!(container_title);
    copy!(volume);
    copy!(issue);
    copy!(pages);
    copy!(doi);
    copy!(url);
    copy!(language);
    copy!(keywords);
    paper.page_count = paper.embedded.page_count;
    paper.pdf_version = paper.embedded.pdf_version.clone();
    paper.encrypted = paper.embedded.encrypted;
    paper.creator = paper.embedded.creator.clone();
    paper.producer = paper.embedded.producer.clone();
}

fn reset_from_embedded(paper: &mut Paper, field: &str) -> bool {
    macro_rules! reset {
        ($name:literal, $field:ident) => {
            if field == $name {
                paper.$field = paper.embedded.$field.clone();
                return true;
            }
        };
    }
    reset!("title", title);
    reset!("authors", authors);
    reset!("abstract_text", abstract_text);
    reset!("publication_date", publication_date);
    reset!("container_title", container_title);
    reset!("volume", volume);
    reset!("issue", issue);
    reset!("pages", pages);
    reset!("doi", doi);
    reset!("url", url);
    reset!("language", language);
    reset!("keywords", keywords);
    false
}

fn validate_importance(value: Option<u8>) -> Result<()> {
    if value.is_some_and(|value| !(1..=5).contains(&value)) {
        Err(LitmanError::InvalidImportance)
    } else {
        Ok(())
    }
}

fn group_components(path: &str) -> Result<Vec<String>> {
    let components = path
        .split('/')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if components.is_empty() || components.iter().any(|value| value == "." || value == "..") {
        return Err(LitmanError::InvalidConfig("invalid group path".into()));
    }
    Ok(components)
}

fn clean(value: String) -> String {
    value.trim().to_owned()
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .map(clean)
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(normalize(value)))
        .collect()
}

fn normalize(value: &str) -> String {
    value.nfkc().flat_map(char::to_lowercase).collect()
}

fn search_terms(query: &str) -> Vec<String> {
    normalize(query)
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn absolute_lexical(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn path_from_database(path: &str) -> PathBuf {
    path.split('/').collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn library() -> (TempDir, Library) {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir_all(&root).unwrap();
        let config_path = temporary.path().join("library.toml");
        let library = Library::init(&config_path, Config::new(root)).unwrap();
        (temporary, library)
    }

    #[test]
    fn nested_groups_are_created_and_resolved() {
        let (_temporary, mut library) = library();
        let group = library.create_group("Research/机器学习").unwrap();
        assert_eq!(library.group_path(group.id).unwrap(), "Research/机器学习");
        assert!(library.group_exists("research/机器学习").unwrap());
        assert!(!library.group_exists("Research/不存在").unwrap());
        assert_eq!(
            library.create_group("research/机器学习").unwrap().id,
            group.id
        );
    }

    #[test]
    fn group_rename_rejects_an_existing_sibling_name() {
        let (_temporary, mut library) = library();
        library.create_group("Research/Imaging").unwrap();
        library.create_group("Research/Astrometry").unwrap();

        assert!(matches!(
            library.rename_group("Research/Imaging", "ASTROMETRY"),
            Err(LitmanError::DuplicateGroup)
        ));
        let renamed = library
            .rename_group("Research/Imaging", "Calibration")
            .unwrap();
        assert_eq!(
            library.group_path(renamed.id).unwrap(),
            "Research/Calibration"
        );
    }

    #[test]
    fn importance_is_validated() {
        assert!(validate_importance(Some(1)).is_ok());
        assert!(validate_importance(Some(5)).is_ok());
        assert!(validate_importance(Some(0)).is_err());
    }

    #[test]
    fn migration_configures_a_portable_constrained_database() {
        let (_temporary, library) = library();
        let version: i64 = library
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let foreign_keys: i64 = library
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        let journal: String = library
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(version, DATABASE_SCHEMA_VERSION);
        assert_eq!(foreign_keys, 1);
        assert_eq!(journal.to_ascii_lowercase(), "delete");
    }

    #[test]
    fn schema_one_is_migrated_for_bibtex_without_losing_compatibility() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE papers(id TEXT); PRAGMA user_version = 1;")
            .unwrap();
        migrate(&connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let mut statement = connection.prepare("PRAGMA table_info(papers)").unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(version, DATABASE_SCHEMA_VERSION);
        assert!(columns.contains(&"bibtex".into()));
        assert!(columns.contains(&"bibcode".into()));
        assert!(columns.contains(&"bibtex_fields_json".into()));
        assert!(columns.contains(&"arxiv_id".into()));
        assert!(columns.contains(&"arxiv_atom_xml".into()));
        assert!(columns.contains(&"arxiv_fields_json".into()));
    }

    #[test]
    fn schema_two_migration_adds_arxiv_and_normalizes_legacy_provenance() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE papers(
                    id TEXT PRIMARY KEY,
                    manual_overrides_json TEXT NOT NULL,
                    bibtex_fields_json TEXT NOT NULL
                 );
                 INSERT INTO papers VALUES(
                    'paper', '[\"title\",\"notes\"]', '[\"title\"]'
                 );
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        migrate(&connection).unwrap();
        let (manual, arxiv): (String, String) = connection
            .query_row(
                "SELECT manual_overrides_json, arxiv_fields_json FROM papers WHERE id='paper'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<BTreeSet<String>>(&manual).unwrap(),
            BTreeSet::from(["notes".into()])
        );
        assert_eq!(arxiv, "[]");
    }

    #[test]
    fn bibtex_import_is_stored_searchable_and_tracks_provenance() {
        let (_temporary, mut library) = library();
        let embedded = EmbeddedMetadata {
            title: Some("Embedded title".into()),
            ..Default::default()
        };
        let id = library
            .insert_scanned(ScannedData {
                relative_path: "paper.pdf",
                file_size: 10,
                modified_unix_ms: 1,
                content_hash: "hash",
                embedded: Some(&embedded),
                scan_error: None,
                duplicate_of: None,
            })
            .unwrap();
        let bibtex = r#"@ARTICLE{2008MNRAS.386..619C,
            author = {{Croke}, S. M. and {Gabuzda}, D. C.},
            title = {Imported title},
            journal = {MNRAS},
            year = {2008},
            doi = {10.1000/example}
        }"#;
        let imported = library.store_bibtex(&id, bibtex).unwrap();
        assert_eq!(imported.title.as_deref(), Some("Imported title"));
        assert_eq!(imported.bibcode.as_deref(), Some("2008MNRAS.386..619C"));
        assert_eq!(imported.bibtex.as_deref(), Some(bibtex));
        assert!(imported.bibtex_fields.contains("title"));
        assert!(!imported.manual_overrides.contains("title"));
        assert_eq!(library.paper_bibtex(&id).unwrap(), bibtex);
        assert_eq!(
            library
                .list_papers(&ListFilter {
                    query: Some("2008MNRAS.386..619C".into()),
                    ..Default::default()
                })
                .unwrap()
                .len(),
            1
        );

        let edited = library
            .update_paper(
                &id,
                PaperUpdate {
                    title: Some(Some("Manual title".into())),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!edited.bibtex_fields.contains("title"));
        assert_eq!(edited.bibtex.as_deref(), Some(bibtex));
        let refreshed = library
            .store_bibtex(
                &id,
                r#"@ARTICLE{2008MNRAS.386..619C, title={New ADS title}, year={2008}}"#,
            )
            .unwrap();
        assert_eq!(refreshed.title.as_deref(), Some("Manual title"));
        assert!(refreshed.manual_overrides.contains("title"));
        assert!(!refreshed.bibtex_fields.contains("title"));
        let reset = library.reset_field(&id, "title").unwrap();
        assert_eq!(reset.title.as_deref(), Some("Embedded title"));
    }

    #[test]
    fn chinese_normalization_is_searchable() {
        assert_eq!(search_terms(" 机器 学习 "), vec!["机器", "学习"]);
    }
}
