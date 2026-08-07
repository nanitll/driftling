//! Настройки ПРИЛОЖЕНИЯ (`~/.config/driftling/config.toml`).
//!
//! Здесь живёт только то, что относится к приложению, а не к питомцу:
//! характеристики питомца — в [`crate::attributes`] (ТЗ §3.3). Сейчас
//! секций мало; файл переживает добавление/удаление полей (serde default,
//! незнакомые ключи игнорируются).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    // Зарезервировано под настройки приложения (мониторы, синк — M3,
    // выбор пака — M4). Автозапуск управляется .desktop-файлом напрямую.
}

/// Достать из legacy-конфига (до переезда характеристик в pet.json)
/// значения старых секций `[pet]`/`[behavior]` — чтобы питомец, настроенный
/// в ранних версиях, пережил обновление. Вернёт None, если legacy-ключей нет.
pub fn legacy_attributes(toml_text: &str) -> Option<crate::attributes::PetAttributes> {
    let value: toml::Value = toml_text.parse().ok()?;
    let mut attrs = crate::attributes::PetAttributes::default();
    let mut found = false;

    if let Some(size) = value
        .get("pet")
        .and_then(|p| p.get("size"))
        .and_then(|v| v.as_integer())
    {
        attrs.size = size.clamp(0, u32::MAX as i64) as u32;
        found = true;
    }
    if let Some(b) = value.get("behavior") {
        let num = |key: &str| -> Option<f64> {
            b.get(key)
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        };
        if let Some(v) = num("walk_speed") {
            attrs.walk_speed = v as f32;
            found = true;
        }
        if let Some(v) = num("w_idle_to_walk") {
            attrs.curiosity = v as u32;
            found = true;
        }
        if let Some(v) = num("w_idle_to_sleep") {
            attrs.sleepiness = v as u32;
            found = true;
        }
        if let Some(v) = num("sleep_min") {
            attrs.sleep_min = v as f32;
            found = true;
        }
        if let Some(v) = num("sleep_max") {
            attrs.sleep_max = v as f32;
            found = true;
        }
    }
    found.then(|| attrs.clamped())
}

#[cfg(not(target_arch = "wasm32"))]
mod fs {
    use super::Config;
    use std::path::PathBuf;

    /// `$XDG_CONFIG_HOME/driftling/config.toml` (или `~/.config/...`).
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("driftling").join("config.toml")
    }

    /// Сырой текст конфига — для миграции legacy-ключей.
    pub fn raw_text() -> Option<String> {
        std::fs::read_to_string(path()).ok()
    }

    impl Config {
        /// Прочитать конфиг; нет файла — дефолт, битый файл — ошибка текстом.
        pub fn load() -> Result<Config, String> {
            match std::fs::read_to_string(path()) {
                Ok(text) => toml::from_str(&text).map_err(|e| e.to_string()),
                Err(_) => Ok(Config::default()),
            }
        }

        /// Атомарная запись: во временный файл + rename.
        pub fn save(&self) -> Result<(), String> {
            let p = path();
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
            let tmp = p.with_extension("toml.tmp");
            std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs::{path, raw_text};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_legacy_keys_do_not_break_load() {
        let cfg: Config = toml::from_str("[pet]\nsize = 90\n[behavior]\nwalk_speed = 400.0\n")
            .unwrap_or_default();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn legacy_attributes_migrate_and_clamp() {
        let attrs = legacy_attributes(
            "[pet]\nsize = 90\n[behavior]\nwalk_speed = 400.0\nw_idle_to_walk = 100\nw_idle_to_sleep = 0\n",
        )
        .expect("legacy-ключи должны найтись");
        assert_eq!(attrs.size, 90);
        assert_eq!(attrs.walk_speed, 400.0);
        assert_eq!(attrs.curiosity, 100);
        assert_eq!(attrs.sleepiness, 0);

        assert!(legacy_attributes("").is_none());
        assert!(legacy_attributes("[app]\nx = 1\n").is_none());
    }
}
