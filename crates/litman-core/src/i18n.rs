use crate::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale(pub Language);

impl Locale {
    pub fn new(language: Language) -> Self {
        Self(language.resolved())
    }

    pub fn text(self, key: &str) -> &'static str {
        tr(self.0, key)
    }
}

pub fn tr(language: Language, key: &str) -> &'static str {
    let chinese = language.resolved() == Language::ZhCn;
    match (chinese, key) {
        (true, "app.title") => "LitMan 文献管理器",
        (true, "action.scan") => "扫描",
        (true, "action.save") => "保存",
        (true, "action.open") => "打开 PDF",
        (true, "action.backup") => "备份",
        (true, "action.manual") => "用户手册",
        (true, "search") => "搜索",
        (true, "groups") => "分组",
        (true, "importance") => "重要程度",
        (true, "papers") => "文献",
        (true, "metadata") => "元数据",
        (true, "title") => "标题",
        (true, "author") => "第一作者",
        (true, "authors") => "作者",
        (true, "year") => "年份",
        (true, "abstract") => "摘要",
        (true, "date") => "发表日期",
        (true, "container") => "期刊或会议",
        (true, "volume") => "卷",
        (true, "issue") => "期",
        (true, "pages") => "页码",
        (true, "doi") => "DOI",
        (true, "url") => "网址",
        (true, "language") => "语言",
        (true, "keywords") => "关键词",
        (true, "notes") => "笔记",
        (true, "file") => "文件",
        (true, "status") => "状态",
        (true, "unrated") => "未评级",
        (true, "no_library") => "请选择或创建文献库配置文件。",
        (true, "scan.complete") => "扫描完成",
        (true, "error") => "错误",
        (true, "present") => "可用",
        (true, "missing") => "缺失",
        (true, "cli.no_results") => "没有匹配的文献。",
        (_, "app.title") => "LitMan Literature Manager",
        (_, "action.scan") => "Scan",
        (_, "action.save") => "Save",
        (_, "action.open") => "Open PDF",
        (_, "action.backup") => "Backup",
        (_, "action.manual") => "User manual",
        (_, "search") => "Search",
        (_, "groups") => "Groups",
        (_, "importance") => "Importance",
        (_, "papers") => "Papers",
        (_, "metadata") => "Metadata",
        (_, "title") => "Title",
        (_, "author") => "First author",
        (_, "authors") => "Authors",
        (_, "year") => "Year",
        (_, "abstract") => "Abstract",
        (_, "date") => "Publication date",
        (_, "container") => "Journal or conference",
        (_, "volume") => "Volume",
        (_, "issue") => "Issue",
        (_, "pages") => "Pages",
        (_, "doi") => "DOI",
        (_, "url") => "URL",
        (_, "language") => "Language",
        (_, "keywords") => "Keywords",
        (_, "notes") => "Notes",
        (_, "file") => "File",
        (_, "status") => "Status",
        (_, "unrated") => "Unrated",
        (_, "no_library") => "Choose or create a library configuration file.",
        (_, "scan.complete") => "Scan complete",
        (_, "error") => "Error",
        (_, "present") => "Present",
        (_, "missing") => "Missing",
        (_, "cli.no_results") => "No matching papers.",
        _ => "",
    }
}
