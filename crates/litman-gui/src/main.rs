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
    Config, FileStatus, Group, Language, Library, ListFilter, Locale, Paper, PaperUpdate,
    ScanEvent, ScanOptions, default_config_path,
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
    let mut wgpu_options = eframe::WgpuConfiguration::default();
    #[cfg(windows)]
    if matches!(renderer, eframe::Renderer::Wgpu)
        && let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup
    {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    }
    let options = eframe::NativeOptions {
        renderer,
        wgpu_options,
        viewport: egui::ViewportBuilder::default()
            .with_title("LitMan")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([850.0, 540.0]),
        ..Default::default()
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
                self.assignment_group = None;
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
        if self.scan_receiver.is_some() {
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
                if ui
                    .button(dual(self.locale, "Open library", "打开文献库"))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("LitMan TOML", &["toml"])
                        .pick_file()
                {
                    self.open_library(path);
                }
                if ui
                    .button(dual(self.locale, "New library", "新建文献库"))
                    .clicked()
                {
                    self.create_library();
                }
                if self.library.is_some() {
                    if ui.button(self.locale.text("action.scan")).clicked() {
                        self.begin_scan(false);
                    }
                    if ui
                        .button(dual(self.locale, "Refresh metadata", "刷新元数据"))
                        .clicked()
                    {
                        self.begin_scan(true);
                    }
                    if ui
                        .button(dual(self.locale, "Relocate root", "更改根目录"))
                        .clicked()
                    {
                        self.relocate_root();
                    }
                    if ui.button(self.locale.text("action.backup")).clicked() {
                        self.backup();
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
                    && let Some(library) = self.library.as_mut()
                {
                    let requested_path = self.new_group_path.trim().to_owned();
                    match library.create_group(&requested_path) {
                        Ok(group) => {
                            self.assignment_group =
                                library.group_path(group.id).ok().or(Some(requested_path));
                            self.new_group_path.clear();
                            self.reload();
                        }
                        Err(error) => self.message = error.localized(self.locale.0),
                    }
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
                            text_clicked |= ui
                                .add(
                                    egui::Label::new(&title).sense(egui::Sense::click()),
                                )
                                .on_hover_text(title)
                                .clicked();
                        });
                        row.col(|ui| {
                            let authors = paper.authors.join("; ");
                            text_clicked |= ui
                                .add(
                                    egui::Label::new(&authors).sense(egui::Sense::click()),
                                )
                                .on_hover_text(authors)
                                .clicked();
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
                                    let source = if paper.manual_overrides.contains(field_name) {
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
                            .button(dual(
                                self.locale,
                                "Remove database record",
                                "移除数据库记录",
                            ))
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
                    "Local-first literature management without modifying PDFs.",
                    "本地优先的文献管理，不修改 PDF。",
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
        self.about_window(root);
        if self.scan_receiver.is_some() {
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

fn dual(locale: Locale, english: &'static str, chinese: &'static str) -> &'static str {
    if locale.0 == Language::ZhCn {
        chinese
    } else {
        english
    }
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
