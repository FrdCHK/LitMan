use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

use litman_core::{Config, Library, ListFilter, ScanOptions};
use lopdf::{Document, Object, dictionary};

fn litman(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_litman"))
        .args(args)
        .output()
        .expect("run litman")
}

fn success(args: &[String]) -> Output {
    let output = litman(args);
    assert!(
        output.status.success(),
        "litman failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn write_valid_pdf(path: &Path, title: &str) {
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
    let catalog_id = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    let info_id = document.add_object(dictionary! { "Title" => Object::string_literal(title) });
    document.trailer.set("Root", catalog_id);
    document.trailer.set("Info", info_id);
    document.save(path).unwrap();
}

fn replacement_library(temporary: &TempDir) -> (PathBuf, PathBuf, String) {
    let root = temporary.path().join("pdfs");
    fs::create_dir(&root).unwrap();
    write_valid_pdf(&root.join("preprint.pdf"), "Preprint");
    let config = temporary.path().join("library.toml");
    let mut library = Library::init(&config, Config::new(root.clone())).unwrap();
    library.scan(ScanOptions::default(), None, |_| {}).unwrap();
    let id = library
        .list_papers(&ListFilter::default())
        .unwrap()
        .remove(0)
        .id;
    library
        .store_bibtex(
            &id,
            "@article{2020ApJ...900....1A, title={Published}, author={Author, A}, year={2020}}",
        )
        .unwrap();
    (config, root, id)
}

#[test]
fn scixplorer_update_pdf_requires_confirmation_and_preserves_selected_download() {
    let temporary = TempDir::new().unwrap();
    let (config, root, id) = replacement_library(&temporary);
    let publisher = temporary.path().join("publisher-download.pdf");
    write_valid_pdf(&publisher, "Publisher");
    let original = fs::read(root.join("preprint.pdf")).unwrap();

    let declined_noninteractive = litman(&[
        "--config".into(),
        path(&config),
        "scixplorer".into(),
        "update-pdf".into(),
        id.clone(),
        "--file".into(),
        path(&publisher),
    ]);
    assert!(!declined_noninteractive.status.success());
    assert!(String::from_utf8_lossy(&declined_noninteractive.stderr).contains("--yes"));
    assert_eq!(fs::read(root.join("preprint.pdf")).unwrap(), original);
    assert!(!root.join("LitMan-backups").exists());

    let replaced = success(&[
        "--config".into(),
        path(&config),
        "scixplorer".into(),
        "update-pdf".into(),
        id,
        "--file".into(),
        path(&publisher),
        "--yes".into(),
    ]);
    let stdout = String::from_utf8(replaced.stdout).unwrap();
    let stderr = String::from_utf8(replaced.stderr).unwrap();
    assert!(stderr.contains("WARNING"));
    assert!(stdout.contains("Active PDF"));
    assert!(stdout.contains("Backup PDF"));
    assert!(
        publisher.is_file(),
        "the external source download must remain untouched"
    );
    assert!(root.join("2020ApJ...900....1A.pdf").is_file());
    assert!(
        root.join("LitMan-backups")
            .join("2020ApJ...900....1A_bk.pdf")
            .is_file()
    );
}

#[test]
fn portable_localized_cli_workflow_and_stable_json() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().join("论文 PDFs");
    let library_dir = temporary.path().join("portable library");
    let config = library_dir.join("library.toml");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&library_dir).unwrap();
    let pdf = root.join("机器学习.pdf");
    let original_pdf = b"%PDF-1.7\nmalformed fixture";
    fs::write(&pdf, original_pdf).unwrap();

    success(&[
        "init".into(),
        "--config".into(),
        path(&config),
        "--root".into(),
        path(&root),
        "--language".into(),
        "zh-CN".into(),
    ]);
    success(&[
        "--config".into(),
        path(&config),
        "scixplorer".into(),
        "token".into(),
        "set".into(),
        "test-personal-token".into(),
    ]);
    let token_status = success(&[
        "--config".into(),
        path(&config),
        "scixplorer".into(),
        "token".into(),
        "status".into(),
    ]);
    let token_status = String::from_utf8(token_status.stdout).unwrap();
    assert!(token_status.contains("已配置"));
    assert!(!token_status.contains("test-personal-token"));
    assert!(
        fs::read_to_string(&config)
            .unwrap()
            .contains("test-personal-token")
    );
    success(&[
        "--config".into(),
        path(&config),
        "scixplorer".into(),
        "token".into(),
        "clear".into(),
    ]);
    assert!(
        !fs::read_to_string(&config)
            .unwrap()
            .contains("scixplorer_api_token")
    );
    success(&["--config".into(), path(&config), "scan".into()]);

    let listed = success(&[
        "--config".into(),
        path(&config),
        "list".into(),
        "--format".into(),
        "json".into(),
    ]);
    let papers: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let paper = &papers.as_array().unwrap()[0];
    for stable_key in [
        "id",
        "relative_path",
        "file_status",
        "title",
        "authors",
        "importance",
        "manual_overrides",
    ] {
        assert!(
            paper.get(stable_key).is_some(),
            "missing JSON key {stable_key}"
        );
    }
    assert_eq!(paper["file_status"], "error");
    let id = paper["id"].as_str().unwrap();
    let prefix = id[..8].to_owned();

    success(&[
        "--config".into(),
        path(&config),
        "edit".into(),
        prefix.clone(),
        "--title".into(),
        "机器学习方法".into(),
        "--author".into(),
        "李伟".into(),
        "--author".into(),
        "Ada Smith".into(),
        "--date".into(),
        "2024-05-17".into(),
        "--keyword".into(),
        "中文".into(),
    ]);
    success(&[
        "--config".into(),
        path(&config),
        "rate".into(),
        prefix.clone(),
        "5".into(),
    ]);
    let invalid_rating = litman(&[
        "--config".into(),
        path(&config),
        "rate".into(),
        prefix.clone(),
        "9".into(),
    ]);
    assert!(!invalid_rating.status.success());
    assert!(String::from_utf8_lossy(&invalid_rating.stderr).contains("重要程度"));
    success(&[
        "--config".into(),
        path(&config),
        "group".into(),
        "create".into(),
        "课题/方法".into(),
    ]);
    success(&[
        "--config".into(),
        path(&config),
        "group".into(),
        "add".into(),
        "课题/方法".into(),
        prefix.clone(),
    ]);

    let searched = success(&[
        "--config".into(),
        path(&config),
        "search".into(),
        "学习".into(),
        "--group".into(),
        "课题/方法".into(),
        "--min-importance".into(),
        "4".into(),
        "--format".into(),
        "json".into(),
    ]);
    let found: Value = serde_json::from_slice(&searched.stdout).unwrap();
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["authors"][0], "李伟");
    assert_eq!(found[0]["authors"][1], "Ada Smith");

    let table = success(&["--config".into(), path(&config), "list".into()]);
    let table = String::from_utf8(table.stdout).unwrap();
    let mut lines = table.lines();
    assert_eq!(lines.next(), Some("ID\t标题\t第一作者\t年份"));
    let expected_row = format!("{prefix}\t机器学习方法\t李伟 等\t2024");
    assert_eq!(lines.next(), Some(expected_row.as_str()));
    assert!(!table.contains("重要程度"));
    assert!(!table.contains("状态"));
    assert!(!table.contains("Ada Smith"));

    let backup = temporary.path().join("backup");
    success(&[
        "--config".into(),
        path(&config),
        "backup".into(),
        path(&backup),
    ]);
    assert!(backup.join("library.toml").is_file());
    assert!(backup.join("literature.sqlite3").is_file());

    let relocated = temporary.path().join("relocated PDFs");
    fs::create_dir(&relocated).unwrap();
    success(&[
        "--config".into(),
        path(&config),
        "root".into(),
        "set".into(),
        path(&relocated),
    ]);
    let updated_config = fs::read_to_string(&config).unwrap();
    assert!(updated_config.contains("relocated PDFs"));
    assert_eq!(fs::read(pdf).unwrap(), original_pdf);
}
