use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use lopdf::{Document, LoadOptions};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::{EmbeddedMetadata, LitmanError, Result};

const MAX_DECOMPRESSED_STREAM: usize = 16 * 1024 * 1024;
const MAX_XMP_SIZE: usize = 4 * 1024 * 1024;

pub fn extract_pdf_metadata(path: impl AsRef<Path>) -> Result<EmbeddedMetadata> {
    let path = path.as_ref();
    let info = Document::load_metadata(path).map_err(pdf_error)?;
    let mut raw_info = BTreeMap::new();
    put_raw(&mut raw_info, "Title", info.title.as_deref());
    put_raw(&mut raw_info, "Author", info.author.as_deref());
    put_raw(&mut raw_info, "Subject", info.subject.as_deref());
    put_raw(&mut raw_info, "Keywords", info.keywords.as_deref());
    put_raw(&mut raw_info, "Creator", info.creator.as_deref());
    put_raw(&mut raw_info, "Producer", info.producer.as_deref());
    put_raw(&mut raw_info, "CreationDate", info.creation_date.as_deref());
    put_raw(
        &mut raw_info,
        "ModificationDate",
        info.modification_date.as_deref(),
    );
    let mut metadata = EmbeddedMetadata {
        title: clean_option(info.title),
        authors: info.author.map(split_authors).unwrap_or_default(),
        abstract_text: clean_option(info.subject),
        keywords: info.keywords.map(split_keywords).unwrap_or_default(),
        page_count: Some(info.page_count),
        pdf_version: Some(info.version),
        encrypted: info.encrypted,
        creator: clean_option(info.creator),
        producer: clean_option(info.producer),
        creation_date: clean_option(info.creation_date),
        modification_date: clean_option(info.modification_date),
        raw_info,
        ..Default::default()
    };
    for (field, present) in [
        ("title", metadata.title.is_some()),
        ("authors", !metadata.authors.is_empty()),
        ("abstract_text", metadata.abstract_text.is_some()),
        ("keywords", !metadata.keywords.is_empty()),
        ("creator", metadata.creator.is_some()),
        ("producer", metadata.producer.is_some()),
    ] {
        if present {
            metadata
                .field_sources
                .insert(field.into(), "pdf_info".into());
        }
    }

    if let Ok(document) = Document::load_with_options(
        path,
        LoadOptions::with_max_decompressed_size(MAX_DECOMPRESSED_STREAM),
    ) && let Ok(catalog) = document.catalog()
        && let Ok(object) = catalog.get_deref(b"Metadata", &document)
        && let Ok(stream) = object.as_stream()
        && let Ok(bytes) = stream.decompressed_content_with_limit(MAX_XMP_SIZE)
    {
        merge_xmp(&mut metadata, &bytes)?;
    }

    Ok(metadata)
}

fn merge_xmp(metadata: &mut EmbeddedMetadata, bytes: &[u8]) -> Result<()> {
    let values = parse_xmp(bytes)?;
    metadata.raw_xmp = values.clone();
    set_first(&mut metadata.title, &values, "title");
    set_first(&mut metadata.abstract_text, &values, "description");
    set_first(&mut metadata.publication_date, &values, "coverdate");
    set_first(&mut metadata.container_title, &values, "publicationname");
    set_first(&mut metadata.volume, &values, "volume");
    set_first(&mut metadata.issue, &values, "number");
    set_first(&mut metadata.url, &values, "url");
    set_first(&mut metadata.language, &values, "language");

    if let Some(doi) = values.get("doi").and_then(|items| items.first()) {
        metadata.doi = normalize_doi(doi);
    }
    if let Some(creators) = values.get("creator") {
        let creators = clean_list(creators.clone());
        if !creators.is_empty() {
            metadata.authors = creators;
        }
    }
    if let Some(subjects) = values.get("subject") {
        let subjects = clean_list(subjects.clone());
        if !subjects.is_empty() {
            metadata.keywords = subjects;
        }
    }
    let starting = values.get("startingpage").and_then(|items| items.first());
    let ending = values.get("endingpage").and_then(|items| items.first());
    metadata.pages = match (starting, ending) {
        (Some(start), Some(end)) if start != end => Some(format!("{start}-{end}")),
        (Some(start), _) => Some(start.clone()),
        _ => metadata.pages.clone(),
    };
    for (property, field) in [
        ("title", "title"),
        ("creator", "authors"),
        ("description", "abstract_text"),
        ("subject", "keywords"),
        ("language", "language"),
        ("publicationname", "container_title"),
        ("doi", "doi"),
        ("volume", "volume"),
        ("number", "issue"),
        ("startingpage", "pages"),
        ("endingpage", "pages"),
        ("coverdate", "publication_date"),
        ("url", "url"),
    ] {
        if values
            .get(property)
            .is_some_and(|items| items.iter().any(|item| !item.trim().is_empty()))
        {
            metadata.field_sources.insert(field.into(), "xmp".into());
        }
    }
    Ok(())
}

fn parse_xmp(bytes: &[u8]) -> Result<BTreeMap<String, Vec<String>>> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut stack = Vec::<String>::new();
    let mut values = BTreeMap::<String, Vec<String>>::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => stack.push(local_name(event.name().as_ref())),
            Ok(Event::Empty(_)) => {}
            Ok(Event::Text(text)) => {
                let value = text
                    .decode()
                    .map_err(|error| metadata_error(error.to_string()))?
                    .trim()
                    .to_owned();
                if !value.is_empty()
                    && let Some(property) = metadata_property(&stack)
                {
                    values.entry(property.clone()).or_default().push(value);
                }
            }
            Ok(Event::CData(text)) => {
                let value = text
                    .decode()
                    .map_err(|error| metadata_error(error.to_string()))?
                    .trim()
                    .to_owned();
                if !value.is_empty()
                    && let Some(property) = metadata_property(&stack)
                {
                    values.entry(property.clone()).or_default().push(value);
                }
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(metadata_error(error.to_string())),
        }
    }
    Ok(values)
}

fn metadata_property(stack: &[String]) -> Option<&String> {
    stack.iter().rev().find(|name| {
        !matches!(
            name.as_str(),
            "li" | "alt" | "seq" | "bag" | "rdf" | "xmpmeta"
        )
    })
}

fn local_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name)
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn set_first(target: &mut Option<String>, values: &BTreeMap<String, Vec<String>>, key: &str) {
    if let Some(value) = values.get(key).and_then(|items| items.first()) {
        *target = clean_option(Some(value.clone()));
    }
}

fn put_raw(values: &mut BTreeMap<String, Vec<String>>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        values.insert(key.into(), vec![value.into()]);
    }
}

fn normalize_doi(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("doi:")
        .trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn split_authors(value: String) -> Vec<String> {
    let delimiter = if value.contains(';') {
        ';'
    } else if value.contains('\n') {
        '\n'
    } else {
        return clean_list(
            value
                .split(" and ")
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>(),
        );
    };
    clean_list(value.split(delimiter).map(ToOwned::to_owned).collect())
}

fn split_keywords(value: String) -> Vec<String> {
    clean_list(value.split([';', ',']).map(ToOwned::to_owned).collect())
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect()
}

fn clean_option(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn pdf_error(error: lopdf::Error) -> LitmanError {
    metadata_error(format!("cannot read PDF metadata: {error}"))
}

fn metadata_error(message: String) -> LitmanError {
    LitmanError::Io(io::Error::new(io::ErrorKind::InvalidData, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xmp_values_override_info() {
        let xmp = r#"<?xpacket begin='x'?>
          <x:xmpmeta xmlns:x='adobe:ns:meta/'>
            <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
              <rdf:Description xmlns:dc='http://purl.org/dc/elements/1.1/' xmlns:prism='http://prismstandard.org/namespaces/basic/2.0/' xmlns:custom='urn:litman-test'>
                <dc:title><rdf:Alt><rdf:li xml:lang='zh-CN'>机器学习</rdf:li></rdf:Alt></dc:title>
                <dc:creator><rdf:Seq><rdf:li>张三</rdf:li><rdf:li>Jane Doe</rdf:li></rdf:Seq></dc:creator>
                <prism:doi>https://doi.org/10.1000/test</prism:doi>
                <custom:review-status>accepted</custom:review-status>
              </rdf:Description>
            </rdf:RDF>
          </x:xmpmeta>"#;
        let mut metadata = EmbeddedMetadata::default();
        merge_xmp(&mut metadata, xmp.as_bytes()).unwrap();
        assert_eq!(metadata.title.as_deref(), Some("机器学习"));
        assert_eq!(metadata.authors, vec!["张三", "Jane Doe"]);
        assert_eq!(metadata.doi.as_deref(), Some("10.1000/test"));
        assert_eq!(metadata.raw_xmp["title"], vec!["机器学习"]);
        assert_eq!(metadata.field_sources["title"], "xmp");
        assert_eq!(metadata.field_sources["authors"], "xmp");
        assert_eq!(metadata.raw_xmp["review-status"], vec!["accepted"]);
    }
}
