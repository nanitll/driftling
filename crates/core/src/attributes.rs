//! Характеристики питомца (ТЗ §3.3): принадлежат конкретному питомцу и
//! персистентны, пользователь напрямую их не меняет — только смотрит.
//! Правка — исключительно через дебаг-панель (IPC `SetAttributes`);
//! игровая «прокачка» появится в M2+.
//!
//! В M2 характеристики переезжают в журнал событий (изменение = событие)
//! и начинают синкаться между устройствами.

use serde::{Deserialize, Serialize};

use crate::behavior::BehaviorConfig;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetAttributes {
    /// Размер спрайта, px (квадрат). В M2 будет зависеть от стадии роста.
    pub size: u32,
    /// Скорость ходьбы, px/s.
    pub walk_speed: f32,
    /// Непоседливость: вес перехода Idle -> Walk (из 100).
    pub curiosity: u32,
    /// Сонливость: вес перехода Idle -> Sleep (из 100).
    pub sleepiness: u32,
    /// Диапазон длительности сна, сек.
    pub sleep_min: f32,
    pub sleep_max: f32,
}

impl Default for PetAttributes {
    fn default() -> Self {
        let d = BehaviorConfig::default();
        Self {
            size: 96,
            walk_speed: d.walk_speed,
            curiosity: d.w_idle_to_walk,
            sleepiness: d.w_idle_to_sleep,
            sleep_min: d.sleep_range.0,
            sleep_max: d.sleep_range.1,
        }
    }
}

impl PetAttributes {
    /// Версия с безопасными диапазонами — кривые значения (битый файл,
    /// дебаг-панель) не должны ломать симуляцию.
    pub fn clamped(&self) -> Self {
        let curiosity = self.curiosity.min(100);
        let sleep_min = self.sleep_min.clamp(1.0, 3600.0);
        Self {
            size: self.size.clamp(32, 256),
            walk_speed: self.walk_speed.clamp(5.0, 400.0),
            curiosity,
            sleepiness: self.sleepiness.min(100 - curiosity),
            sleep_min,
            sleep_max: self.sleep_max.clamp(sleep_min, 7200.0),
        }
    }

    /// Полный BehaviorConfig: характеристики поверх дефолтов движка.
    pub fn behavior_config(&self) -> BehaviorConfig {
        let a = self.clamped();
        BehaviorConfig {
            walk_speed: a.walk_speed,
            w_idle_to_walk: a.curiosity,
            w_idle_to_sleep: a.sleepiness,
            sleep_range: (a.sleep_min, a.sleep_max),
            ..BehaviorConfig::default()
        }
    }
}

/// Персистентная запись питомца — прообраз журнала (M2).
/// Хранится в `$XDG_DATA_HOME/driftling/pet.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetRecord {
    pub name: String,
    pub attributes: PetAttributes,
}

impl Default for PetRecord {
    fn default() -> Self {
        Self {
            name: "Дрифтлинг".to_string(),
            attributes: PetAttributes::default(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod fs {
    use super::PetRecord;
    use std::path::PathBuf;

    /// `$XDG_DATA_HOME/driftling/pet.json` (или `~/.local/share/...`).
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("driftling").join("pet.json")
    }

    impl PetRecord {
        /// Прочитать запись; нет файла — None, битый файл — ошибка.
        pub fn load() -> Result<Option<PetRecord>, String> {
            match std::fs::read_to_string(path()) {
                Ok(text) => serde_json::from_str(&text)
                    .map(Some)
                    .map_err(|e| e.to_string()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        }

        /// Атомарная запись (tmp + rename) — краш не теряет питомца (ТЗ §7).
        pub fn save(&self) -> Result<(), String> {
            let p = path();
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
            let tmp = p.with_extension("json.tmp");
            std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs::path as record_path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_engine_defaults() {
        let b = PetAttributes::default().behavior_config();
        let d = BehaviorConfig::default();
        assert_eq!(b.walk_speed, d.walk_speed);
        assert_eq!(b.w_idle_to_walk, d.w_idle_to_walk);
        assert_eq!(b.sleep_range, d.sleep_range);
    }

    #[test]
    fn hostile_values_are_clamped() {
        let a = PetAttributes {
            size: 9000,
            walk_speed: -5.0,
            curiosity: 100,
            sleepiness: 100,
            sleep_min: 100.0,
            sleep_max: 1.0,
        }
        .clamped();
        assert_eq!(a.size, 256);
        assert_eq!(a.walk_speed, 5.0);
        assert!(a.curiosity + a.sleepiness <= 100);
        assert!(a.sleep_min <= a.sleep_max);
    }

    #[test]
    fn record_json_roundtrip() {
        let rec = PetRecord::default();
        let text = serde_json::to_string(&rec).unwrap();
        let back: PetRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(rec, back);
    }
}
