//! Пользовательский конфиг (`~/.config/driftling/config.toml`).
//!
//! Структура и (де)сериализация — платформонезависимы; файловые операции
//! отключены под wasm32. Настройки-UI и демон работают с одним и тем же
//! файлом: UI сохраняет, демон перечитывает по IPC `Reload`.

use serde::{Deserialize, Serialize};

use crate::behavior::BehaviorConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub pet: PetConfig,
    pub behavior: BehaviorTuning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetConfig {
    /// Размер спрайта, px (квадрат).
    pub size: u32,
}

impl Default for PetConfig {
    fn default() -> Self {
        Self { size: 96 }
    }
}

/// Подмножество BehaviorConfig, вынесенное пользователю. Остальные поля
/// берутся из BehaviorConfig::default().
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorTuning {
    /// Скорость ходьбы, px/s.
    pub walk_speed: f32,
    /// Вес перехода Idle -> Walk (из 100): насколько питомец непоседлив.
    pub w_idle_to_walk: u32,
    /// Вес перехода Idle -> Sleep (из 100): насколько питомец соня.
    pub w_idle_to_sleep: u32,
    /// Диапазон длительности сна, сек.
    pub sleep_min: f32,
    pub sleep_max: f32,
}

impl Default for BehaviorTuning {
    fn default() -> Self {
        let d = BehaviorConfig::default();
        Self {
            walk_speed: d.walk_speed,
            w_idle_to_walk: d.w_idle_to_walk,
            w_idle_to_sleep: d.w_idle_to_sleep,
            sleep_min: d.sleep_range.0,
            sleep_max: d.sleep_range.1,
        }
    }
}

impl Config {
    /// Собрать полный BehaviorConfig: пользовательские поля поверх дефолтов.
    /// Значения приводятся к безопасным диапазонам — кривой конфиг не должен
    /// ломать симуляцию.
    pub fn behavior_config(&self) -> BehaviorConfig {
        let t = &self.behavior;
        let w_walk = t.w_idle_to_walk.min(100);
        let sleep_min = t.sleep_min.clamp(1.0, 3600.0);
        BehaviorConfig {
            walk_speed: t.walk_speed.clamp(5.0, 400.0),
            w_idle_to_walk: w_walk,
            w_idle_to_sleep: t.w_idle_to_sleep.min(100 - w_walk),
            sleep_range: (sleep_min, t.sleep_max.clamp(sleep_min, 7200.0)),
            ..BehaviorConfig::default()
        }
    }

    /// Размер спрайта с ограничением разумного диапазона.
    pub fn sprite_size(&self) -> u32 {
        self.pet.size.clamp(32, 256)
    }
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

    impl Config {
        /// Прочитать конфиг; нет файла или файл битый — дефолт (ошибку парсинга
        /// возвращаем, чтобы UI мог её показать).
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
pub use fs::path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_behavior_default() {
        let cfg = Config::default();
        let b = cfg.behavior_config();
        let d = BehaviorConfig::default();
        assert_eq!(b.walk_speed, d.walk_speed);
        assert_eq!(b.w_idle_to_walk, d.w_idle_to_walk);
        assert_eq!(b.sleep_range, d.sleep_range);
    }

    #[test]
    fn toml_roundtrip_and_partial_files() {
        let cfg = Config {
            pet: PetConfig { size: 128 },
            ..Config::default()
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);

        // Частичный файл: незнакомые поля игнорируются, пропущенные — дефолт.
        let partial: Config = toml::from_str("[behavior]\nwalk_speed = 99.0\n").unwrap();
        assert_eq!(partial.behavior.walk_speed, 99.0);
        assert_eq!(partial.pet.size, 96);
    }

    #[test]
    fn hostile_values_are_clamped() {
        let cfg: Config = toml::from_str(
            "[pet]\nsize = 9000\n[behavior]\nwalk_speed = -5.0\nw_idle_to_walk = 100\nw_idle_to_sleep = 100\nsleep_min = 100.0\nsleep_max = 1.0\n",
        )
        .unwrap();
        assert_eq!(cfg.sprite_size(), 256);
        let b = cfg.behavior_config();
        assert_eq!(b.walk_speed, 5.0);
        assert_eq!(b.w_idle_to_walk + b.w_idle_to_sleep, 100);
        assert!(b.sleep_range.0 <= b.sleep_range.1);
    }
}
