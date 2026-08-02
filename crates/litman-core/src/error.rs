use std::path::PathBuf;

use crate::config::Language;

#[derive(Debug, thiserror::Error)]
pub enum LitmanError {
    #[error("configuration file was not found: {0}")]
    ConfigNotFound(PathBuf),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("library root is unavailable: {0}")]
    RootUnavailable(PathBuf),
    #[error("paper was not found: {0}")]
    PaperNotFound(String),
    #[error("paper id prefix is ambiguous: {0}")]
    AmbiguousPaperId(String),
    #[error("group was not found: {0}")]
    GroupNotFound(String),
    #[error("a group with that name already exists")]
    DuplicateGroup,
    #[error("importance must be between 1 and 5")]
    InvalidImportance,
    #[error("the requested field cannot be reset: {0}")]
    InvalidField(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    TomlDecode(#[from] toml::de::Error),
    #[error(transparent)]
    TomlEncode(#[from] toml::ser::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl LitmanError {
    /// Format human-facing errors in the selected UI language while preserving
    /// stable English enum and JSON values elsewhere.
    pub fn localized(&self, language: Language) -> String {
        if language.resolved() != Language::ZhCn {
            return self.to_string();
        }
        match self {
            Self::ConfigNotFound(path) => format!("未找到配置文件：{}", path.display()),
            Self::InvalidConfig(detail) => format!("配置无效：{detail}"),
            Self::RootUnavailable(path) => format!("文献根目录不可用：{}", path.display()),
            Self::PaperNotFound(id) => format!("未找到文献：{id}"),
            Self::AmbiguousPaperId(id) => format!("文献 ID 前缀有歧义：{id}"),
            Self::GroupNotFound(path) => format!("未找到分组：{path}"),
            Self::DuplicateGroup => "同名分组已经存在".into(),
            Self::InvalidImportance => "重要程度必须在 1 到 5 之间".into(),
            Self::InvalidField(field) => format!("无法重置指定字段：{field}"),
            Self::Io(error) => format!("输入/输出错误：{error}"),
            Self::Database(error) => format!("数据库错误：{error}"),
            Self::TomlDecode(error) => format!("TOML 读取错误：{error}"),
            Self::TomlEncode(error) => format!("TOML 写入错误：{error}"),
            Self::Json(error) => format!("JSON 错误：{error}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, LitmanError>;
