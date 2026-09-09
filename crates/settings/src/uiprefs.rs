//! Настройки САМОГО окна: язык, стартовая страница, «Продвинутые».
//!
//! Отдельный файл `settings-ui.toml` рядом с `config.toml`, а не секция в
//! нём: окно должно читать свои настройки до всякого IPC и оставаться
//! настраиваемым при лежащем демоне. Заодно демон не обязан знать про
//! существование окна.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Язык интерфейса окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Как в системе (переменные окружения).
    #[default]
    System,
    Ru,
    En,
}

impl Language {
    pub fn code(self) -> Option<&'static str> {
        match self {
            Language::System => None,
            Language::Ru => Some("ru"),
            Language::En => Some("en"),
        }
    }

    pub const ALL: [Language; 3] = [Language::System, Language::Ru, Language::En];
}

/// Настройки окна.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub language: Language,
    /// Показывать страницу «Продвинутые» без флага `--debug`.
    pub advanced: bool,
    /// На какой странице открываться: pet | world | devices | app.
    pub start_page: String,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            language: Language::System,
            advanced: false,
            start_page: "pet".into(),
        }
    }
}

impl UiPrefs {
    /// Путь к файлу настроек окна.
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("driftling").join("settings-ui.toml")
    }

    /// Прочитать; нет файла или он битый — дефолт (окно всегда открывается).
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Атомарная запись: временный файл + rename.
    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_old_behaviour() {
        let p = UiPrefs::default();
        assert_eq!(p.language, Language::System);
        assert!(!p.advanced);
        assert_eq!(p.start_page, "pet");
    }

    #[test]
    fn unknown_and_missing_keys_survive() {
        let p: UiPrefs = toml::from_str("advanced = true\nnonsense = 5\n").unwrap();
        assert!(p.advanced);
        assert_eq!(p.language, Language::System, "чего нет — то по умолчанию");
        let round: UiPrefs = toml::from_str(&toml::to_string_pretty(&p).unwrap()).unwrap();
        assert_eq!(round, p, "запись и чтение сходятся");
    }

    #[test]
    fn language_codes_map_to_bundles() {
        assert_eq!(Language::System.code(), None);
        assert_eq!(Language::Ru.code(), Some("ru"));
        assert_eq!(Language::En.code(), Some("en"));
        assert_eq!(Language::ALL.len(), 3);
    }
}
