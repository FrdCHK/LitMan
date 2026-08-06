use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{LitmanError, Result, ScixplorerRecord, ScixplorerSearchField};

const ADS_API_BASE: &str = "https://api.adsabs.harvard.edu/v1";
const SEARCH_FIELDS: &str = "bibcode,title,author,pubdate,doi,pub";
const MAX_API_RESPONSE_SIZE: u64 = 2 * 1024 * 1024;
const MAX_BIBTEX_SIZE: usize = 1024 * 1024;

#[derive(Clone)]
pub struct ScixplorerClient {
    token: String,
    api_base: String,
    agent: ureq::Agent,
}

impl ScixplorerClient {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        Self::with_api_base(token, ADS_API_BASE, false)
    }

    pub(crate) fn with_api_base(
        token: impl Into<String>,
        api_base: impl Into<String>,
        allow_http: bool,
    ) -> Result<Self> {
        let token = token.into().trim().to_owned();
        if token.is_empty() || token.chars().any(char::is_control) {
            return Err(LitmanError::MissingScixplorerToken);
        }
        Ok(Self {
            token,
            api_base: api_base.into().trim_end_matches('/').to_owned(),
            agent: ureq::Agent::new_with_config(
                ureq::Agent::config_builder()
                    .https_only(!allow_http)
                    .max_redirects(0)
                    .timeout_global(Some(Duration::from_secs(30)))
                    .build(),
            ),
        })
    }

    pub fn search(
        &self,
        field: ScixplorerSearchField,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ScixplorerRecord>> {
        let normalized_doi;
        let query = if field == ScixplorerSearchField::Doi {
            normalized_doi = normalize_doi(query);
            normalized_doi.as_str()
        } else {
            query.trim()
        };
        if query.is_empty() || query.len() > 900 {
            return Err(LitmanError::Scixplorer(
                "search text must contain between 1 and 900 bytes".into(),
            ));
        }
        if !(1..=100).contains(&limit) {
            return Err(LitmanError::Scixplorer(
                "search result limit must be between 1 and 100".into(),
            ));
        }
        let field = match field {
            ScixplorerSearchField::Title => "title",
            ScixplorerSearchField::Doi => "doi",
            ScixplorerSearchField::Bibcode => "bibcode",
        };
        let ads_query = format!("{field}:\"{}\"", escape_ads_phrase(query));
        let authorization = self.authorization();
        let rows = limit.to_string();
        let url = format!("{}/search/query", self.api_base);
        let mut response = self
            .agent
            .get(url)
            .header("Authorization", &authorization)
            .query("q", &ads_query)
            .query("fl", SEARCH_FIELDS)
            .query("rows", &rows)
            .call()
            .map_err(api_error)?;
        let envelope = response
            .body_mut()
            .with_config()
            .limit(MAX_API_RESPONSE_SIZE)
            .read_json::<SearchEnvelope>()
            .map_err(api_error)?;
        Ok(envelope
            .response
            .docs
            .into_iter()
            .map(|document| ScixplorerRecord {
                bibcode: document.bibcode,
                title: document.title.into_iter().next().unwrap_or_default(),
                authors: document.author,
                publication_date: document.pubdate,
                doi: document.doi.into_iter().next(),
                publication: document.publication,
            })
            .collect())
    }

    pub fn bibtex(&self, bibcode: &str) -> Result<String> {
        validate_bibcode(bibcode)?;
        let authorization = self.authorization();
        let url = format!("{}/export/bibtex", self.api_base);
        let mut response = self
            .agent
            .post(url)
            .header("Authorization", &authorization)
            .header("Content-Type", "application/json")
            .send_json(&ExportRequest {
                bibcode: [bibcode.trim()],
            })
            .map_err(api_error)?;
        let export = response
            .body_mut()
            .with_config()
            .limit(MAX_API_RESPONSE_SIZE)
            .read_json::<ExportResponse>()
            .map_err(api_error)?
            .export;
        if export.trim().is_empty() {
            return Err(LitmanError::Scixplorer(
                "ADS returned an empty BibTeX export".into(),
            ));
        }
        if export.len() > MAX_BIBTEX_SIZE {
            return Err(LitmanError::Scixplorer(
                "ADS returned a BibTeX entry larger than 1 MiB".into(),
            ));
        }
        Ok(export)
    }

    pub(crate) fn pdf_sources(&self, bibcode: &str) -> Result<AdsPdfSources> {
        validate_bibcode(bibcode)?;
        let authorization = self.authorization();
        let query = format!("bibcode:\"{}\"", escape_ads_phrase(bibcode.trim()));
        let url = format!("{}/search/query", self.api_base);
        let mut response = self
            .agent
            .get(url)
            .header("Authorization", &authorization)
            .query("q", &query)
            .query("fl", "bibcode,esources")
            .query("rows", "2")
            .call()
            .map_err(api_error)?;
        let envelope = response
            .body_mut()
            .with_config()
            .limit(MAX_API_RESPONSE_SIZE)
            .read_json::<SearchEnvelope>()
            .map_err(api_error)?;
        let document = envelope
            .response
            .docs
            .into_iter()
            .find(|document| document.bibcode == bibcode.trim())
            .ok_or_else(|| {
                LitmanError::Scixplorer(format!(
                    "ADS did not return an exact record for {}",
                    bibcode.trim()
                ))
            })?;
        let normalized = document
            .esources
            .into_iter()
            .map(|source| source.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        Ok(AdsPdfSources {
            pub_pdf: normalized.contains("pub_pdf"),
            eprint_pdf: normalized.contains("eprint_pdf"),
        })
    }

    fn authorization(&self) -> String {
        format!("Bearer {}", self.token)
    }
}

pub fn scixplorer_url(bibcode: &str) -> Result<String> {
    validate_bibcode(bibcode)?;
    Ok(format!(
        "https://scixplorer.org/abs/{}/abstract",
        bibcode.trim()
    ))
}

pub fn publisher_pdf_url(bibcode: &str) -> Result<String> {
    validate_bibcode(bibcode)?;
    Ok(format!(
        "https://scixplorer.org/link_gateway/{}/PUB_PDF",
        bibcode.trim()
    ))
}

pub(crate) fn eprint_pdf_url(bibcode: &str) -> Result<String> {
    validate_bibcode(bibcode)?;
    Ok(format!(
        "https://scixplorer.org/link_gateway/{}/EPRINT_PDF",
        bibcode.trim()
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdsPdfSources {
    pub(crate) pub_pdf: bool,
    pub(crate) eprint_pdf: bool,
}

pub(crate) fn validate_bibcode(bibcode: &str) -> Result<()> {
    let bibcode = bibcode.trim();
    if bibcode.is_empty()
        || bibcode.len() > 128
        || !bibcode.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '&' | '+' | '-' | '_' | ':')
        })
    {
        return Err(LitmanError::Scixplorer("invalid ADS bibcode".into()));
    }
    Ok(())
}

fn api_error(error: ureq::Error) -> LitmanError {
    LitmanError::Scixplorer(format!("ADS API request failed: {error}"))
}

fn escape_ads_phrase(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Deserialize)]
struct SearchEnvelope {
    response: SearchResponse,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    docs: Vec<SearchDocument>,
}

#[derive(Deserialize)]
struct SearchDocument {
    bibcode: String,
    #[serde(default)]
    title: Vec<String>,
    #[serde(default)]
    author: Vec<String>,
    pubdate: Option<String>,
    #[serde(default)]
    doi: Vec<String>,
    #[serde(rename = "pub")]
    publication: Option<String>,
    #[serde(default)]
    esources: Vec<String>,
}

#[derive(Serialize)]
struct ExportRequest<'a> {
    bibcode: [&'a str; 1],
}

#[derive(Deserialize)]
struct ExportResponse {
    export: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BibtexMetadata {
    pub(crate) bibcode: String,
    pub(crate) title: Option<String>,
    pub(crate) authors: Option<Vec<String>>,
    pub(crate) abstract_text: Option<String>,
    pub(crate) publication_date: Option<String>,
    pub(crate) container_title: Option<String>,
    pub(crate) volume: Option<String>,
    pub(crate) issue: Option<String>,
    pub(crate) pages: Option<String>,
    pub(crate) doi: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) language: Option<String>,
    pub(crate) keywords: Option<Vec<String>>,
}

impl BibtexMetadata {
    pub(crate) fn populated_fields(&self) -> BTreeSet<String> {
        let mut fields = BTreeSet::new();
        for (name, present) in [
            ("title", self.title.is_some()),
            ("authors", self.authors.is_some()),
            ("abstract_text", self.abstract_text.is_some()),
            ("publication_date", self.publication_date.is_some()),
            ("container_title", self.container_title.is_some()),
            ("volume", self.volume.is_some()),
            ("issue", self.issue.is_some()),
            ("pages", self.pages.is_some()),
            ("doi", self.doi.is_some()),
            ("url", self.url.is_some()),
            ("language", self.language.is_some()),
            ("keywords", self.keywords.is_some()),
        ] {
            if present {
                fields.insert(name.into());
            }
        }
        fields
    }
}

pub(crate) fn parse_bibtex(input: &str) -> Result<BibtexMetadata> {
    if input.len() > MAX_BIBTEX_SIZE {
        return Err(invalid_bibtex("entry is larger than 1 MiB"));
    }
    let (bibcode, fields) = parse_entry(input)?;
    validate_bibcode(&bibcode)?;

    let text = |name: &str| {
        fields
            .get(name)
            .map(tex_to_text)
            .filter(|value| !value.is_empty())
    };
    let authors = fields.get("author").map(|value| {
        split_bibtex_list(value, " and ")
            .into_iter()
            .map(|value| tex_to_text(&value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
    });
    let authors = authors.filter(|values| !values.is_empty());
    let keywords = fields.get("keywords").map(|value| {
        value
            .split([',', ';'])
            .map(tex_to_text)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
    });
    let keywords = keywords.filter(|values| !values.is_empty());
    let publication_date = text("year").map(|year| match text("month") {
        Some(month) => month_number(&month)
            .map(|month| format!("{year}-{month}"))
            .unwrap_or(year),
        None => year,
    });

    Ok(BibtexMetadata {
        bibcode,
        title: text("title"),
        authors,
        abstract_text: text("abstract"),
        publication_date,
        container_title: text("journal").or_else(|| text("booktitle")),
        volume: text("volume"),
        issue: text("number").or_else(|| text("issue")),
        pages: text("pages"),
        doi: text("doi").map(|value| normalize_doi(&value)),
        url: text("url").or_else(|| text("adsurl")),
        language: text("language"),
        keywords,
    })
}

fn parse_entry(input: &str) -> Result<(String, BTreeMap<String, String>)> {
    let at = input
        .find('@')
        .ok_or_else(|| invalid_bibtex("missing entry marker"))?;
    let bytes = input.as_bytes();
    let mut cursor = at + 1;
    skip_while(bytes, &mut cursor, |byte| byte.is_ascii_alphanumeric());
    skip_space(bytes, &mut cursor);
    let Some(&opening) = bytes.get(cursor) else {
        return Err(invalid_bibtex("missing entry body"));
    };
    let closing = match opening {
        b'{' => b'}',
        b'(' => b')',
        _ => return Err(invalid_bibtex("entry body must use braces or parentheses")),
    };
    cursor += 1;
    skip_space(bytes, &mut cursor);
    let key_start = cursor;
    while bytes.get(cursor).is_some_and(|byte| *byte != b',') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b',') {
        return Err(invalid_bibtex("missing citation key separator"));
    }
    let bibcode = input[key_start..cursor].trim().to_owned();
    cursor += 1;
    let mut fields = BTreeMap::new();

    loop {
        skip_separators(bytes, &mut cursor);
        if bytes.get(cursor) == Some(&closing) {
            break;
        }
        if cursor >= bytes.len() {
            return Err(invalid_bibtex("unterminated entry"));
        }
        let name_start = cursor;
        skip_while(bytes, &mut cursor, |byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
        });
        if name_start == cursor {
            return Err(invalid_bibtex("invalid field name"));
        }
        let name = input[name_start..cursor].to_ascii_lowercase();
        skip_space(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'=') {
            return Err(invalid_bibtex("missing field assignment"));
        }
        cursor += 1;
        skip_space(bytes, &mut cursor);
        let value = match bytes.get(cursor).copied() {
            Some(b'{') => parse_braced(input, &mut cursor)?,
            Some(b'"') => parse_quoted(input, &mut cursor)?,
            Some(_) => {
                let start = cursor;
                while bytes
                    .get(cursor)
                    .is_some_and(|byte| *byte != b',' && *byte != closing)
                {
                    cursor += 1;
                }
                input[start..cursor].trim().to_owned()
            }
            None => return Err(invalid_bibtex("missing field value")),
        };
        fields.insert(name, value);
        skip_space(bytes, &mut cursor);
        if bytes.get(cursor) == Some(&b',') {
            cursor += 1;
        }
    }
    Ok((bibcode, fields))
}

fn parse_braced(input: &str, cursor: &mut usize) -> Result<String> {
    let bytes = input.as_bytes();
    *cursor += 1;
    let start = *cursor;
    let mut depth = 1usize;
    let mut escaped = false;
    while let Some(&byte) = bytes.get(*cursor) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth -= 1;
            if depth == 0 {
                let value = input[start..*cursor].to_owned();
                *cursor += 1;
                return Ok(value);
            }
        }
        *cursor += 1;
    }
    Err(invalid_bibtex("unterminated braced value"))
}

fn parse_quoted(input: &str, cursor: &mut usize) -> Result<String> {
    let bytes = input.as_bytes();
    *cursor += 1;
    let start = *cursor;
    let mut escaped = false;
    while let Some(&byte) = bytes.get(*cursor) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            let value = input[start..*cursor].to_owned();
            *cursor += 1;
            return Ok(value);
        }
        *cursor += 1;
    }
    Err(invalid_bibtex("unterminated quoted value"))
}

fn split_bibtex_list(value: &str, separator: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let separator = separator.as_bytes();
    let mut values = Vec::new();
    let mut start = 0;
    let mut cursor = 0;
    let mut depth = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && bytes[cursor..].starts_with(separator) {
            values.push(value[start..cursor].trim().to_owned());
            cursor += separator.len();
            start = cursor;
        } else {
            cursor += 1;
        }
    }
    values.push(value[start..].trim().to_owned());
    values
}

fn tex_to_text(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '{' | '}' => {}
            '~' => output.push(' '),
            '\\' => match characters.peek().copied() {
                Some(command @ ('\'' | '`' | '"' | '^' | '~')) => {
                    characters.next();
                    let braced = characters.peek() == Some(&'{');
                    if braced {
                        characters.next();
                    }
                    if let Some(base) = characters.next() {
                        output.push(accented(command, base).unwrap_or(base));
                    }
                    if braced && characters.peek() == Some(&'}') {
                        characters.next();
                    }
                }
                Some(symbol @ ('&' | '%' | '_' | '#' | '$' | '{' | '}')) => {
                    characters.next();
                    output.push(symbol);
                }
                Some(_) => {
                    let mut command = String::new();
                    while characters
                        .peek()
                        .is_some_and(|value| value.is_ascii_alphabetic())
                    {
                        command.push(characters.next().unwrap());
                    }
                    match command.as_str() {
                        "AA" => output.push('Å'),
                        "aa" => output.push('å'),
                        "AE" => output.push('Æ'),
                        "ae" => output.push('æ'),
                        "OE" => output.push('Œ'),
                        "oe" => output.push('œ'),
                        "O" => output.push('Ø'),
                        "o" => output.push('ø'),
                        "ss" => output.push('ß'),
                        "L" => output.push('Ł'),
                        "l" => output.push('ł'),
                        _ => {}
                    }
                    if command.is_empty()
                        && let Some(character) = characters.next()
                    {
                        output.push(character);
                    }
                }
                None => {}
            },
            _ => output.push(character),
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn accented(mark: char, base: char) -> Option<char> {
    Some(match (mark, base) {
        ('\'', 'a') => 'á',
        ('\'', 'e') => 'é',
        ('\'', 'i') => 'í',
        ('\'', 'o') => 'ó',
        ('\'', 'u') => 'ú',
        ('\'', 'A') => 'Á',
        ('\'', 'E') => 'É',
        ('\'', 'I') => 'Í',
        ('\'', 'O') => 'Ó',
        ('\'', 'U') => 'Ú',
        ('`', 'a') => 'à',
        ('`', 'e') => 'è',
        ('`', 'i') => 'ì',
        ('`', 'o') => 'ò',
        ('`', 'u') => 'ù',
        ('"', 'a') => 'ä',
        ('"', 'e') => 'ë',
        ('"', 'i') => 'ï',
        ('"', 'o') => 'ö',
        ('"', 'u') => 'ü',
        ('"', 'A') => 'Ä',
        ('"', 'O') => 'Ö',
        ('"', 'U') => 'Ü',
        ('^', 'a') => 'â',
        ('^', 'e') => 'ê',
        ('^', 'i') => 'î',
        ('^', 'o') => 'ô',
        ('^', 'u') => 'û',
        ('~', 'a') => 'ã',
        ('~', 'n') => 'ñ',
        ('~', 'o') => 'õ',
        ('~', 'N') => 'Ñ',
        _ => return None,
    })
}

fn normalize_doi(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("doi:")
        .trim()
        .to_owned()
}

fn month_number(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "jan" | "january" | "1" | "01" => Some("01"),
        "feb" | "february" | "2" | "02" => Some("02"),
        "mar" | "march" | "3" | "03" => Some("03"),
        "apr" | "april" | "4" | "04" => Some("04"),
        "may" | "5" | "05" => Some("05"),
        "jun" | "june" | "6" | "06" => Some("06"),
        "jul" | "july" | "7" | "07" => Some("07"),
        "aug" | "august" | "8" | "08" => Some("08"),
        "sep" | "sept" | "september" | "9" | "09" => Some("09"),
        "oct" | "october" | "10" => Some("10"),
        "nov" | "november" | "11" => Some("11"),
        "dec" | "december" | "12" => Some("12"),
        _ => None,
    }
}

fn skip_space(bytes: &[u8], cursor: &mut usize) {
    skip_while(bytes, cursor, |byte| byte.is_ascii_whitespace());
}

fn skip_separators(bytes: &[u8], cursor: &mut usize) {
    skip_while(bytes, cursor, |byte| {
        byte.is_ascii_whitespace() || byte == b','
    });
}

fn skip_while(bytes: &[u8], cursor: &mut usize, predicate: impl Fn(u8) -> bool) {
    while bytes.get(*cursor).copied().is_some_and(&predicate) {
        *cursor += 1;
    }
}

fn invalid_bibtex(detail: &str) -> LitmanError {
    LitmanError::Scixplorer(format!("ADS returned invalid BibTeX: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ads_bibtex_is_parsed_into_litman_metadata() {
        let bibtex = r#"@ARTICLE{2008MNRAS.386..619C,
 author = {{Croke}, S.~M. and {Gabuzda}, D.~C.},
 title = "{Parsec-scale magnetic-field structure in several BL Lac objects}",
 journal = {Monthly Notices of the Royal Astronomical Society},
 keywords = {galaxies: active, polarization},
 year = 2008,
 month = May,
 volume = {386},
 number = {2},
 pages = {619--626},
 doi = {10.1111/j.1365-2966.2008.13087.x},
 adsurl = {https://ui.adsabs.harvard.edu/abs/2008MNRAS.386..619C}
}"#;
        let metadata = parse_bibtex(bibtex).unwrap();
        assert_eq!(metadata.bibcode, "2008MNRAS.386..619C");
        assert_eq!(
            metadata.authors.unwrap(),
            vec!["Croke, S. M.", "Gabuzda, D. C."]
        );
        assert_eq!(metadata.publication_date.as_deref(), Some("2008-05"));
        assert_eq!(metadata.volume.as_deref(), Some("386"));
        assert_eq!(metadata.issue.as_deref(), Some("2"));
        assert_eq!(metadata.pages.as_deref(), Some("619--626"));
        assert_eq!(
            metadata.keywords.unwrap(),
            vec!["galaxies: active", "polarization"]
        );
    }

    #[test]
    fn latex_accents_and_nested_braces_become_readable_text() {
        let bibtex = r#"@ARTICLE{2015RaSc...50..916A,
          author = {{Bergad{\`a}}, P. and {M{\"u}ller}, A.},
          title = {{An {ADS} title with \& symbols}},
          year = {2015}
        }"#;
        let metadata = parse_bibtex(bibtex).unwrap();
        assert_eq!(metadata.authors.unwrap(), vec!["Bergadà, P.", "Müller, A."]);
        assert_eq!(
            metadata.title.as_deref(),
            Some("An ADS title with & symbols")
        );
    }

    #[test]
    fn scixplorer_urls_reject_path_injection() {
        assert_eq!(
            scixplorer_url("2008MNRAS.386..619C").unwrap(),
            "https://scixplorer.org/abs/2008MNRAS.386..619C/abstract"
        );
        assert!(scixplorer_url("../../settings").is_err());
    }

    #[test]
    fn oversized_bibtex_is_rejected_before_parsing() {
        assert!(parse_bibtex(&"x".repeat(MAX_BIBTEX_SIZE + 1)).is_err());
    }

    #[test]
    fn search_queries_escape_quotes_and_backslashes() {
        assert_eq!(
            escape_ads_phrase("a \\\"quoted\\\" title"),
            "a \\\\\\\"quoted\\\\\\\" title"
        );
    }

    #[test]
    fn documented_ads_json_shapes_are_decoded() {
        let search: SearchEnvelope = serde_json::from_str(
            r#"{"response":{"docs":[{"bibcode":"2008MNRAS.386..619C","title":["A title"],"author":["Croke, S. M."],"pubdate":"2008-05-00","doi":["10.1000/example"],"pub":"MNRAS"}]}}"#,
        )
        .unwrap();
        assert_eq!(search.response.docs[0].bibcode, "2008MNRAS.386..619C");
        let export: ExportResponse = serde_json::from_str(
            r#"{"export":"@ARTICLE{2008MNRAS.386..619C,\\n title={A title}\\n}"}"#,
        )
        .unwrap();
        assert!(export.export.starts_with("@ARTICLE{2008MNRAS.386..619C"));
    }
}
