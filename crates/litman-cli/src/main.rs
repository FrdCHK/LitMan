use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use litman_core::{
    Config, FileStatus, Language, Library, ListFilter, LitmanError, Locale, Paper, PaperUpdate,
    ScanEvent, ScanOptions, default_config_path,
};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "litman",
    version,
    author,
    about = "Local-first literature manager / 本地文献管理器",
    after_help = "Author / 作者: Jingdong Zhang\nLicense / 许可证: GNU GPLv3\nRun `litman manual` for the complete offline manual. / 运行 `litman manual` 查看完整离线手册。"
)]
struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        help = "Library configuration / 文献库配置文件"
    )]
    config: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        value_enum,
        help = "Human-output language / 普通输出语言"
    )]
    language: Option<LanguageArg>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Create a portable library / 新建可移植文献库")]
    Init(InitArgs),
    #[command(about = "Scan the configured PDF root / 扫描配置的 PDF 根目录")]
    Scan {
        #[arg(
            long,
            help = "Reread metadata from unchanged PDFs / 重新读取未改变 PDF 的元数据"
        )]
        refresh_metadata: bool,
    },
    #[command(about = "List papers / 列出文献")]
    List(FilterArgs),
    #[command(about = "Search paper metadata and organization / 搜索文献元数据和分类")]
    Search {
        #[arg(help = "Literal Unicode query / Unicode 字面查询")]
        query: String,
        #[command(flatten)]
        filter: FilterArgs,
    },
    #[command(about = "Show one paper / 显示一篇文献")]
    Show {
        #[arg(help = "Full UUID or unambiguous prefix / 完整 UUID 或无歧义前缀")]
        paper_id: String,
        #[arg(
            long,
            value_enum,
            default_value = "table",
            help = "Output format / 输出格式"
        )]
        format: OutputFormat,
    },
    #[command(about = "Edit manual metadata / 编辑手工元数据")]
    Edit(Box<EditArgs>),
    #[command(about = "Set or clear importance / 设置或清除重要程度")]
    Rate {
        #[arg(help = "Full UUID or unambiguous prefix / 完整 UUID 或无歧义前缀")]
        paper_id: String,
        #[arg(help = "One through five, or clear / 一至五，或 clear")]
        value: String,
    },
    #[command(about = "Manage nested groups / 管理嵌套分组")]
    Group {
        #[command(subcommand)]
        command: GroupCommand,
    },
    #[command(about = "Relocate the PDF root / 更改 PDF 根目录")]
    Root {
        #[command(subcommand)]
        command: RootCommand,
    },
    #[command(about = "Open a PDF in the system viewer / 用系统阅读器打开 PDF")]
    Open {
        #[arg(help = "Full UUID or unambiguous prefix / 完整 UUID 或无歧义前缀")]
        paper_id: String,
    },
    #[command(about = "Create an online backup / 创建在线备份")]
    Backup {
        #[arg(help = "New empty backup directory / 新的空备份目录")]
        destination: PathBuf,
    },
    #[command(about = "Open the local manual / 打开本地手册")]
    Manual,
}

#[derive(Args)]
struct InitArgs {
    #[arg(
        long,
        value_name = "FILE",
        help = "New TOML configuration / 新 TOML 配置文件"
    )]
    config: PathBuf,
    #[arg(
        long,
        value_name = "DIR",
        help = "Existing PDF root / 已存在的 PDF 根目录"
    )]
    root: PathBuf,
    #[arg(
        long,
        value_enum,
        default_value = "system",
        help = "Interface language / 界面语言"
    )]
    language: LanguageArg,
}

#[derive(Args, Default)]
struct FilterArgs {
    #[arg(long, help = "Nested group path / 嵌套分组路径")]
    group: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=5), help = "Exact importance 1-5 / 指定重要程度 1-5")]
    importance: Option<u8>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=5), help = "Minimum importance 1-5 / 最低重要程度 1-5")]
    min_importance: Option<u8>,
    #[arg(long, value_enum, help = "File status / 文件状态")]
    status: Option<StatusArg>,
    #[arg(
        long,
        value_enum,
        default_value = "table",
        help = "Output format / 输出格式"
    )]
    format: OutputFormat,
    #[arg(long, help = "Maximum result count / 最大结果数")]
    limit: Option<usize>,
}

#[derive(Args)]
struct EditArgs {
    #[arg(help = "Full UUID or unambiguous prefix / 完整 UUID 或无歧义前缀")]
    paper_id: String,
    #[arg(long, help = "Manual title / 手工标题")]
    title: Option<String>,
    #[arg(
        long = "author",
        help = "Ordered author; repeatable / 有序作者；可重复"
    )]
    authors: Vec<String>,
    #[arg(long = "abstract", help = "Manual abstract / 手工摘要")]
    abstract_text: Option<String>,
    #[arg(long = "date", help = "Publication date / 发表日期")]
    publication_date: Option<String>,
    #[arg(long, help = "Journal or conference / 期刊或会议")]
    container: Option<String>,
    #[arg(long, help = "Volume / 卷")]
    volume: Option<String>,
    #[arg(long, help = "Issue / 期")]
    issue: Option<String>,
    #[arg(long, help = "Page range / 页码")]
    pages: Option<String>,
    #[arg(long, help = "Digital Object Identifier / DOI")]
    doi: Option<String>,
    #[arg(long, help = "Publication URL / 发表网址")]
    url: Option<String>,
    #[arg(long = "paper-language", help = "Paper language / 文献语言")]
    paper_language: Option<String>,
    #[arg(
        long = "keyword",
        help = "Ordered keyword; repeatable / 有序关键词；可重复"
    )]
    keywords: Vec<String>,
    #[arg(long, help = "Private local notes / 本地备注")]
    notes: Option<String>,
    #[arg(long, value_enum, help = "Set a manual field blank / 将手工字段设为空")]
    clear: Vec<EditableField>,
    #[arg(long, help = "Prompt for common fields / 交互输入常用字段")]
    interactive: bool,
}

#[derive(Subcommand)]
enum GroupCommand {
    #[command(about = "List all group paths / 列出所有分组路径")]
    List,
    #[command(about = "Create a nested path / 创建嵌套路径")]
    Create { path: String },
    #[command(about = "Rename one group / 重命名单个分组")]
    Rename { path: String, new_name: String },
    #[command(about = "Move a group and its descendants / 移动分组及其子分组")]
    Move {
        path: String,
        #[arg(long, conflicts_with = "root")]
        parent: Option<String>,
        #[arg(long)]
        root: bool,
    },
    #[command(about = "Delete a group tree, not PDFs / 删除分组树，不删除 PDF")]
    Delete { path: String },
    #[command(about = "Add papers to a group / 把文献加入分组")]
    Add {
        path: String,
        paper_ids: Vec<String>,
    },
    #[command(about = "Remove papers from a group / 从分组移除文献")]
    Remove {
        path: String,
        paper_ids: Vec<String>,
    },
}

#[derive(Subcommand)]
enum RootCommand {
    #[command(about = "Set a new existing PDF root / 设置新的现有 PDF 根目录")]
    Set { directory: PathBuf },
}

#[derive(Clone, Copy, ValueEnum, Default)]
enum OutputFormat {
    #[default]
    Table,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum StatusArg {
    Present,
    Missing,
    Error,
}

#[derive(Clone, Copy, ValueEnum)]
enum LanguageArg {
    System,
    En,
    #[value(name = "zh-CN")]
    ZhCn,
}

#[derive(Clone, Copy, ValueEnum)]
enum EditableField {
    Title,
    Authors,
    Abstract,
    Date,
    Container,
    Volume,
    Issue,
    Pages,
    Doi,
    Url,
    PaperLanguage,
    Keywords,
    Notes,
}

#[derive(Serialize)]
struct PaperDetails {
    paper: Paper,
    groups: Vec<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let error_language = error_language(&cli);
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("litman: {}", error.localized(error_language));
            match &error {
                LitmanError::PaperNotFound(_)
                | LitmanError::AmbiguousPaperId(_)
                | LitmanError::GroupNotFound(_) => ExitCode::from(3),
                _ => ExitCode::from(4),
            }
        }
    }
}

fn error_language(cli: &Cli) -> Language {
    if let Some(language) = cli.language {
        return language.into();
    }
    if let Command::Init(args) = &cli.command {
        return args.language.into();
    }
    let config_path = cli
        .config
        .clone()
        .or_else(|| env::var_os("LITMAN_CONFIG").map(PathBuf::from))
        .unwrap_or_else(default_config_path);
    Config::load(config_path)
        .map(|config| config.language)
        .unwrap_or(Language::System)
}

fn run(cli: Cli) -> litman_core::Result<()> {
    if let Command::Init(args) = cli.command {
        if !args.root.is_dir() {
            return Err(LitmanError::RootUnavailable(args.root));
        }
        let mut config = Config::new(args.root);
        config.language = args.language.into();
        Library::init(&args.config, config)?;
        println!("{}", args.config.display());
        return Ok(());
    }

    let config_path = cli
        .config
        .or_else(|| env::var_os("LITMAN_CONFIG").map(PathBuf::from))
        .unwrap_or_else(default_config_path);
    let mut library = Library::open(config_path)?;
    let language = cli
        .language
        .map(Into::into)
        .unwrap_or(library.config().language);
    let locale = Locale::new(language);

    match cli.command {
        Command::Init(_) => unreachable!(),
        Command::Scan { refresh_metadata } => {
            let report = library.scan(
                ScanOptions { refresh_metadata },
                None,
                |event| match event {
                    ScanEvent::Started { total } => eprintln!("scan: {total} PDF(s)"),
                    ScanEvent::Processing { current, path } => eprintln!("[{current}] {path}"),
                    ScanEvent::Warning { path, message } => eprintln!("warning: {path}: {message}"),
                    ScanEvent::Finished(_) => {}
                },
            )?;
            println!(
                "{}: discovered={}, added={}, updated={}, moved={}, unchanged={}, missing={}, errors={}",
                locale.text("scan.complete"),
                report.discovered,
                report.added,
                report.updated,
                report.moved,
                report.unchanged,
                report.missing,
                report.errors
            );
        }
        Command::List(filter) => {
            let papers = library.list_papers(&to_filter(None, &filter))?;
            print_papers(&papers, filter.format, locale)?;
        }
        Command::Search { query, filter } => {
            let papers = library.list_papers(&to_filter(Some(query), &filter))?;
            print_papers(&papers, filter.format, locale)?;
        }
        Command::Show { paper_id, format } => {
            let paper = library.get_paper(&paper_id)?;
            let groups = library
                .groups_for_paper(&paper.id)?
                .into_iter()
                .map(|group| library.group_path(group.id))
                .collect::<litman_core::Result<Vec<_>>>()?;
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&PaperDetails { paper, groups })?
                ),
                OutputFormat::Table => print_details(&paper, &groups, locale),
            }
        }
        Command::Edit(args) => {
            let update = edit_update(*args, locale)?;
            let paper = library.update_paper(&update.0, update.1)?;
            print_details(&paper, &[], locale);
        }
        Command::Rate { paper_id, value } => {
            let rating = if value.eq_ignore_ascii_case("clear") {
                None
            } else {
                Some(
                    value
                        .parse::<u8>()
                        .map_err(|_| LitmanError::InvalidImportance)?,
                )
            };
            library.set_importance(&paper_id, rating)?;
        }
        Command::Group { command } => match command {
            GroupCommand::List => {
                for group in library.list_groups()? {
                    println!("{}\t{}", group.id, library.group_path(group.id)?);
                }
            }
            GroupCommand::Create { path } => {
                let group = library.create_group(&path)?;
                println!("{}", library.group_path(group.id)?);
            }
            GroupCommand::Rename { path, new_name } => {
                let group = library.rename_group(&path, &new_name)?;
                println!("{}", library.group_path(group.id)?);
            }
            GroupCommand::Move { path, parent, root } => {
                let parent = if root { None } else { parent.as_deref() };
                let group = library.move_group(&path, parent)?;
                println!("{}", library.group_path(group.id)?);
            }
            GroupCommand::Delete { path } => library.delete_group(&path)?,
            GroupCommand::Add { path, paper_ids } => library.add_to_group(&path, &paper_ids)?,
            GroupCommand::Remove { path, paper_ids } => {
                library.remove_from_group(&path, &paper_ids)?
            }
        },
        Command::Root { command } => match command {
            RootCommand::Set { directory } => {
                if !directory.is_dir() {
                    return Err(LitmanError::RootUnavailable(directory));
                }
                library.set_root(directory)?;
            }
        },
        Command::Open { paper_id } => library.open_pdf(&paper_id)?,
        Command::Backup { destination } => println!("{}", library.backup(destination)?.display()),
        Command::Manual => open_manual(language)?,
    }
    Ok(())
}

fn to_filter(query: Option<String>, args: &FilterArgs) -> ListFilter {
    ListFilter {
        query,
        group_path: args.group.clone(),
        importance: args.importance,
        min_importance: args.min_importance,
        unrated: false,
        status: args.status.map(Into::into),
        limit: args.limit,
    }
}

fn edit_update(args: EditArgs, locale: Locale) -> litman_core::Result<(String, PaperUpdate)> {
    let mut update = PaperUpdate {
        title: args.title.map(Some),
        authors: (!args.authors.is_empty()).then_some(args.authors),
        abstract_text: args.abstract_text.map(Some),
        publication_date: args.publication_date.map(Some),
        container_title: args.container.map(Some),
        volume: args.volume.map(Some),
        issue: args.issue.map(Some),
        pages: args.pages.map(Some),
        doi: args.doi.map(Some),
        url: args.url.map(Some),
        language: args.paper_language.map(Some),
        keywords: (!args.keywords.is_empty()).then_some(args.keywords),
        notes: args.notes.map(Some),
    };
    for field in args.clear {
        match field {
            EditableField::Title => update.title = Some(None),
            EditableField::Authors => update.authors = Some(vec![]),
            EditableField::Abstract => update.abstract_text = Some(None),
            EditableField::Date => update.publication_date = Some(None),
            EditableField::Container => update.container_title = Some(None),
            EditableField::Volume => update.volume = Some(None),
            EditableField::Issue => update.issue = Some(None),
            EditableField::Pages => update.pages = Some(None),
            EditableField::Doi => update.doi = Some(None),
            EditableField::Url => update.url = Some(None),
            EditableField::PaperLanguage => update.language = Some(None),
            EditableField::Keywords => update.keywords = Some(vec![]),
            EditableField::Notes => update.notes = Some(None),
        }
    }
    if args.interactive {
        update.title = prompt(locale.text("title"))?.map(Some);
        update.authors = prompt(locale.text("authors"))?.map(|value| split_values(&value));
        update.abstract_text = prompt(locale.text("abstract"))?.map(Some);
        update.publication_date = prompt(locale.text("date"))?.map(Some);
        update.container_title = prompt(locale.text("container"))?.map(Some);
        update.doi = prompt("DOI")?.map(Some);
        update.keywords = prompt(locale.text("keywords"))?.map(|value| split_values(&value));
        update.notes = prompt(locale.text("notes"))?.map(Some);
    }
    Ok((args.paper_id, update))
}

fn prompt(label: &str) -> litman_core::Result<Option<String>> {
    print!("{label} (blank=keep): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

fn split_values(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn print_papers(papers: &[Paper], format: OutputFormat, locale: Locale) -> litman_core::Result<()> {
    if matches!(format, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(papers)?);
        return Ok(());
    }
    if papers.is_empty() {
        println!("{}", locale.text("cli.no_results"));
        return Ok(());
    }
    println!(
        "ID\t{}\t{}\t{}",
        locale.text("title"),
        locale.text("author"),
        locale.text("year")
    );
    for paper in papers {
        println!(
            "{}\t{}\t{}\t{}",
            paper.id.get(..8).unwrap_or(&paper.id),
            clean_table_cell(&paper.display_title()),
            compact_authors(&paper.authors, locale),
            publication_year(paper.publication_date.as_deref())
        );
    }
    Ok(())
}

fn compact_authors(authors: &[String], locale: Locale) -> String {
    let Some(first) = authors.first() else {
        return "-".into();
    };
    let bundled_first = bundled_first_author(first);
    let first = clean_table_cell(bundled_first.unwrap_or(first));
    let has_multiple_authors = authors.len() > 1 || bundled_first.is_some();
    if !has_multiple_authors {
        first
    } else if locale.0 == Language::ZhCn {
        format!("{first} 等")
    } else {
        format!("{first} et al.")
    }
}

fn bundled_first_author(value: &str) -> Option<&str> {
    if let Some(index) = value.find([';', '\n']) {
        return nonempty_prefix(value, index);
    }

    let lowercase = value.to_ascii_lowercase();
    if let Some(index) = lowercase.find(" and ") {
        return nonempty_prefix(value, index);
    }

    let (first, remainder) = value.split_once(',')?;
    let first = first.trim();
    let next = remainder.split(',').next().unwrap_or_default().trim();
    let comma_count = value.bytes().filter(|byte| *byte == b',').count();
    let explicitly_abbreviated = next
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .starts_with("et al");
    let two_complete_names = looks_like_complete_author(first) && looks_like_complete_author(next);

    (comma_count >= 2 || explicitly_abbreviated || two_complete_names)
        .then_some(first)
        .filter(|first| !first.is_empty())
}

fn nonempty_prefix(value: &str, end: usize) -> Option<&str> {
    let prefix = value[..end].trim();
    (!prefix.is_empty()).then_some(prefix)
}

fn looks_like_complete_author(value: &str) -> bool {
    value.chars().any(char::is_whitespace) || value.contains('.') || !value.is_ascii()
}

fn publication_year(publication_date: Option<&str>) -> String {
    publication_date
        .and_then(|date| {
            date.split(|character: char| !character.is_ascii_digit())
                .find(|part| part.len() >= 4)
        })
        .map(|digits| digits[..4].to_owned())
        .unwrap_or_else(|| "-".into())
}

fn clean_table_cell(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            _ => character,
        })
        .collect()
}

fn print_details(paper: &Paper, groups: &[String], locale: Locale) {
    println!("ID: {}", paper.id);
    println!("{}: {}", locale.text("title"), paper.display_title());
    println!("{}: {}", locale.text("authors"), paper.authors.join("; "));
    println!(
        "{}: {}",
        locale.text("importance"),
        paper
            .importance
            .map(|value| value.to_string())
            .unwrap_or_else(|| locale.text("unrated").into())
    );
    println!("DOI: {}", paper.doi.as_deref().unwrap_or(""));
    println!("{}: {}", locale.text("groups"), groups.join("; "));
    println!("{}: {}", locale.text("file"), paper.relative_path);
    println!(
        "{}: {}",
        locale.text("status"),
        locale.text(paper.file_status.as_str())
    );
}

fn open_manual(language: Language) -> litman_core::Result<()> {
    let locale = if language.resolved() == Language::ZhCn {
        "zh-CN"
    } else {
        "en"
    };
    let mut candidates = Vec::new();
    if let Some(directory) = env::var_os("LITMAN_MANUAL_DIR") {
        candidates.push(PathBuf::from(directory).join(locale).join("index.html"));
    }
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        candidates.push(
            parent
                .join("../share/doc/litman")
                .join(locale)
                .join("index.html"),
        );
        candidates.push(parent.join("manual").join(locale).join("index.html"));
        candidates.push(
            parent
                .join("../Resources/manual")
                .join(locale)
                .join("index.html"),
        );
    }
    candidates.push(Path::new("docs").join(locale).join("book/index.html"));
    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        open::that_detached(path).map_err(|error| LitmanError::Io(io::Error::other(error)))?;
        Ok(())
    } else {
        Err(LitmanError::ConfigNotFound(PathBuf::from(format!(
            "docs/{locale}/book/index.html"
        ))))
    }
}

impl From<LanguageArg> for Language {
    fn from(value: LanguageArg) -> Self {
        match value {
            LanguageArg::System => Self::System,
            LanguageArg::En => Self::En,
            LanguageArg::ZhCn => Self::ZhCn,
        }
    }
}

impl From<StatusArg> for FileStatus {
    fn from(value: StatusArg) -> Self {
        match value {
            StatusArg::Present => Self::Present,
            StatusArg::Missing => Self::Missing,
            StatusArg::Error => Self::Error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_authors_are_compact_and_localized() {
        let authors = vec!["李伟".into(), "Ada Smith".into()];
        assert_eq!(
            compact_authors(&authors, Locale::new(Language::En)),
            "李伟 et al."
        );
        assert_eq!(
            compact_authors(&authors, Locale::new(Language::ZhCn)),
            "李伟 等"
        );
        assert_eq!(
            compact_authors(&authors[..1], Locale::new(Language::En)),
            "李伟"
        );
        assert_eq!(compact_authors(&[], Locale::new(Language::En)), "-");
    }

    #[test]
    fn table_authors_compact_bundled_metadata_lists() {
        for (authors, expected_first) in [
            (
                "Gaia Collaboration, Klioner S. A., Lindegren L.",
                "Gaia Collaboration",
            ),
            ("Simon Ellingsen, Mark Reid", "Simon Ellingsen"),
            ("C. MA, et al.", "C. MA"),
            ("Ada Smith; Grace Hopper", "Ada Smith"),
            ("Ada Smith and Grace Hopper", "Ada Smith"),
        ] {
            assert_eq!(
                compact_authors(&[authors.into()], Locale::new(Language::En)),
                format!("{expected_first} et al.")
            );
        }
        assert_eq!(
            compact_authors(&["刘佳成, 刘牛".into()], Locale::new(Language::ZhCn)),
            "刘佳成 等"
        );
    }

    #[test]
    fn table_authors_preserve_one_inverted_name() {
        assert_eq!(
            compact_authors(&["Smith, Jane".into()], Locale::new(Language::En)),
            "Smith, Jane"
        );
    }

    #[test]
    fn table_year_uses_the_first_four_digit_year() {
        assert_eq!(publication_year(Some("2024-05-17")), "2024");
        assert_eq!(publication_year(Some("March 2023")), "2023");
        assert_eq!(publication_year(None), "-");
    }

    #[test]
    fn table_cells_cannot_create_extra_rows_or_columns() {
        assert_eq!(clean_table_cell("A\tB\nC\rD"), "A B C D");
    }
}
