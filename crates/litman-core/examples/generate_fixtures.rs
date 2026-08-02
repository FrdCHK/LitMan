use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use lopdf::{
    Document, EncryptionState, EncryptionVersion, Object, Permissions, Stream, StringFormat,
    dictionary,
};

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/litman-fixtures"));
    fs::create_dir_all(&output)?;

    save(
        document(Some(("Information metadata", "Ada Smith; 李伟")), None),
        &output.join("info-only.pdf"),
    )?;
    save(
        document(None, Some(&xmp("XMP and PRISM", "张三", "10.1000/xmp"))),
        &output.join("xmp-prism.pdf"),
    )?;
    save(
        document(None, Some(&xmp("中文文献管理", "李伟", "10.1000/chinese"))),
        &output.join("中文文件名.pdf"),
    )?;
    save(document(None, None), &output.join("no-metadata.pdf"))?;
    save(
        document(
            Some(("PDF Info loses", "Info Author")),
            Some(&xmp("XMP wins", "XMP Author", "10.1000/conflict")),
        ),
        &output.join("metadata-conflict.pdf"),
    )?;

    fs::write(output.join("malformed.pdf"), b"%PDF-1.7\nmalformed fixture")?;

    let mut encrypted = document(Some(("Encrypted title", "Protected Author")), None);
    let state = EncryptionState::try_from(EncryptionVersion::V1 {
        document: &encrypted,
        owner_password: "owner-password",
        user_password: "litman-fixture",
        permissions: Permissions::empty(),
    })?;
    encrypted.encrypt(&state)?;
    save(
        encrypted,
        &output.join("encrypted-password-litman-fixture.pdf"),
    )?;

    let duplicate = output.join("duplicate-a.pdf");
    save(
        document(Some(("Duplicate content", "Same Author")), None),
        &duplicate,
    )?;
    fs::copy(&duplicate, output.join("duplicate-b.pdf"))?;

    save(
        document(Some(("Move scenario", "Fixture Author")), None),
        &output.join("move-source.pdf"),
    )?;
    save(
        document(Some(("Missing scenario", "Fixture Author")), None),
        &output.join("missing-after-first-scan.pdf"),
    )?;
    fs::write(
        output.join("SCENARIOS.txt"),
        "Move: scan once, then rename move-source.pdf to nested/move-target.pdf and scan again.\n\
         Missing: scan once, then remove missing-after-first-scan.pdf and scan again.\n\
         Encrypted fixture user password: litman-fixture.\n",
    )?;

    println!("{}", output.display());
    Ok(())
}

fn document(info: Option<(&str, &str)>, xmp_packet: Option<&str>) -> Document {
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

    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };
    if let Some(packet) = xmp_packet {
        let metadata_id = document.add_object(Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            packet.as_bytes().to_vec(),
        ));
        catalog.set("Metadata", metadata_id);
    }
    let catalog_id = document.add_object(catalog);
    document.trailer.set("Root", catalog_id);
    document.trailer.set(
        "ID",
        vec![
            Object::string_literal("LitMan fixture document"),
            Object::string_literal("LitMan fixture document"),
        ],
    );

    if let Some((title, author)) = info {
        let info_id = document.add_object(dictionary! {
            "Title" => pdf_string(title),
            "Author" => pdf_string(author),
            "Subject" => pdf_string("Fixture abstract"),
            "Keywords" => pdf_string("fixture; metadata"),
        });
        document.trailer.set("Info", info_id);
    }
    document
}

fn xmp(title: &str, author: &str, doi: &str) -> String {
    format!(
        r#"<?xpacket begin='﻿'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/'>
  <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
    <rdf:Description xmlns:dc='http://purl.org/dc/elements/1.1/' xmlns:prism='http://prismstandard.org/namespaces/basic/2.0/'>
      <dc:title><rdf:Alt><rdf:li xml:lang='x-default'>{title}</rdf:li></rdf:Alt></dc:title>
      <dc:creator><rdf:Seq><rdf:li>{author}</rdf:li></rdf:Seq></dc:creator>
      <dc:description><rdf:Alt><rdf:li>Fixture abstract from XMP</rdf:li></rdf:Alt></dc:description>
      <prism:publicationName>Fixture Journal</prism:publicationName>
      <prism:coverDate>2026-08-02</prism:coverDate>
      <prism:doi>{doi}</prism:doi>
      <prism:volume>12</prism:volume><prism:number>3</prism:number>
      <prism:startingPage>10</prism:startingPage><prism:endingPage>19</prism:endingPage>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end='w'?>"#
    )
}

fn pdf_string(value: &str) -> Object {
    let mut bytes = vec![0xFE, 0xFF];
    for unit in value.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, StringFormat::Hexadecimal)
}

fn save(mut document: Document, path: &Path) -> Result<(), Box<dyn Error>> {
    document.compress();
    document.save(path)?;
    Ok(())
}
