use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{LitmanError, Result};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    System,
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl Language {
    pub fn resolved(self) -> Self {
        match self {
            Self::System => {
                let locale = sys_locale::get_locale()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if locale.starts_with("zh") {
                    Self::ZhCn
                } else {
                    Self::En
                }
            }
            language => language,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    pub database: PathBuf,
    pub library_root: PathBuf,
    #[serde(default)]
    pub language: Language,
}

impl Config {
    pub fn new(library_root: PathBuf) -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            database: PathBuf::from("literature.sqlite3"),
            library_root,
            language: Language::System,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let backup = backup_path(path);
        if !path.is_file() && backup.is_file() {
            fs::rename(&backup, path)?;
        }
        if !path.is_file() {
            return Err(LitmanError::ConfigNotFound(path.to_path_buf()));
        }
        let config: Self = toml::from_str(&fs::read_to_string(path)?)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tmp");
        let backup = backup_path(path);
        fs::write(&temporary, toml::to_string_pretty(self)?)?;
        if path.exists() {
            if backup.exists() {
                fs::remove_file(&backup)?;
            }
            fs::rename(path, &backup)?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if backup.is_file() {
                let _ = fs::rename(&backup, path);
            }
            return Err(error.into());
        }
        if backup.is_file() {
            fs::remove_file(backup)?;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(LitmanError::InvalidConfig(format!(
                "unsupported schema version {}; expected {}",
                self.schema_version, CONFIG_SCHEMA_VERSION
            )));
        }
        if self.database.is_absolute()
            || self.database.components().count() != 1
            || !matches!(
                self.database.components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(LitmanError::InvalidConfig(
                "database must be a filename beside the configuration".into(),
            ));
        }
        if self.database.extension().and_then(|value| value.to_str()) != Some("sqlite3") {
            return Err(LitmanError::InvalidConfig(
                "database filename must end in .sqlite3".into(),
            ));
        }
        if self.library_root.as_os_str().is_empty() {
            return Err(LitmanError::InvalidConfig(
                "library_root cannot be empty".into(),
            ));
        }
        Ok(())
    }

    pub fn database_path(&self, config_path: &Path) -> PathBuf {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&self.database)
    }

    pub fn root_path(&self, config_path: &Path) -> PathBuf {
        if self.library_root.is_absolute() {
            self.library_root.clone()
        } else {
            config_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&self.library_root)
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| format!("{}.bak", name.to_string_lossy()))
        .unwrap_or_else(|| "library.toml.bak".into());
    path.with_file_name(name)
}

pub fn default_config_path() -> PathBuf {
    PathBuf::from("library.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn database_must_be_adjacent() {
        let mut config = Config::new(PathBuf::from("papers"));
        config.database = PathBuf::from("../outside.sqlite3");
        assert!(config.validate().is_err());
        config.database = PathBuf::from(".");
        assert!(config.validate().is_err());
    }

    #[test]
    fn relative_paths_are_based_on_config() {
        let config = Config::new(PathBuf::from("../papers"));
        let path = Path::new("portable/library.toml");
        assert_eq!(
            config.database_path(path),
            PathBuf::from("portable/literature.sqlite3")
        );
        assert_eq!(config.root_path(path), PathBuf::from("portable/../papers"));
    }

    #[test]
    fn interrupted_config_replacement_recovers_the_backup() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("library.toml");
        let config = Config::new(PathBuf::from("papers"));
        fs::write(backup_path(&path), toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        assert!(path.is_file());
        assert!(!backup_path(&path).exists());
    }
}
