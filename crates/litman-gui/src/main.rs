#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui;
use egui::{FontData, FontDefinitions, FontFamily};
use egui_extras::{Column, TableBuilder};
use litman_core::{
    Config, FileStatus, Group, Language, Library, ListFilter, LitmanError, Locale, Paper,
    PaperUpdate, PdfReplacementPlan, PdfReplacementResult, ScanEvent, ScanOptions,
    ScixplorerClient, ScixplorerRecord, ScixplorerSearchField, default_config_path,
};

const USER_MANUAL_EN: &str = include_str!("../../../docs/en/src/user.md");
const USER_MANUAL_ZH_CN: &str = include_str!("../../../docs/zh-CN/src/user.md");
const GPL_V3_LICENSE: &str = include_str!("../../../LICENSE");

fn main() -> eframe::Result {
    #[cfg(windows)]
    {
        if let Err(error) = launch(eframe::Renderer::Wgpu) {
            eprintln!("LitMan WGPU initialization failed ({error}); retrying with OpenGL");
            return launch(eframe::Renderer::Glow);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        launch(eframe::Renderer::Glow)
    }
}

fn launch(renderer: eframe::Renderer) -> eframe::Result {
    let config_path = argument_config().or_else(|| {
        let path = default_config_path();
        path.is_file().then_some(path)
    });
    let options = eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_title("LitMan")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([850.0, 540.0]),
        ..Default::default()
    };
    #[cfg(windows)]
    let options = {
        let mut options = options;
        let mut wgpu_options = eframe::WgpuConfiguration::default();
        if matches!(renderer, eframe::Renderer::Wgpu)
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup
        {
            setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
        }
        options.wgpu_options = wgpu_options;
        options
    };
    eframe::run_native(
        "LitMan",
        options,
        Box::new(move |creation| {
            install_fonts(&creation.egui_ctx);
            Ok(Box::new(LitManApp::new(config_path.clone())))
        }),
    )
}

fn install_fonts(context: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "noto-cjk".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/NotoSansCJKsc-Regular.otf"
        ))),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "noto-cjk".into());
    }
    context.set_fonts(fonts);
}

#[derive(Default, Clone)]
struct EditorState {
    paper_id: String,
    title: String,
    authors: String,
    abstract_text: String,
    publication_date: String,
    container_title: String,
    volume: String,
    issue: String,
    pages: String,
    doi: String,
    url: String,
    language: String,
    keywords: String,
    notes: String,
}

impl EditorState {
    fn from_paper(paper: &Paper) -> Self {
        Self {
            paper_id: paper.id.clone(),
            title: paper.title.clone().unwrap_or_default(),
            authors: paper.authors.join("; "),
            abstract_text: paper.abstract_text.clone().unwrap_or_default(),
            publication_date: paper.publication_date.clone().unwrap_or_default(),
            container_title: paper.container_title.clone().unwrap_or_default(),
            volume: paper.volume.clone().unwrap_or_default(),
            issue: paper.issue.clone().unwrap_or_default(),
            pages: paper.pages.clone().unwrap_or_default(),
            doi: paper.doi.clone().unwrap_or_default(),
            url: paper.url.clone().unwrap_or_default(),
            language: paper.language.clone().unwrap_or_default(),
            keywords: paper.keywords.join("; "),
            notes: paper.notes.clone().unwrap_or_default(),
        }
    }

    fn update(&self, original: &Paper) -> PaperUpdate {
        PaperUpdate {
            title: changed(&self.title, original.title.as_deref()),
            authors: changed_list(&self.authors, &original.authors),
            abstract_text: changed(&self.abstract_text, original.abstract_text.as_deref()),
            publication_date: changed(&self.publication_date, original.publication_date.as_deref()),
            container_title: changed(&self.container_title, original.container_title.as_deref()),
            volume: changed(&self.volume, original.volume.as_deref()),
            issue: changed(&self.issue, original.issue.as_deref()),
            pages: changed(&self.pages, original.pages.as_deref()),
            doi: changed(&self.doi, original.doi.as_deref()),
            url: changed(&self.url, original.url.as_deref()),
            language: changed(&self.language, original.language.as_deref()),
            keywords: changed_list(&self.keywords, &original.keywords),
            notes: changed(&self.notes, original.notes.as_deref()),
        }
    }
}

enum WorkerMessage {
    Event(ScanEvent),
    Done(std::result::Result<(), String>),
}

enum ScixplorerWorkerMessage {
    Search(litman_core::Result<Vec<ScixplorerRecord>>),
    Bibtex {
        paper_id: String,
        bibcode: String,
        result: litman_core::Result<String>,
    },
}

enum PdfReplacementWorkerMessage {
    Done(litman_core::Result<PdfReplacementResult>),
}

struct PdfReplacementWindowState {
    plan: PdfReplacementPlan,
    acknowledged: bool,
    busy: bool,
    browser_fallback: bool,
    error: String,
}

struct ScixplorerWindowState {
    paper_id: String,
    field: ScixplorerSearchField,
    query: String,
    results: Vec<ScixplorerRecord>,
    busy: bool,
    error: String,
}

struct RenameGroupState {
    original_path: String,
    name: String,
    warning: String,
    request_focus: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Importance,
    Title,
    Authors,
    Date,
    Status,
}

struct LitManApp {
    library: Option<Library>,
    config_path: Option<PathBuf>,
    locale: Locale,
    papers: Vec<Paper>,
    groups: Vec<Group>,
    selected: HashSet<String>,
    editor: EditorState,
    query: String,
    group_filter: Option<String>,
    importance_filter: Option<u8>,
    unrated_filter: bool,
    sort_column: SortColumn,
    sort_ascending: bool,
    status_filter: Option<FileStatus>,
    new_group_path: String,
    assignment_group: Option<String>,
    reset_field: String,
    message: String,
    scan_receiver: Option<Receiver<WorkerMessage>>,
    scan_cancel: Option<Arc<AtomicBool>>,
    scan_progress: Option<(usize, usize, String)>,
    auto_scan_pending: bool,
    confirm_delete: bool,
    confirm_group_delete: Option<String>,
    rename_group: Option<RenameGroupState>,
    scixplorer: Option<ScixplorerWindowState>,
    scixplorer_receiver: Option<Receiver<ScixplorerWorkerMessage>>,
    pdf_replacement: Option<PdfReplacementWindowState>,
    pdf_replacement_receiver: Option<Receiver<PdfReplacementWorkerMessage>>,
    show_scixplorer_settings: bool,
    scixplorer_token_input: String,
    show_about: bool,
}

impl LitManApp {
    fn new(config_path: Option<PathBuf>) -> Self {
        let mut app = Self {
            library: None,
            config_path: None,
            locale: Locale::new(Language::System),
            papers: vec![],
            groups: vec![],
            selected: HashSet::new(),
            editor: EditorState::default(),
            query: String::new(),
            group_filter: None,
            importance_filter: None,
            unrated_filter: false,
            sort_column: SortColumn::Importance,
            sort_ascending: false,
            status_filter: None,
            new_group_path: String::new(),
            assignment_group: None,
            reset_field: "title".into(),
            message: String::new(),
            scan_receiver: None,
            scan_cancel: None,
            scan_progress: None,
            auto_scan_pending: false,
            confirm_delete: false,
            confirm_group_delete: None,
            rename_group: None,
            scixplorer: None,
            scixplorer_receiver: None,
            pdf_replacement: None,
            pdf_replacement_receiver: None,
            show_scixplorer_settings: false,
            scixplorer_token_input: String::new(),
            show_about: false,
        };
        if let Some(path) = config_path {
            app.open_library(path);
        }
        app
    }

    fn open_library(&mut self, path: PathBuf) {
        match Library::open(&path) {
            Ok(library) => {
                self.locale = Locale::new(library.config().language);
                self.config_path = Some(path);
                self.library = Some(library);
                self.group_filter = None;
                self.assignment_group = None;
                self.confirm_group_delete = None;
                self.rename_group = None;
                self.scixplorer = None;
                self.scixplorer_receiver = None;
                self.pdf_replacement = None;
                self.pdf_replacement_receiver = None;
                self.show_scixplorer_settings = false;
                self.scixplorer_token_input.clear();
                self.auto_scan_pending = true;
                self.message.clear();
                self.reload();
            }
            Err(error) => self.message = error.localized(self.locale.0),
        }
    }

    fn reload(&mut self) {
        let Some(library) = self.library.as_ref() else {
            return;
        };
        let filter = ListFilter {
            query: (!self.query.trim().is_empty()).then(|| self.query.clone()),
            group_path: self.group_filter.clone(),
            importance: self.importance_filter,
            unrated: self.unrated_filter,
            status: self.status_filter,
            ..Default::default()
        };
        let papers_result = library.list_papers(&filter);
        let groups_result = library.list_groups();
        match papers_result {
            Ok(papers) => {
                self.papers = papers;
                self.sort_papers();
                self.selected
                    .retain(|id| self.papers.iter().any(|paper| &paper.id == id));
            }
            Err(error) => self.message = error.localized(self.locale.0),
        }
        match groups_result {
            Ok(groups) => self.groups = groups,
            Err(error) => self.message = error.localized(self.locale.0),
        }
        if let Some(id) = self.selected.iter().next()
            && let Some(paper) = self.papers.iter().find(|paper| &paper.id == id)
        {
            self.editor = EditorState::from_paper(paper);
        }
    }

    fn change_sort(&mut self, column: SortColumn) {
        if self.sort_column == column {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_column = column;
            self.sort_ascending = column != SortColumn::Importance;
        }
        self.sort_papers();
    }

    fn sort_papers(&mut self) {
        let column = self.sort_column;
        let ascending = self.sort_ascending;
        self.papers.sort_by(|left, right| {
            let ordering = match column {
                SortColumn::Importance => left.importance.cmp(&right.importance),
                SortColumn::Title => left
                    .display_title()
                    .to_lowercase()
                    .cmp(&right.display_title().to_lowercase()),
                SortColumn::Authors => left
                    .authors
                    .join(";")
                    .to_lowercase()
                    .cmp(&right.authors.join(";").to_lowercase()),
                SortColumn::Date => left.publication_date.cmp(&right.publication_date),
                SortColumn::Status => left.file_status.as_str().cmp(right.file_status.as_str()),
            };
            if ascending {
                ordering
            } else {
                ordering.reverse()
            }
            .then_with(|| left.id.cmp(&right.id))
        });
    }

    fn sort_label(&self, column: SortColumn, label: &str) -> String {
        if self.sort_column == column {
            format!("{label} {}", if self.sort_ascending { "▲" } else { "▼" })
        } else {
            label.into()
        }
    }

    fn begin_scan(&mut self, refresh_metadata: bool) {
        if self.scan_receiver.is_some()
            || self.pdf_replacement_receiver.is_some()
            || self.pdf_replacement.is_some()
        {
            return;
        }
        let Some(config_path) = self.config_path.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let language = self.locale.0;
        let cancellation = Arc::new(AtomicBool::new(false));
        let thread_cancellation = cancellation.clone();
        thread::spawn(move || {
            let result = Library::open(config_path)
                .and_then(|mut library| {
                    library.scan(
                        ScanOptions { refresh_metadata },
                        Some(&thread_cancellation),
                        |event| {
                            let _ = sender.send(WorkerMessage::Event(event));
                        },
                    )?;
                    Ok(())
                })
                .map_err(|error| error.localized(language));
            let _ = sender.send(WorkerMessage::Done(result));
        });
        self.scan_receiver = Some(receiver);
        self.scan_cancel = Some(cancellation);
        self.scan_progress = Some((0, 0, String::new()));
    }

    fn poll_scan(&mut self) {
        let mut finished = None;
        if let Some(receiver) = self.scan_receiver.as_ref() {
            while let Ok(message) = receiver.try_recv() {
                match message {
                    WorkerMessage::Event(ScanEvent::Started { total }) => {
                        self.scan_progress = Some((0, total, String::new()));
                    }
                    WorkerMessage::Event(ScanEvent::Processing { current, path }) => {
                        let total = self
                            .scan_progress
                            .as_ref()
                            .map(|value| value.1)
                            .unwrap_or(0);
                        self.scan_progress = Some((current, total, path));
                    }
                    WorkerMessage::Event(ScanEvent::Warning { path, message }) => {
                        self.message = format!("{path}: {message}");
                    }
                    WorkerMessage::Event(ScanEvent::Finished(report)) => {
                        self.message = format!(
                            "{}: +{}, ~{}, moved {}, errors {}",
                            self.locale.text("scan.complete"),
                            report.added,
                            report.updated,
                            report.moved,
                            report.errors
                        );
                    }
                    WorkerMessage::Done(result) => finished = Some(result),
                }
            }
        }
        if let Some(result) = finished {
            if let Err(error) = result {
                self.message = error;
            }
            self.scan_receiver = None;
            self.scan_cancel = None;
            self.scan_progress = None;
            self.reload();
        }
    }

    fn toolbar(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(root, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading(self.locale.text("app.title"));
                ui.separator();
                let replacement_idle =
                    self.pdf_replacement_receiver.is_none() && self.pdf_replacement.is_none();
                if ui
                    .add_enabled(
                        replacement_idle,
                        egui::Button::new(dual(self.locale, "Open library", "打开文献库")),
                    )
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("LitMan TOML", &["toml"])
                        .pick_file()
                {
                    self.open_library(path);
                }
                if ui
                    .add_enabled(
                        replacement_idle,
                        egui::Button::new(dual(self.locale, "New library", "新建文献库")),
                    )
                    .clicked()
                {
                    self.create_library();
                }
                if self.library.is_some() {
                    if ui
                        .add_enabled(
                            replacement_idle,
                            egui::Button::new(self.locale.text("action.scan")),
                        )
                        .clicked()
                    {
                        self.begin_scan(false);
                    }
                    if ui
                        .add_enabled(
                            replacement_idle,
                            egui::Button::new(dual(self.locale, "Refresh metadata", "刷新元数据")),
                        )
                        .clicked()
                    {
                        self.begin_scan(true);
                    }
                    if ui
                        .add_enabled(
                            replacement_idle,
                            egui::Button::new(dual(self.locale, "Relocate root", "更改根目录")),
                        )
                        .clicked()
                    {
                        self.relocate_root();
                    }
                    if ui.button(self.locale.text("action.backup")).clicked() {
                        self.backup();
                    }
                    if ui
                        .button(dual(self.locale, "SciXplorer settings", "SciXplorer 设置"))
                        .clicked()
                    {
                        self.show_scixplorer_settings = true;
                        self.scixplorer_token_input.clear();
                    }
                    if ui.button(self.locale.text("action.manual")).clicked() {
                        self.open_manual();
                    }
                    if ui.button(dual(self.locale, "About", "关于")).clicked() {
                        self.show_about = true;
                    }
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(match self.locale.0 {
                            Language::ZhCn => "简体中文",
                            _ => "English",
                        })
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.locale.0 == Language::En, "English")
                                .clicked()
                            {
                                self.change_language(Language::En);
                            }
                            if ui
                                .selectable_label(self.locale.0 == Language::ZhCn, "简体中文")
                                .clicked()
                            {
                                self.change_language(Language::ZhCn);
                            }
                        });
                }
            });
            if let Some((current, total, path)) = &self.scan_progress {
                ui.horizontal(|ui| {
                    let fraction = if *total == 0 {
                        0.0
                    } else {
                        *current as f32 / *total as f32
                    };
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    ui.label(path);
                    if ui.button(dual(self.locale, "Cancel", "取消")).clicked()
                        && let Some(flag) = &self.scan_cancel
                    {
                        flag.store(true, Ordering::Relaxed);
                    }
                });
            }
            if !self.message.is_empty() {
                ui.label(&self.message);
            }
        });
    }

    fn sidebar(&mut self, root: &mut egui::Ui) {
        let group_paths = self.group_paths();
        egui::Panel::left("groups")
            .resizable(true)
            .default_size(260.0)
            .show(root, |ui| {
                ui.heading(dual(self.locale, "Filter papers", "筛选文献"));
                if ui
                    .selectable_label(
                        self.group_filter.is_none(),
                        dual(self.locale, "All papers", "全部文献"),
                    )
                    .clicked()
                {
                    self.group_filter = None;
                    self.confirm_group_delete = None;
                    self.rename_group = None;
                    self.reload();
                }
                let roots = self
                    .groups
                    .iter()
                    .filter(|group| group.parent_id.is_none())
                    .cloned()
                    .collect::<Vec<_>>();
                for group in roots {
                    self.group_node(ui, &group, 0);
                }
                self.group_controls(ui);
                ui.separator();
                ui.label(dual(
                    self.locale,
                    "New nested group path",
                    "新建嵌套分组路径",
                ));
                ui.text_edit_singleline(&mut self.new_group_path);
                if ui
                    .button(dual(self.locale, "Create group", "创建分组"))
                    .clicked()
                {
                    self.create_group_from_input();
                }
                ui.separator();
                ui.heading(dual(
                    self.locale,
                    "Assign selected papers",
                    "将所选文献加入分组",
                ));
                ui.label(format!(
                    "{}: {}",
                    dual(self.locale, "Selected papers", "已选择文献"),
                    self.selected.len()
                ));
                if group_paths.is_empty() {
                    ui.label(dual(self.locale, "Create a group first.", "请先创建分组。"));
                } else {
                    egui::ComboBox::from_id_salt("assignment-group")
                        .selected_text(
                            self.assignment_group
                                .as_deref()
                                .unwrap_or_else(|| dual(self.locale, "Choose a group", "选择分组")),
                        )
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for path in &group_paths {
                                ui.selectable_value(
                                    &mut self.assignment_group,
                                    Some(path.clone()),
                                    path,
                                );
                            }
                        });
                }
                let target_group = self.assignment_group.clone();
                let can_change_membership = !self.selected.is_empty() && target_group.is_some();
                ui.add_enabled_ui(can_change_membership, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button(dual(self.locale, "Add to group", "加入分组"))
                            .clicked()
                            && let Some(path) = target_group.as_deref()
                        {
                            self.change_group_membership(path, true);
                        }
                        if ui
                            .button(dual(self.locale, "Remove from group", "从分组移除"))
                            .clicked()
                            && let Some(path) = target_group.as_deref()
                        {
                            self.change_group_membership(path, false);
                        }
                    });
                });
                if self.selected.is_empty() {
                    ui.small(dual(
                        self.locale,
                        "Select one or more papers in the center list first.",
                        "请先在中间列表中选择一篇或多篇文献。",
                    ));
                }
                ui.separator();
                ui.heading(self.locale.text("importance"));
                if ui
                    .selectable_label(
                        self.importance_filter.is_none() && !self.unrated_filter,
                        dual(self.locale, "All ratings", "全部重要程度"),
                    )
                    .clicked()
                {
                    self.importance_filter = None;
                    self.unrated_filter = false;
                    self.reload();
                }
                if ui
                    .selectable_label(self.unrated_filter, self.locale.text("unrated"))
                    .clicked()
                {
                    self.importance_filter = None;
                    self.unrated_filter = true;
                    self.reload();
                }
                for rating in (1..=5).rev() {
                    if ui
                        .selectable_label(
                            self.importance_filter == Some(rating),
                            format!("{} {rating}", "★".repeat(rating as usize)),
                        )
                        .clicked()
                    {
                        self.importance_filter = Some(rating);
                        self.unrated_filter = false;
                        self.reload();
                    }
                }
                ui.separator();
                egui::ComboBox::from_label(self.locale.text("status"))
                    .selected_text(
                        self.status_filter
                            .map(|status| self.locale.text(status.as_str()))
                            .unwrap_or_else(|| dual(self.locale, "All", "全部")),
                    )
                    .show_ui(ui, |ui| {
                        for (status, name) in [
                            (None, dual(self.locale, "All", "全部")),
                            (Some(FileStatus::Present), self.locale.text("present")),
                            (Some(FileStatus::Missing), self.locale.text("missing")),
                            (Some(FileStatus::Error), self.locale.text("error")),
                        ] {
                            if ui
                                .selectable_label(self.status_filter == status, name)
                                .clicked()
                            {
                                self.status_filter = status;
                                self.reload();
                            }
                        }
                    });
            });
    }

    fn group_node(&mut self, ui: &mut egui::Ui, group: &Group, depth: usize) {
        let path = self
            .library
            .as_ref()
            .and_then(|library| library.group_path(group.id).ok())
            .unwrap_or_else(|| group.name.clone());
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 12.0);
            if ui
                .selectable_label(self.group_filter.as_deref() == Some(&path), &group.name)
                .clicked()
            {
                self.group_filter = Some(path.clone());
                self.confirm_group_delete = None;
                self.rename_group = None;
                self.reload();
            }
        });
        let children = self
            .groups
            .iter()
            .filter(|candidate| candidate.parent_id == Some(group.id))
            .cloned()
            .collect::<Vec<_>>();
        for child in children {
            self.group_node(ui, &child, depth + 1);
        }
    }

    fn group_controls(&mut self, ui: &mut egui::Ui) {
        let selected_path = self.group_filter.clone();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    selected_path.is_some(),
                    egui::Button::new(dual(self.locale, "Rename group", "重命名分组")),
                )
                .clicked()
                && let Some(path) = selected_path.as_deref()
            {
                self.rename_group = Some(RenameGroupState {
                    original_path: path.to_owned(),
                    name: group_leaf_name(path).to_owned(),
                    warning: String::new(),
                    request_focus: true,
                });
                self.confirm_group_delete = None;
            }
            if ui
                .add_enabled(
                    selected_path.is_some(),
                    egui::Button::new(dual(self.locale, "Delete group", "删除分组")),
                )
                .clicked()
            {
                self.confirm_group_delete = selected_path.clone();
                self.rename_group = None;
            }
        });

        let Some(path) = self.confirm_group_delete.clone() else {
            return;
        };
        let question = if self.locale.0 == Language::ZhCn {
            format!("删除分组树“{path}”？")
        } else {
            format!("Delete group tree “{path}”?")
        };
        ui.label(question);
        ui.small(dual(
            self.locale,
            "This also deletes all nested groups and their assignments. Papers and PDFs are not deleted.",
            "这也会删除所有嵌套分组及其分组关系，但不会删除文献记录或 PDF。",
        ));
        let mut confirm = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            confirm = ui
                .button(dual(self.locale, "Confirm delete", "确认删除"))
                .clicked();
            cancel = ui.button(dual(self.locale, "Cancel", "取消")).clicked();
        });
        if confirm {
            self.delete_group_tree(&path);
        } else if cancel {
            self.confirm_group_delete = None;
        }
    }

    fn group_paths(&self) -> Vec<String> {
        let Some(library) = self.library.as_ref() else {
            return Vec::new();
        };
        let mut paths = self
            .groups
            .iter()
            .filter_map(|group| library.group_path(group.id).ok())
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| path.to_lowercase());
        paths
    }

    fn create_group_from_input(&mut self) {
        let requested_path = self.new_group_path.trim().to_owned();
        let Some(library) = self.library.as_mut() else {
            return;
        };
        let result = (|| -> litman_core::Result<String> {
            if library.group_exists(&requested_path)? {
                return Err(LitmanError::DuplicateGroup);
            }
            let group = library.create_group(&requested_path)?;
            library.group_path(group.id)
        })();

        match result {
            Ok(created_path) => {
                self.assignment_group = Some(created_path.clone());
                self.new_group_path.clear();
                self.confirm_group_delete = None;
                self.message = if self.locale.0 == Language::ZhCn {
                    format!("已创建分组“{created_path}”。")
                } else {
                    format!("Created group “{created_path}”.")
                };
                self.reload();
            }
            Err(error) => self.message = error.localized(self.locale.0),
        }
    }

    fn group_rename_window(&mut self, root: &mut egui::Ui) {
        let Some(mut rename) = self.rename_group.take() else {
            return;
        };
        let locale = self.locale;
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(dual(locale, "Rename group", "重命名分组"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                ui.label(if locale.0 == Language::ZhCn {
                    format!("当前分组：{}", rename.original_path)
                } else {
                    format!("Current group: {}", rename.original_path)
                });
                ui.label(dual(locale, "New group name", "新分组名称"));
                let response = ui.text_edit_singleline(&mut rename.name);
                if response.changed() {
                    rename.warning.clear();
                }
                if rename.request_focus {
                    response.request_focus();
                    rename.request_focus = false;
                }
                submit |=
                    response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if !rename.warning.is_empty() {
                    let warning_color = ui.visuals().warn_fg_color;
                    ui.colored_label(warning_color, &rename.warning);
                }
                ui.horizontal(|ui| {
                    submit |= ui.button(dual(locale, "Rename", "重命名")).clicked();
                    cancel = ui.button(dual(locale, "Cancel", "取消")).clicked();
                });
            });

        if submit {
            if let Err(warning) = self.rename_selected_group(&rename.original_path, &rename.name) {
                rename.warning = warning;
                self.rename_group = Some(rename);
            }
        } else if open && !cancel {
            self.rename_group = Some(rename);
        }
    }

    fn rename_selected_group(
        &mut self,
        original_path: &str,
        new_name: &str,
    ) -> std::result::Result<(), String> {
        let new_name = new_name.trim();
        if new_name.is_empty() || new_name.contains('/') {
            return Err(
                LitmanError::InvalidConfig("invalid group name".into()).localized(self.locale.0)
            );
        }
        let candidate_path = group_path_with_leaf(original_path, new_name);
        let Some(library) = self.library.as_mut() else {
            return Ok(());
        };
        let result = (|| -> litman_core::Result<String> {
            if library.group_exists(&candidate_path)? {
                return Err(LitmanError::DuplicateGroup);
            }
            let group = library.rename_group(original_path, new_name)?;
            library.group_path(group.id)
        })();
        let renamed_path = result.map_err(|error| error.localized(self.locale.0))?;

        if let Some(path) = self.assignment_group.take() {
            self.assignment_group = Some(rewrite_group_path(&path, original_path, &renamed_path));
        }
        if let Some(path) = self.group_filter.take() {
            self.group_filter = Some(rewrite_group_path(&path, original_path, &renamed_path));
        }
        self.confirm_group_delete = None;
        self.message = if self.locale.0 == Language::ZhCn {
            format!("已将分组“{original_path}”重命名为“{renamed_path}”。")
        } else {
            format!("Renamed group “{original_path}” to “{renamed_path}”.")
        };
        self.reload();
        Ok(())
    }

    fn paper_table(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show(root, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(self.locale.text("search"));
                let response = ui.text_edit_singleline(&mut self.query);
                if response.changed() {
                    self.reload();
                }
                ui.label(format!(
                    "{}: {}",
                    self.locale.text("papers"),
                    self.papers.len()
                ));
            });
            ui.small(dual(
                self.locale,
                "Click a row to select it; Ctrl/Command-click selects several. Drag header dividers to resize columns.",
                "单击行可选择文献；按住 Ctrl/Command 单击可多选。拖动表头分隔线可调整列宽。",
            ));
            ui.separator();
            let headers = [
                self.sort_label(SortColumn::Importance, self.locale.text("importance")),
                self.sort_label(SortColumn::Title, self.locale.text("title")),
                self.sort_label(SortColumn::Authors, self.locale.text("authors")),
                self.sort_label(SortColumn::Date, self.locale.text("date")),
                self.sort_label(SortColumn::Status, self.locale.text("status")),
            ];
            let papers = self.papers.clone();
            let selected = &self.selected;
            let locale = self.locale;
            let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
            let mut requested_sort = None;
            let mut clicked_paper = None;

            TableBuilder::new(ui)
                .id_salt("papers-table")
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .auto_shrink([false, false])
                .column(Column::initial(120.0).at_least(80.0).clip(true))
                .column(Column::initial(300.0).at_least(120.0).clip(true))
                .column(Column::initial(220.0).at_least(100.0).clip(true))
                .column(Column::initial(120.0).at_least(80.0).clip(true))
                .column(Column::initial(100.0).at_least(70.0).clip(true))
                .header(24.0, |mut header| {
                    for (index, label) in headers.iter().enumerate() {
                        header.col(|ui| {
                            if ui.button(label).clicked() {
                                requested_sort = Some([
                                    SortColumn::Importance,
                                    SortColumn::Title,
                                    SortColumn::Authors,
                                    SortColumn::Date,
                                    SortColumn::Status,
                                ][index]);
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(24.0, papers.len(), |mut row| {
                        let paper = &papers[row.index()];
                        row.set_selected(selected.contains(&paper.id));
                        let mut text_clicked = false;
                        row.col(|ui| {
                            text_clicked |= ui
                                .add(
                                    egui::Label::new(
                                        paper
                                            .importance
                                            .map(stars)
                                            .unwrap_or_else(|| "-".into()),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .clicked();
                        });
                        row.col(|ui| {
                            let title = paper.display_title();
                            text_clicked |= literature_list_cell(ui, &title).clicked();
                        });
                        row.col(|ui| {
                            let authors = paper.authors.join("; ");
                            text_clicked |= literature_list_cell(ui, &authors).clicked();
                        });
                        row.col(|ui| {
                            text_clicked |= ui
                                .add(
                                    egui::Label::new(
                                        paper.publication_date.as_deref().unwrap_or(""),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .clicked();
                        });
                        row.col(|ui| {
                            text_clicked |= ui
                                .add(
                                    egui::Label::new(locale.text(paper.file_status.as_str()))
                                        .sense(egui::Sense::click()),
                                )
                                .clicked();
                        });
                        if text_clicked || row.response().clicked() {
                            clicked_paper = Some(paper.clone());
                        }
                    });
                });

            if let Some(column) = requested_sort {
                self.change_sort(column);
            }
            if let Some(paper) = clicked_paper {
                self.select_paper(paper, additive);
            }
        });
    }

    fn select_paper(&mut self, paper: Paper, additive: bool) {
        let was_selected = self.selected.contains(&paper.id);
        if !additive {
            self.selected.clear();
        }
        if was_selected && additive {
            self.selected.remove(&paper.id);
        } else {
            self.selected.insert(paper.id.clone());
            self.editor = EditorState::from_paper(&paper);
        }
        self.confirm_delete = false;
    }

    fn details_panel(&mut self, root: &mut egui::Ui) {
        egui::Panel::right("details")
            .resizable(true)
            .default_size(340.0)
            .show(root, |ui| {
                ui.heading(self.locale.text("metadata"));
                if self.editor.paper_id.is_empty() {
                    ui.label(dual(self.locale, "Select a paper", "请选择文献"));
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    field(ui, self.locale.text("title"), &mut self.editor.title, false);
                    field(
                        ui,
                        self.locale.text("authors"),
                        &mut self.editor.authors,
                        false,
                    );
                    field(
                        ui,
                        self.locale.text("abstract"),
                        &mut self.editor.abstract_text,
                        true,
                    );
                    field(
                        ui,
                        self.locale.text("date"),
                        &mut self.editor.publication_date,
                        false,
                    );
                    field(
                        ui,
                        self.locale.text("container"),
                        &mut self.editor.container_title,
                        false,
                    );
                    field(
                        ui,
                        self.locale.text("volume"),
                        &mut self.editor.volume,
                        false,
                    );
                    field(ui, self.locale.text("issue"), &mut self.editor.issue, false);
                    field(ui, self.locale.text("pages"), &mut self.editor.pages, false);
                    field(ui, "DOI", &mut self.editor.doi, false);
                    field(ui, self.locale.text("url"), &mut self.editor.url, false);
                    field(
                        ui,
                        self.locale.text("language"),
                        &mut self.editor.language,
                        false,
                    );
                    field(
                        ui,
                        self.locale.text("keywords"),
                        &mut self.editor.keywords,
                        false,
                    );
                    field(ui, self.locale.text("notes"), &mut self.editor.notes, true);
                    ui.horizontal(|ui| {
                        if ui.button(self.locale.text("action.save")).clicked() {
                            self.save_editor();
                        }
                        if ui.button(self.locale.text("action.open")).clicked()
                            && let Some(library) = self.library.as_ref()
                            && let Err(error) = library.open_pdf(&self.editor.paper_id)
                        {
                            self.message = error.localized(self.locale.0);
                        }
                    });
                    let selected_paper = self
                        .papers
                        .iter()
                        .find(|paper| paper.id == self.editor.paper_id)
                        .cloned();
                    let token_configured = scixplorer_token_configured(self.library.as_ref());
                    ui.horizontal_wrapped(|ui| {
                        let search = ui.add_enabled(
                            token_configured
                                && self.pdf_replacement_receiver.is_none()
                                && self.pdf_replacement.is_none(),
                            egui::Button::new("SciXplorer"),
                        );
                        if search.clicked() {
                            self.open_scixplorer_search();
                        }
                        if !token_configured {
                            search.on_disabled_hover_text(dual(
                                self.locale,
                                "Configure an API token in SciXplorer settings first.",
                                "请先在 SciXplorer 设置中配置 API 令牌。",
                            ));
                        }

                        let bibtex = selected_paper
                            .as_ref()
                            .and_then(|paper| paper.bibtex.as_deref());
                        if ui
                            .add_enabled(bibtex.is_some(), egui::Button::new("BibTeX"))
                            .on_disabled_hover_text(dual(
                                self.locale,
                                "No BibTeX has been imported.",
                                "尚未导入 BibTeX。",
                            ))
                            .clicked()
                            && let Some(bibtex) = bibtex
                        {
                            ui.ctx().copy_text(bibtex.to_owned());
                            self.message = dual(
                                self.locale,
                                "BibTeX copied to the clipboard.",
                                "BibTeX 已复制到剪贴板。",
                            )
                            .into();
                        }

                        let has_bibcode = selected_paper
                            .as_ref()
                            .is_some_and(|paper| paper.bibcode.is_some());
                        if ui
                            .add_enabled(
                                has_bibcode,
                                egui::Button::new(dual(
                                    self.locale,
                                    "Open SciXplorer",
                                    "打开 SciXplorer",
                                )),
                            )
                            .on_disabled_hover_text(dual(
                                self.locale,
                                "No ADS bibcode has been imported.",
                                "尚未导入 ADS bibcode。",
                            ))
                            .clicked()
                            && let Some(library) = self.library.as_ref()
                            && let Err(error) = library.open_scixplorer(&self.editor.paper_id)
                        {
                            self.message = error.localized(self.locale.0);
                        }

                        let can_replace = pdf_update_enabled(
                            self.selected.len(),
                            selected_paper
                                .as_ref()
                                .is_some_and(|paper| paper.file_status == FileStatus::Present),
                            selected_paper.as_ref().is_some_and(|paper| {
                                paper
                                    .bibcode
                                    .as_deref()
                                    .is_some_and(|value| !value.is_empty())
                            }),
                            self.scan_receiver.is_some(),
                            self.pdf_replacement_receiver.is_some()
                                || self.pdf_replacement.is_some(),
                        );
                        if ui
                            .add_enabled(
                                can_replace,
                                egui::Button::new(dual(
                                    self.locale,
                                    "Update PDF",
                                    "更新 PDF",
                                )),
                            )
                            .on_disabled_hover_text(dual(
                                self.locale,
                                "Select one present paper with an imported ADS bibcode; scanning and replacement must be idle.",
                                "请选择一篇存在且已导入 ADS bibcode 的文献；扫描和替换任务必须处于空闲状态。",
                            ))
                            .clicked()
                        {
                            self.open_pdf_replacement_warning();
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(self.locale.text("importance"));
                        if ui.button("×").clicked() {
                            self.rate_selected(None);
                        }
                        for rating in 1..=5 {
                            if ui.button(stars(rating)).clicked() {
                                self.rate_selected(Some(rating));
                            }
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("reset-field")
                            .selected_text(metadata_field_label(self.locale, &self.reset_field))
                            .show_ui(ui, |ui| {
                                for value in [
                                    "title",
                                    "authors",
                                    "abstract_text",
                                    "publication_date",
                                    "container_title",
                                    "volume",
                                    "issue",
                                    "pages",
                                    "doi",
                                    "url",
                                    "language",
                                    "keywords",
                                ] {
                                    ui.selectable_value(
                                        &mut self.reset_field,
                                        value.into(),
                                        metadata_field_label(self.locale, value),
                                    );
                                }
                            });
                        if ui
                            .button(dual(self.locale, "Reset to PDF", "恢复 PDF 元数据"))
                            .clicked()
                            && let Some(library) = self.library.as_mut()
                        {
                            match library.reset_field(&self.editor.paper_id, &self.reset_field) {
                                Ok(paper) => {
                                    self.editor = EditorState::from_paper(&paper);
                                    self.reload();
                                }
                                Err(error) => self.message = error.localized(self.locale.0),
                            }
                        }
                    });
                    if let Some(paper) = self
                        .papers
                        .iter()
                        .find(|paper| paper.id == self.editor.paper_id)
                    {
                        ui.separator();
                        ui.label(format!(
                            "{}: {}",
                            self.locale.text("file"),
                            paper.relative_path
                        ));
                        ui.label(format!(
                            "{}: {}",
                            self.locale.text("status"),
                            self.locale.text(paper.file_status.as_str())
                        ));
                        ui.label(format!(
                            "{}: {}",
                            self.locale.text("pages"),
                            paper
                                .page_count
                                .map(|value| value.to_string())
                                .unwrap_or_default()
                        ));
                        ui.label(format!(
                            "PDF: {}",
                            paper.pdf_version.as_deref().unwrap_or("")
                        ));
                        ui.label(format!("BLAKE3: {}", paper.content_hash));
                        if let Some(duplicate) = &paper.duplicate_of {
                            ui.label(format!(
                                "{}: {}",
                                dual(self.locale, "Duplicate of", "副本来源"),
                                duplicate
                            ));
                        }
                        if let Some(error) = &paper.scan_error {
                            ui.colored_label(egui::Color32::RED, error);
                        }
                        ui.collapsing(
                            dual(self.locale, "Import provenance", "导入来源"),
                            |ui| {
                                for field_name in [
                                    "title",
                                    "authors",
                                    "abstract_text",
                                    "publication_date",
                                    "container_title",
                                    "volume",
                                    "issue",
                                    "pages",
                                    "doi",
                                    "url",
                                    "language",
                                    "keywords",
                                    "notes",
                                ] {
                                    let source = if paper.bibtex_fields.contains(field_name) {
                                        "SciXplorer/BibTeX"
                                    } else if paper.manual_overrides.contains(field_name) {
                                        dual(self.locale, "manual", "手工")
                                    } else {
                                        match paper
                                            .embedded
                                            .field_sources
                                            .get(field_name)
                                            .map(String::as_str)
                                        {
                                            Some("xmp") => "XMP",
                                            Some("pdf_info") => "PDF Info",
                                            _ => dual(self.locale, "blank", "空白"),
                                        }
                                    };
                                    ui.label(format!(
                                        "{}: {source}",
                                        metadata_field_label(self.locale, field_name)
                                    ));
                                }
                                ui.collapsing("PDF Info", |ui| {
                                    for (key, values) in &paper.embedded.raw_info {
                                        ui.label(format!("{key}: {}", values.join("; ")));
                                    }
                                });
                                ui.collapsing("XMP / Dublin Core / PRISM", |ui| {
                                    for (key, values) in &paper.embedded.raw_xmp {
                                        ui.label(format!("{key}: {}", values.join("; ")));
                                    }
                                });
                            },
                        );
                    }
                    ui.separator();
                    if !self.confirm_delete {
                        if ui
                            .add_enabled(
                                self.pdf_replacement.is_none()
                                    && self.pdf_replacement_receiver.is_none(),
                                egui::Button::new(dual(
                                    self.locale,
                                    "Remove database record",
                                    "移除数据库记录",
                                )),
                            )
                            .clicked()
                        {
                            self.confirm_delete = true;
                        }
                    } else {
                        ui.colored_label(
                            egui::Color32::RED,
                            dual(
                                self.locale,
                                "The PDF will not be deleted.",
                                "PDF 文件不会被删除。",
                            ),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .button(dual(self.locale, "Confirm remove", "确认移除"))
                                .clicked()
                            {
                                self.remove_selected_record();
                            }
                            if ui.button(dual(self.locale, "Cancel", "取消")).clicked() {
                                self.confirm_delete = false;
                            }
                        });
                    }
                });
            });
    }

    fn create_library(&mut self) {
        let Some(config_path) = rfd::FileDialog::new()
            .set_file_name("library.toml")
            .add_filter("TOML", &["toml"])
            .save_file()
        else {
            return;
        };
        let Some(root) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        match Library::init(&config_path, Config::new(root)) {
            Ok(library) => {
                self.config_path = Some(config_path);
                self.locale = Locale::new(library.config().language);
                self.library = Some(library);
                self.assignment_group = None;
                self.auto_scan_pending = true;
                self.reload();
            }
            Err(error) => self.message = error.localized(self.locale.0),
        }
    }

    fn relocate_root(&mut self) {
        let Some(root) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        if let Some(library) = self.library.as_mut() {
            match library.set_root(root) {
                Ok(()) => self.begin_scan(false),
                Err(error) => self.message = error.localized(self.locale.0),
            }
        }
    }

    fn backup(&mut self) {
        let Some(destination) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        if let Some(library) = self.library.as_ref() {
            match library.backup(destination) {
                Ok(path) => self.message = path.display().to_string(),
                Err(error) => self.message = error.localized(self.locale.0),
            }
        }
    }

    fn save_editor(&mut self) {
        let Some(original) = self
            .papers
            .iter()
            .find(|paper| paper.id == self.editor.paper_id)
            .cloned()
        else {
            return;
        };
        let update = self.editor.update(&original);
        if let Some(library) = self.library.as_mut() {
            match library.update_paper(&original.id, update) {
                Ok(paper) => {
                    self.editor = EditorState::from_paper(&paper);
                    self.reload();
                }
                Err(error) => self.message = error.localized(self.locale.0),
            }
        }
    }

    fn rate_selected(&mut self, rating: Option<u8>) {
        if let Some(library) = self.library.as_mut() {
            for id in self.selected.clone() {
                if let Err(error) = library.set_importance(&id, rating) {
                    self.message = error.localized(self.locale.0);
                    break;
                }
            }
            self.reload();
        }
    }

    fn change_group_membership(&mut self, path: &str, add: bool) {
        let ids = self.selected.iter().cloned().collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        let paper_count = ids.len();
        if let Some(library) = self.library.as_mut() {
            let result = if add {
                library.add_to_group(path, &ids)
            } else {
                library.remove_from_group(path, &ids)
            };
            match result {
                Ok(()) if add => {
                    self.message = if self.locale.0 == Language::ZhCn {
                        format!("已将 {paper_count} 篇所选文献加入“{path}”。")
                    } else {
                        format!("Added {paper_count} selected paper(s) to “{path}”.")
                    };
                }
                Ok(()) => {
                    self.message = if self.locale.0 == Language::ZhCn {
                        format!("已从“{path}”移除 {paper_count} 篇所选文献。")
                    } else {
                        format!("Removed {paper_count} selected paper(s) from “{path}”.")
                    };
                }
                Err(error) => self.message = error.localized(self.locale.0),
            }
            self.reload();
        }
    }

    fn delete_group_tree(&mut self, path: &str) {
        let Some(library) = self.library.as_mut() else {
            return;
        };
        match library.delete_group(path) {
            Ok(()) => {
                if self
                    .assignment_group
                    .as_deref()
                    .is_some_and(|candidate| group_path_is_in_tree(candidate, path))
                {
                    self.assignment_group = None;
                }
                self.group_filter = None;
                self.confirm_group_delete = None;
                self.message = if self.locale.0 == Language::ZhCn {
                    format!("已删除分组树“{path}”。文献记录和 PDF 均未删除。")
                } else {
                    format!("Deleted group tree “{path}”. Papers and PDFs were not deleted.")
                };
                self.reload();
            }
            Err(error) => {
                self.confirm_group_delete = None;
                self.message = error.localized(self.locale.0);
            }
        }
    }

    fn remove_selected_record(&mut self) {
        let id = self.editor.paper_id.clone();
        if let Some(library) = self.library.as_mut() {
            match library.remove_paper(&id) {
                Ok(()) => {
                    self.selected.remove(&id);
                    self.editor = EditorState::default();
                    self.confirm_delete = false;
                    self.reload();
                }
                Err(error) => self.message = error.localized(self.locale.0),
            }
        }
    }

    fn change_language(&mut self, language: Language) {
        if let Some(library) = self.library.as_mut()
            && let Err(error) = library.set_language(language)
        {
            self.message = error.localized(self.locale.0);
            return;
        }
        self.locale = Locale::new(language);
    }

    fn open_manual(&mut self) {
        let locale = if self.locale.0 == Language::ZhCn {
            "zh-CN"
        } else {
            "en"
        };
        let candidates = manual_candidates(locale);
        if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
            if let Err(error) = open::that_detached(path) {
                self.message = error.to_string();
            }
        } else {
            match write_embedded_manual(locale)
                .and_then(|path| open::that_detached(path).map_err(|error| error.to_string()))
            {
                Ok(()) => self.message.clear(),
                Err(error) => {
                    self.message = format!(
                        "{}: {error}",
                        dual(
                            self.locale,
                            "Could not open the embedded offline manual",
                            "无法打开内置离线手册",
                        )
                    );
                }
            }
        }
    }

    fn open_pdf_replacement_warning(&mut self) {
        if self.selected.len() != 1
            || self.scan_receiver.is_some()
            || self.pdf_replacement_receiver.is_some()
            || self.pdf_replacement.is_some()
        {
            return;
        }
        let Some(library) = self.library.as_ref() else {
            return;
        };
        match library.pdf_replacement_plan(&self.editor.paper_id) {
            Ok(plan) => {
                self.pdf_replacement = Some(PdfReplacementWindowState {
                    plan,
                    acknowledged: false,
                    busy: false,
                    browser_fallback: false,
                    error: String::new(),
                });
            }
            Err(error) => self.message = error.localized(self.locale.0),
        }
    }

    fn start_pdf_replacement_worker(&mut self, source_path: Option<PathBuf>) {
        let Some(config_path) = self.config_path.clone() else {
            return;
        };
        let Some(state) = self.pdf_replacement.as_mut() else {
            return;
        };
        state.busy = true;
        state.browser_fallback = false;
        state.error.clear();
        let plan = state.plan.clone();
        let (sender, receiver) = mpsc::channel();
        self.pdf_replacement_receiver = Some(receiver);
        thread::spawn(move || {
            let result = Library::open(config_path).and_then(|mut library| match source_path {
                Some(path) => library.replace_pdf_from_file_with_plan(&plan, path),
                None => library.replace_pdf_from_scixplorer_with_plan(&plan),
            });
            let _ = sender.send(PdfReplacementWorkerMessage::Done(result));
        });
    }

    fn poll_pdf_replacement(&mut self) {
        let Some(receiver) = self.pdf_replacement_receiver.as_ref() else {
            return;
        };
        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pdf_replacement_receiver = None;
                if let Some(state) = self.pdf_replacement.as_mut() {
                    state.busy = false;
                    state.error = dual(
                        self.locale,
                        "PDF replacement worker stopped unexpectedly.",
                        "PDF 替换后台任务意外停止。",
                    )
                    .into();
                }
                return;
            }
        };
        self.pdf_replacement_receiver = None;
        match message {
            PdfReplacementWorkerMessage::Done(Ok(result)) => {
                let backups = result
                    .backup_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                self.message = if self.locale.0 == Language::ZhCn {
                    format!(
                        "PDF 已替换。当前文件：{}；备份：{}",
                        result.active_path.display(),
                        backups
                    )
                } else {
                    format!(
                        "PDF replaced. Active: {}; backups: {}",
                        result.active_path.display(),
                        backups
                    )
                };
                self.pdf_replacement = None;
                self.reload();
            }
            PdfReplacementWorkerMessage::Done(Err(LitmanError::PublisherPdfBrowserRequired {
                ..
            })) => {
                if let Some(state) = self.pdf_replacement.as_mut() {
                    state.busy = false;
                    state.browser_fallback = true;
                    state.error = dual(
                        self.locale,
                        "The publisher returned a login/HTML page. Open the publisher link, download the PDF, then select it here.",
                        "出版商返回了登录/HTML 页面。请打开出版商链接，下载 PDF 后在此选择。",
                    )
                    .into();
                }
            }
            PdfReplacementWorkerMessage::Done(Err(error)) => {
                if let Some(state) = self.pdf_replacement.as_mut() {
                    state.busy = false;
                    state.error = error.localized(self.locale.0);
                }
            }
        }
    }

    fn pdf_replacement_window(&mut self, root: &mut egui::Ui) {
        let Some(mut state) = self.pdf_replacement.take() else {
            return;
        };
        let locale = self.locale;
        let mut open = true;
        let mut replace = false;
        let mut open_publisher = false;
        let mut select_download = false;
        let mut cancel = false;
        egui::Window::new(dual(locale, "Replace PDF", "替换 PDF"))
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(true)
            .default_width(650.0)
            .show(root.ctx(), |ui| {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    dual(
                        locale,
                        "Warning: this action replaces files on disk.",
                        "警告：此操作会替换磁盘上的文件。",
                    ),
                );
                ui.label(pdf_replacement_warning(locale));
                ui.separator();
                for (index, movement) in state.plan.backup_moves.iter().enumerate() {
                    ui.label(if index == 0 {
                        dual(locale, "Current selected PDF → backup", "当前选中 PDF → 备份")
                    } else {
                        dual(locale, "Existing untracked target → additional backup", "现有未跟踪目标 → 额外备份")
                    });
                    ui.monospace(format!(
                        "{}\n→ {}",
                        movement.source_path.display(),
                        movement.backup_path.display()
                    ));
                }
                ui.label(dual(locale, "Final published PDF", "最终正式版 PDF"));
                ui.monospace(state.plan.active_path.display().to_string());
                ui.separator();
                ui.checkbox(
                    &mut state.acknowledged,
                    dual(
                        locale,
                        "I understand that files and the database path will change and recovery is manual.",
                        "我了解文件和数据库路径将改变，且恢复需要手工完成。",
                    ),
                );
                if state.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(dual(
                            locale,
                            "Downloading, validating, and replacing PDF…",
                            "正在下载、验证并替换 PDF…",
                        ));
                    });
                }
                if !state.error.is_empty() {
                    ui.colored_label(egui::Color32::RED, &state.error);
                }
                if state.browser_fallback {
                    ui.horizontal_wrapped(|ui| {
                        open_publisher = ui
                            .button(dual(locale, "Open publisher link", "打开出版商链接"))
                            .clicked();
                        select_download = ui
                            .button(dual(locale, "Select downloaded PDF", "选择已下载的 PDF"))
                            .clicked();
                        cancel = ui.button(dual(locale, "Cancel", "取消")).clicked();
                    });
                } else {
                    ui.horizontal(|ui| {
                        replace = ui
                            .add_enabled(
                                pdf_replacement_confirmation_enabled(
                                    state.acknowledged,
                                    state.busy,
                                ),
                                egui::Button::new(
                                    egui::RichText::new(dual(locale, "Replace PDF", "替换 PDF"))
                                        .color(egui::Color32::WHITE),
                                )
                                .fill(egui::Color32::DARK_RED),
                            )
                            .clicked();
                        cancel = ui
                            .add_enabled(
                                !state.busy,
                                egui::Button::new(dual(locale, "Cancel", "取消")),
                            )
                            .clicked();
                    });
                }
            });

        if open_publisher && let Err(error) = open::that_detached(&state.plan.gateway_url) {
            state.error = error.to_string();
        }
        if select_download
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .pick_file()
        {
            self.pdf_replacement = Some(state);
            self.start_pdf_replacement_worker(Some(path));
            return;
        }
        if replace {
            self.pdf_replacement = Some(state);
            self.start_pdf_replacement_worker(None);
        } else if open && !cancel {
            self.pdf_replacement = Some(state);
        }
    }

    fn open_scixplorer_search(&mut self) {
        if !scixplorer_token_configured(self.library.as_ref()) {
            self.message = LitmanError::MissingScixplorerToken.localized(self.locale.0);
            return;
        }
        let Some(paper) = self
            .papers
            .iter()
            .find(|paper| paper.id == self.editor.paper_id)
        else {
            return;
        };
        self.scixplorer = Some(ScixplorerWindowState {
            paper_id: paper.id.clone(),
            field: ScixplorerSearchField::Title,
            query: paper.title.clone().unwrap_or_else(|| paper.display_title()),
            results: Vec::new(),
            busy: false,
            error: String::new(),
        });
    }

    fn start_scixplorer_search(
        &mut self,
        client: ScixplorerClient,
        field: ScixplorerSearchField,
        query: String,
    ) {
        let (sender, receiver) = mpsc::channel();
        self.scixplorer_receiver = Some(receiver);
        thread::spawn(move || {
            let result = client.search(field, &query, 20);
            let _ = sender.send(ScixplorerWorkerMessage::Search(result));
        });
    }

    fn start_scixplorer_import(
        &mut self,
        client: ScixplorerClient,
        paper_id: String,
        bibcode: String,
    ) {
        let (sender, receiver) = mpsc::channel();
        self.scixplorer_receiver = Some(receiver);
        thread::spawn(move || {
            let result = client.bibtex(&bibcode);
            let _ = sender.send(ScixplorerWorkerMessage::Bibtex {
                paper_id,
                bibcode,
                result,
            });
        });
    }

    fn poll_scixplorer(&mut self) {
        let Some(receiver) = self.scixplorer_receiver.as_ref() else {
            return;
        };
        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.scixplorer_receiver = None;
                if let Some(state) = self.scixplorer.as_mut() {
                    state.busy = false;
                    state.error = dual(
                        self.locale,
                        "SciXplorer worker stopped unexpectedly.",
                        "SciXplorer 后台任务意外停止。",
                    )
                    .into();
                }
                return;
            }
        };
        self.scixplorer_receiver = None;
        if let Some(state) = self.scixplorer.as_mut() {
            state.busy = false;
        }
        match message {
            ScixplorerWorkerMessage::Search(Ok(results)) => {
                if let Some(state) = self.scixplorer.as_mut() {
                    state.results = results;
                    state.error.clear();
                }
            }
            ScixplorerWorkerMessage::Search(Err(error)) => {
                if let Some(state) = self.scixplorer.as_mut() {
                    state.error = error.localized(self.locale.0);
                }
            }
            ScixplorerWorkerMessage::Bibtex {
                paper_id,
                bibcode,
                result: Ok(bibtex),
            } => {
                let imported = self
                    .library
                    .as_mut()
                    .ok_or_else(|| LitmanError::InvalidConfig("no open library".into()))
                    .and_then(|library| library.store_bibtex(&paper_id, &bibtex));
                match imported {
                    Ok(paper) => {
                        if self.editor.paper_id == paper.id {
                            self.editor = EditorState::from_paper(&paper);
                        }
                        self.message = if self.locale.0 == Language::ZhCn {
                            format!("已从 SciXplorer 导入 {bibcode} 的 BibTeX 和元数据。")
                        } else {
                            format!("Imported BibTeX and metadata for {bibcode} from SciXplorer.")
                        };
                        self.scixplorer = None;
                        self.reload();
                    }
                    Err(error) => {
                        if let Some(state) = self.scixplorer.as_mut() {
                            state.error = error.localized(self.locale.0);
                        }
                    }
                }
            }
            ScixplorerWorkerMessage::Bibtex {
                result: Err(error), ..
            } => {
                if let Some(state) = self.scixplorer.as_mut() {
                    state.error = error.localized(self.locale.0);
                }
            }
        }
    }

    fn scixplorer_window(&mut self, root: &mut egui::Ui) {
        let Some(mut state) = self.scixplorer.take() else {
            return;
        };
        let locale = self.locale;
        let mut open = true;
        let mut search = false;
        let mut import_bibcode = None;
        let previous_field = state.field;
        egui::Window::new(dual(locale, "Search SciXplorer", "搜索 SciXplorer"))
            .open(&mut open)
            .default_width(680.0)
            .default_height(520.0)
            .resizable(true)
            .show(root.ctx(), |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("scixplorer-search-field")
                        .selected_text(scixplorer_field_label(locale, state.field))
                        .show_ui(ui, |ui| {
                            for field in [
                                ScixplorerSearchField::Title,
                                ScixplorerSearchField::Doi,
                                ScixplorerSearchField::Bibcode,
                            ] {
                                ui.selectable_value(
                                    &mut state.field,
                                    field,
                                    scixplorer_field_label(locale, field),
                                );
                            }
                        });
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut state.query).desired_width(f32::INFINITY),
                    );
                    if ui
                        .add_enabled(
                            !state.busy && !state.query.trim().is_empty(),
                            egui::Button::new(self.locale.text("search")),
                        )
                        .clicked()
                        || (!state.busy
                            && !state.query.trim().is_empty()
                            && response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                    {
                        search = true;
                    }
                });
                if state.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(dual(locale, "Contacting ADS…", "正在连接 ADS…"));
                    });
                }
                if !state.error.is_empty() {
                    ui.colored_label(egui::Color32::RED, &state.error);
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if !state.busy && state.results.is_empty() && state.error.is_empty() {
                        ui.label(dual(
                            locale,
                            "Search by title, DOI, or ADS/SciXplorer bibcode.",
                            "可按标题、DOI 或 ADS/SciXplorer bibcode 搜索。",
                        ));
                    }
                    for record in &state.results {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.strong(if record.title.is_empty() {
                                        &record.bibcode
                                    } else {
                                        &record.title
                                    });
                                    ui.label(format!(
                                        "{} · {}",
                                        record.bibcode,
                                        record.publication_date.as_deref().unwrap_or("-")
                                    ));
                                    if !record.authors.is_empty() {
                                        let shown = record
                                            .authors
                                            .iter()
                                            .take(3)
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join("; ");
                                        let suffix = if record.authors.len() > 3 {
                                            "; …"
                                        } else {
                                            ""
                                        };
                                        ui.label(format!("{shown}{suffix}"));
                                    }
                                    if let Some(doi) = &record.doi {
                                        ui.label(format!("DOI: {doi}"));
                                    }
                                });
                                if ui
                                    .add_enabled(
                                        !state.busy,
                                        egui::Button::new(dual(locale, "Use", "使用")),
                                    )
                                    .clicked()
                                {
                                    import_bibcode = Some(record.bibcode.clone());
                                }
                            });
                        });
                    }
                });
            });

        if state.field != previous_field
            && let Some(paper) = self.papers.iter().find(|paper| paper.id == state.paper_id)
        {
            state.query = match state.field {
                ScixplorerSearchField::Title => {
                    paper.title.clone().unwrap_or_else(|| paper.display_title())
                }
                ScixplorerSearchField::Doi => paper.doi.clone().unwrap_or_default(),
                ScixplorerSearchField::Bibcode => paper.bibcode.clone().unwrap_or_default(),
            };
        }

        if search {
            state.busy = true;
            state.error.clear();
            state.results.clear();
            match self
                .library
                .as_ref()
                .ok_or_else(|| LitmanError::InvalidConfig("no open library".into()))
                .and_then(Library::scixplorer_client)
            {
                Ok(client) => {
                    self.start_scixplorer_search(client, state.field, state.query.clone())
                }
                Err(error) => {
                    state.busy = false;
                    state.error = error.localized(locale.0);
                }
            }
        }
        if let Some(bibcode) = import_bibcode {
            state.busy = true;
            state.error.clear();
            match self
                .library
                .as_ref()
                .ok_or_else(|| LitmanError::InvalidConfig("no open library".into()))
                .and_then(Library::scixplorer_client)
            {
                Ok(client) => self.start_scixplorer_import(client, state.paper_id.clone(), bibcode),
                Err(error) => {
                    state.busy = false;
                    state.error = error.localized(locale.0);
                }
            }
        }
        if open {
            self.scixplorer = Some(state);
        }
    }

    fn scixplorer_settings_window(&mut self, root: &mut egui::Ui) {
        if !self.show_scixplorer_settings {
            return;
        }
        let locale = self.locale;
        let configured = scixplorer_token_configured(self.library.as_ref());
        let mut open = self.show_scixplorer_settings;
        let mut save = false;
        let mut clear = false;
        let mut get_token = false;
        egui::Window::new(dual(
            locale,
            "SciXplorer API settings",
            "SciXplorer API 设置",
        ))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(480.0)
        .show(root.ctx(), |ui| {
            ui.label(if configured {
                dual(locale, "Token status: configured", "令牌状态：已配置")
            } else {
                dual(locale, "Token status: not configured", "令牌状态：未配置")
            });
            ui.label(dual(
                locale,
                "The personal token is stored as plain text in this library's TOML configuration.",
                "个人令牌将以明文保存在此文献库的 TOML 配置文件中。",
            ));
            ui.label(dual(locale, "New API token", "新 API 令牌"));
            ui.add(
                egui::TextEdit::singleline(&mut self.scixplorer_token_input)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                save = ui
                    .add_enabled(
                        !self.scixplorer_token_input.trim().is_empty(),
                        egui::Button::new(dual(locale, "Save token", "保存令牌")),
                    )
                    .clicked();
                clear = ui
                    .add_enabled(
                        configured,
                        egui::Button::new(dual(locale, "Remove token", "删除令牌")),
                    )
                    .clicked();
                get_token = ui
                    .button(dual(locale, "Get an ADS token", "获取 ADS 令牌"))
                    .clicked();
            });
        });
        self.show_scixplorer_settings = open;

        if save {
            let token = self.scixplorer_token_input.trim().to_owned();
            if let Some(library) = self.library.as_mut() {
                match library.set_scixplorer_api_token(Some(token)) {
                    Ok(()) => {
                        self.scixplorer_token_input.clear();
                        self.message = dual(
                            locale,
                            "SciXplorer API token configured.",
                            "SciXplorer API 令牌已配置。",
                        )
                        .into();
                    }
                    Err(error) => self.message = error.localized(locale.0),
                }
            }
        }
        if clear && let Some(library) = self.library.as_mut() {
            match library.set_scixplorer_api_token(None) {
                Ok(()) => {
                    self.scixplorer_token_input.clear();
                    self.scixplorer = None;
                    self.message = dual(
                        locale,
                        "SciXplorer API token removed.",
                        "SciXplorer API 令牌已删除。",
                    )
                    .into();
                }
                Err(error) => self.message = error.localized(locale.0),
            }
        }
        if get_token
            && let Err(error) =
                open::that_detached("https://ui.adsabs.harvard.edu/#user/settings/token")
        {
            self.message = error.to_string();
        }
    }

    fn about_window(&mut self, root: &mut egui::Ui) {
        if !self.show_about {
            return;
        }
        let locale = self.locale;
        let mut open = self.show_about;
        egui::Window::new(dual(locale, "About LitMan", "关于 LitMan"))
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .resizable(true)
            .show(root.ctx(), |ui| {
                ui.heading(format!("LitMan {}", env!("CARGO_PKG_VERSION")));
                ui.label(format!(
                    "{}: {}",
                    dual(locale, "Author", "作者"),
                    env!("CARGO_PKG_AUTHORS")
                ));
                ui.label(dual(
                    locale,
                    "License: GNU General Public License version 3",
                    "许可证：GNU 通用公共许可证第 3 版",
                ));
                ui.label(dual(
                    locale,
                    "Local-first literature management. Ordinary actions keep PDFs read-only; explicit Update PDF preserves and replaces one selected file after confirmation.",
                    "本地优先的文献管理。普通操作保持 PDF 只读；明确确认“更新 PDF”后会保留并替换一个选中文件。",
                ));
                ui.collapsing(
                    dual(locale, "Full GPLv3 license", "GPLv3 完整许可证"),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(GPL_V3_LICENSE).selectable(true).wrap());
                            });
                    },
                );
            });
        self.show_about = open;
    }
}

impl eframe::App for LitManApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_scan();
        self.poll_scixplorer();
        self.poll_pdf_replacement();
        if self.auto_scan_pending {
            self.auto_scan_pending = false;
            self.begin_scan(false);
        }
        self.toolbar(root);
        if self.library.is_none() {
            egui::CentralPanel::default().show(root, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.heading(self.locale.text("app.title"));
                    ui.label(self.locale.text("no_library"));
                });
            });
        } else {
            self.sidebar(root);
            self.details_panel(root);
            self.paper_table(root);
        }
        self.group_rename_window(root);
        self.scixplorer_window(root);
        self.pdf_replacement_window(root);
        self.scixplorer_settings_window(root);
        self.about_window(root);
        if self.scan_receiver.is_some()
            || self.scixplorer_receiver.is_some()
            || self.pdf_replacement_receiver.is_some()
        {
            root.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, multiline: bool) {
    ui.label(label);
    if multiline {
        ui.add(egui::TextEdit::multiline(value).desired_rows(4));
    } else {
        ui.text_edit_singleline(value);
    }
}

fn changed(value: &str, original: Option<&str>) -> Option<Option<String>> {
    let trimmed = value.trim();
    if trimmed == original.unwrap_or_default().trim() {
        None
    } else if trimmed.is_empty() {
        Some(None)
    } else {
        Some(Some(trimmed.to_owned()))
    }
}

fn changed_list(value: &str, original: &[String]) -> Option<Vec<String>> {
    let values = value
        .split([';', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (values != original).then_some(values)
}

fn stars(rating: u8) -> String {
    "★".repeat(rating as usize)
}

fn literature_list_cell(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Label::new(text)
            .sense(egui::Sense::click())
            // We provide the tooltip below. Egui otherwise adds another one when
            // the table column clips this label, producing duplicate pop-outs.
            .show_tooltip_when_elided(false),
    )
    .on_hover_text(text)
}

fn group_path_is_in_tree(candidate: &str, root: &str) -> bool {
    candidate == root
        || candidate
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn group_leaf_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn group_path_with_leaf(path: &str, new_name: &str) -> String {
    path.rsplit_once('/').map_or_else(
        || new_name.to_owned(),
        |(parent, _)| format!("{parent}/{new_name}"),
    )
}

fn rewrite_group_path(candidate: &str, old_path: &str, new_path: &str) -> String {
    if candidate == old_path {
        new_path.to_owned()
    } else if let Some(suffix) = candidate
        .strip_prefix(old_path)
        .filter(|suffix| suffix.starts_with('/'))
    {
        format!("{new_path}{suffix}")
    } else {
        candidate.to_owned()
    }
}

fn dual(locale: Locale, english: &'static str, chinese: &'static str) -> &'static str {
    if locale.0 == Language::ZhCn {
        chinese
    } else {
        english
    }
}

fn pdf_update_enabled(
    selected_count: usize,
    paper_is_present: bool,
    has_bibcode: bool,
    scan_busy: bool,
    replacement_busy: bool,
) -> bool {
    selected_count == 1 && paper_is_present && has_bibcode && !scan_busy && !replacement_busy
}

fn pdf_replacement_confirmation_enabled(acknowledged: bool, busy: bool) -> bool {
    acknowledged && !busy
}

fn pdf_replacement_warning(locale: Locale) -> &'static str {
    dual(
        locale,
        "Close PDF viewers first. LitMan will move/rename files, change the database path, and install the publisher PDF. Backups are unmanaged and require manual recovery.",
        "请先关闭 PDF 阅读器。LitMan 将移动/重命名文件、修改数据库路径并安装正式版 PDF。备份不受 LitMan 管理，需要手工恢复。",
    )
}

fn metadata_field_label(locale: Locale, field: &str) -> &str {
    match field {
        "title" => locale.text("title"),
        "authors" => locale.text("authors"),
        "abstract_text" => locale.text("abstract"),
        "publication_date" => locale.text("date"),
        "container_title" => locale.text("container"),
        "volume" => locale.text("volume"),
        "issue" => locale.text("issue"),
        "pages" => locale.text("pages"),
        "doi" => "DOI",
        "url" => locale.text("url"),
        "language" => locale.text("language"),
        "keywords" => locale.text("keywords"),
        "notes" => locale.text("notes"),
        _ => field,
    }
}

fn scixplorer_field_label(locale: Locale, field: ScixplorerSearchField) -> &'static str {
    match field {
        ScixplorerSearchField::Title => locale.text("title"),
        ScixplorerSearchField::Doi => "DOI",
        ScixplorerSearchField::Bibcode => "Bibcode",
    }
}

fn scixplorer_token_configured(library: Option<&Library>) -> bool {
    library.is_some_and(|library| library.config().scixplorer_api_token.is_some())
}

fn argument_config() -> Option<PathBuf> {
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--config" {
            return arguments.next().map(PathBuf::from);
        }
    }
    env::var_os("LITMAN_CONFIG").map(PathBuf::from)
}

fn manual_candidates(locale: &str) -> Vec<PathBuf> {
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
    candidates
}

fn write_embedded_manual(locale: &str) -> Result<PathBuf, String> {
    let markdown = if locale == "zh-CN" {
        USER_MANUAL_ZH_CN
    } else {
        USER_MANUAL_EN
    };
    let directory = env::temp_dir()
        .join("LitMan")
        .join(format!("manual-{}", env!("CARGO_PKG_VERSION")));
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("user-{locale}.html"));
    let html = embedded_manual_html(locale, markdown);
    let needs_write = fs::read_to_string(&path)
        .map(|existing| existing != html)
        .unwrap_or(true);
    if needs_write {
        fs::write(&path, html).map_err(|error| error.to_string())?;
    }
    Ok(path)
}

fn embedded_manual_html(locale: &str, markdown: &str) -> String {
    let title = if locale == "zh-CN" {
        "LitMan 用户手册"
    } else {
        "LitMan User Manual"
    };
    format!(
        "<!doctype html>\n<html lang=\"{locale}\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>{title}</title>\n<style>body{{max-width:960px;margin:2rem auto;padding:0 1.5rem;color:#202124;background:#fff;font-family:system-ui,'Noto Sans CJK SC',sans-serif}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;font:inherit;line-height:1.55}}@media(prefers-color-scheme:dark){{body{{color:#e8eaed;background:#202124}}}}</style>\n</head>\n<body>\n<h1>{title}</h1>\n<p>Jingdong Zhang · LitMan {} · GNU GPLv3</p>\n<pre>{}</pre>\n</body>\n</html>\n",
        env!("CARGO_PKG_VERSION"),
        html_escape(markdown)
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn app_with_library() -> (TempDir, LitManApp) {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("papers");
        fs::create_dir(&root).unwrap();
        let config_path = temporary.path().join("library.toml");
        let library = Library::init(&config_path, Config::new(root)).unwrap();
        let mut app = LitManApp::new(None);
        app.locale = Locale::new(Language::En);
        app.config_path = Some(config_path);
        app.library = Some(library);
        (temporary, app)
    }

    #[test]
    fn editor_tracks_explicit_blanks_and_unchanged_values() {
        assert_eq!(changed("same", Some("same")), None);
        assert_eq!(changed("", Some("embedded")), Some(None));
        assert_eq!(changed("手工题名", None), Some(Some("手工题名".into())));
    }

    #[test]
    fn author_and_keyword_order_is_preserved() {
        assert_eq!(
            changed_list("李伟; Ada Smith; 王芳", &[]),
            Some(vec!["李伟".into(), "Ada Smith".into(), "王芳".into()])
        );
        assert_eq!(
            changed_list("李伟; Ada Smith", &["李伟".into(), "Ada Smith".into()]),
            None
        );
    }

    #[test]
    fn importance_has_a_readable_non_color_representation() {
        assert_eq!(stars(1), "★");
        assert_eq!(stars(5), "★★★★★");
    }

    #[test]
    fn clipped_literature_cell_registers_one_tooltip() {
        let context = egui::Context::default();
        context.memory_mut(|memory| memory.set_everything_is_visible(true));
        let mut tooltip_ids = None;

        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.set_width(20.0);
            let text = "A title long enough to be clipped by the literature table column";
            let (_, galley, _) = egui::Label::new(text).layout_in_ui(ui);
            assert!(galley.elided, "the test label must exercise clipping");

            let response = literature_list_cell(ui, text);
            tooltip_ids = Some((
                response.id,
                egui::Tooltip::next_tooltip_id(ui.ctx(), response.id),
            ));
        });

        let (widget_id, next_tooltip_id) = tooltip_ids.expect("the cell should be rendered");
        assert_eq!(
            next_tooltip_id,
            egui::Tooltip::tooltip_id(widget_id, 1),
            "the explicit tooltip must be the only tooltip registered for this cell"
        );
    }

    #[test]
    fn group_tree_membership_respects_path_boundaries() {
        assert!(group_path_is_in_tree("Research", "Research"));
        assert!(group_path_is_in_tree("Research/Imaging", "Research"));
        assert!(!group_path_is_in_tree("Research Notes", "Research"));
        assert!(!group_path_is_in_tree("Other/Research", "Research"));
    }

    #[test]
    fn group_rename_rewrites_only_the_selected_tree() {
        assert_eq!(group_leaf_name("Research/Imaging"), "Imaging");
        assert_eq!(
            group_path_with_leaf("Research/Imaging", "Calibration"),
            "Research/Calibration"
        );
        assert_eq!(
            rewrite_group_path(
                "Research/Imaging/Results",
                "Research/Imaging",
                "Research/Calibration"
            ),
            "Research/Calibration/Results"
        );
        assert_eq!(
            rewrite_group_path(
                "Research/Imaging Notes",
                "Research/Imaging",
                "Research/Calibration"
            ),
            "Research/Imaging Notes"
        );
    }

    #[test]
    fn group_creation_reports_success_and_duplicate_warning() {
        let (_temporary, mut app) = app_with_library();
        app.new_group_path = "Research/Imaging".into();
        app.create_group_from_input();
        assert_eq!(app.message, "Created group “Research/Imaging”.");
        assert!(app.new_group_path.is_empty());

        app.new_group_path = "research/imaging".into();
        app.create_group_from_input();
        assert_eq!(app.message, "a group with that name already exists");
        assert_eq!(app.new_group_path, "research/imaging");
    }

    #[test]
    fn scixplorer_search_activation_follows_optional_token_configuration() {
        let (_temporary, mut app) = app_with_library();
        assert!(!scixplorer_token_configured(app.library.as_ref()));
        app.library
            .as_mut()
            .unwrap()
            .set_scixplorer_api_token(Some("personal-token".into()))
            .unwrap();
        assert!(scixplorer_token_configured(app.library.as_ref()));
        app.library
            .as_mut()
            .unwrap()
            .set_scixplorer_api_token(None)
            .unwrap();
        assert!(!scixplorer_token_configured(app.library.as_ref()));
    }

    #[test]
    fn pdf_update_enablement_and_acknowledgment_are_strict() {
        assert!(pdf_update_enabled(1, true, true, false, false));
        assert!(!pdf_update_enabled(2, true, true, false, false));
        assert!(!pdf_update_enabled(1, false, true, false, false));
        assert!(!pdf_update_enabled(1, true, false, false, false));
        assert!(!pdf_update_enabled(1, true, true, true, false));
        assert!(!pdf_update_enabled(1, true, true, false, true));
        assert!(!pdf_replacement_confirmation_enabled(false, false));
        assert!(pdf_replacement_confirmation_enabled(true, false));
        assert!(!pdf_replacement_confirmation_enabled(true, true));
    }

    #[test]
    fn pdf_replacement_warning_is_localized_and_explicit() {
        let english = pdf_replacement_warning(Locale::new(Language::En));
        assert!(english.contains("move/rename files"));
        assert!(english.contains("manual recovery"));
        let chinese = pdf_replacement_warning(Locale::new(Language::ZhCn));
        assert!(chinese.contains("移动/重命名"));
        assert!(chinese.contains("手工恢复"));
    }

    #[test]
    fn group_rename_preserves_selected_descendant_paths() {
        let (_temporary, mut app) = app_with_library();
        let library = app.library.as_mut().unwrap();
        library.create_group("Research/Imaging/Results").unwrap();
        library.create_group("Research/Astrometry").unwrap();
        app.reload();
        app.group_filter = Some("Research/Imaging".into());
        app.assignment_group = Some("Research/Imaging/Results".into());

        assert_eq!(
            app.rename_selected_group("Research/Imaging", "ASTROMETRY")
                .unwrap_err(),
            "a group with that name already exists"
        );
        app.rename_selected_group("Research/Imaging", "Calibration")
            .unwrap();

        assert_eq!(app.group_filter.as_deref(), Some("Research/Calibration"));
        assert_eq!(
            app.assignment_group.as_deref(),
            Some("Research/Calibration/Results")
        );
        let library = app.library.as_ref().unwrap();
        assert!(
            library
                .group_exists("Research/Calibration/Results")
                .unwrap()
        );
        assert!(!library.group_exists("Research/Imaging").unwrap());
        assert_eq!(
            app.message,
            "Renamed group “Research/Imaging” to “Research/Calibration”."
        );
    }

    #[test]
    fn crate_inherits_the_software_author() {
        assert_eq!(env!("CARGO_PKG_AUTHORS"), "Jingdong Zhang");
    }

    #[test]
    fn embedded_manual_is_searchable_safe_html() {
        let html = embedded_manual_html("en", "# Manual\nFind <paper> & notes");
        assert!(html.contains("Jingdong Zhang"));
        assert!(html.contains("Find &lt;paper&gt; &amp; notes"));
        assert!(html.contains("<meta charset=\"utf-8\">"));
    }

    #[test]
    fn embedded_manual_fallback_writes_a_standalone_page() {
        let path = write_embedded_manual("zh-CN").expect("embedded manual should be writable");
        let html = fs::read_to_string(path).expect("embedded manual should be readable");
        assert!(html.contains("LitMan 用户手册"));
        assert!(html.contains("备份"));
    }
}
