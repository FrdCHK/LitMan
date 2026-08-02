use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

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
