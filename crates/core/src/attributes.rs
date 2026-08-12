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

/// Текущая версия схемы pet.json (ТД-15). История:
/// - v1 — неявная (файлы без поля `schema_version`): имя + характеристики;
/// - v2 — добавлены `schema_version` и `summoned` (убранный питомец
///   переживает рестарт демона, ТД-17).
pub const SCHEMA_VERSION: u32 = 2;

/// Персистентная запись питомца — прообраз журнала (M2).
/// Хранится в `$XDG_DATA_HOME/driftling/pet.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetRecord {
    /// Версия схемы файла. Файл без поля (v1) читается как текущая схема —
    /// serde-default и есть миграция v1 -> v2; файл НОВЕЕ поддерживаемой
    /// версии load() отвергает явной ошибкой, а не тихим дефолтом.
    pub schema_version: u32,
    pub name: String,
    pub attributes: PetAttributes,
    /// Питомец призван на экран; false = убран через dismiss (ТД-17).
    pub summoned: bool,
}

impl Default for PetRecord {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            // Нейтральный запасной вариант: локализованное имя питомцу даёт
            // демон при первом создании записи (ТД-30) — в файле имя данные.
            name: "Driftling".to_string(),
            attributes: PetAttributes::default(),
            summoned: true,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod fs {
    use super::{PetRecord, SCHEMA_VERSION};
    use std::path::{Path, PathBuf};

    /// Каталог данных: `$XDG_DATA_HOME/driftling` (или `~/.local/share/...`).
    pub fn data_dir() -> PathBuf {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("driftling")
    }

    /// pet.json внутри произвольного каталога данных (DI для тестов, ТД-26).
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join("pet.json")
    }

    /// Штатный путь: `$XDG_DATA_HOME/driftling/pet.json`.
    pub fn path() -> PathBuf {
        path_in(&data_dir())
    }

    impl PetRecord {
        /// Прочитать запись из штатного каталога данных.
        pub fn load() -> Result<Option<PetRecord>, String> {
            Self::load_in(&data_dir())
        }

        /// Прочитать запись из `dir`. Семантика (ТД-15,16):
        /// - нет файла — `Ok(None)`;
        /// - схема новее поддерживаемой — явная ошибка (файл от более
        ///   новой версии Driftling трогать нельзя);
        /// - битый JSON — файл переименовывается в
        ///   `pet.json.corrupt-<unixts>` (улики сохраняются) и `Ok(None)`:
        ///   демон стартует с чистого листа, не перезаписывая оригинал.
        pub fn load_in(dir: &Path) -> Result<Option<PetRecord>, String> {
            let p = path_in(dir);
            let text = match std::fs::read_to_string(&p) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.to_string()),
            };
            match serde_json::from_str::<PetRecord>(&text) {
                // Ошибки ядра — технические, по-английски (ТД-30): ядро не
                // тянет i18n (wasm-чистота), наружу их заворачивают UI-слои.
                Ok(rec) if rec.schema_version > SCHEMA_VERSION => Err(format!(
                    "pet.json schema v{} is newer than supported v{SCHEMA_VERSION} — \
                     file from a newer Driftling version, leaving it untouched",
                    rec.schema_version
                )),
                Ok(rec) => Ok(Some(rec)),
                Err(parse_err) => {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let backup = p.with_file_name(format!("pet.json.corrupt-{ts}"));
                    match std::fs::rename(&p, &backup) {
                        Ok(()) => Ok(None),
                        // Бэкап не удался — оригинал не трогаем и не даём
                        // его молча перезаписать дефолтом.
                        Err(io_err) => Err(format!(
                            "pet.json is corrupt ({parse_err}) and backup failed: {io_err}"
                        )),
                    }
                }
            }
        }

        /// Атомарная запись в штатный каталог данных.
        pub fn save(&self) -> Result<(), String> {
            self.save_in(&data_dir())
        }

        /// Атомарная запись (tmp + rename) в `dir` — краш не теряет
        /// питомца (ТЗ §7). DI для тестов (ТД-26).
        pub fn save_in(&self, dir: &Path) -> Result<(), String> {
            let p = path_in(dir);
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
            let tmp = p.with_extension("json.tmp");
            std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs::{data_dir, path as record_path, path_in as record_path_in};

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
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert!(back.summoned);
    }

    /// v1-файл (без schema_version/summoned) читается через serde-default —
    /// это и есть миграция v1 -> v2.
    #[test]
    fn v1_record_migrates_via_defaults() {
        let rec: PetRecord =
            serde_json::from_str(r#"{"name":"Тестик","attributes":{"size":128}}"#).unwrap();
        assert_eq!(rec.name, "Тестик");
        assert_eq!(rec.attributes.size, 128);
        assert_eq!(rec.schema_version, SCHEMA_VERSION);
        assert!(rec.summoned, "v1 не знал dismiss — питомец призван");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod fs_tests {
    use super::*;
    use std::path::PathBuf;

    /// Уникальный временный каталог на тест — без мутации env (ТД-26).
    fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("driftling-attrs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_and_load_roundtrip_in_dir() {
        let dir = tmp_dir("roundtrip");
        assert_eq!(PetRecord::load_in(&dir).unwrap(), None, "файла ещё нет");

        let rec = PetRecord {
            name: "Пробник".into(),
            summoned: false,
            ..PetRecord::default()
        };
        rec.save_in(&dir).unwrap();

        let back = PetRecord::load_in(&dir).unwrap().expect("файл сохранён");
        assert_eq!(back, rec);
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert!(!back.summoned, "dismissed переживает перезапись");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_is_explicit_error() {
        let dir = tmp_dir("newer");
        std::fs::write(
            record_path_in(&dir),
            format!(
                r#"{{"schema_version":{},"name":"Из будущего"}}"#,
                SCHEMA_VERSION + 1
            ),
        )
        .unwrap();
        let err = PetRecord::load_in(&dir).unwrap_err();
        assert!(
            err.contains("newer"),
            "ошибка должна объяснять причину: {err}"
        );
        assert!(
            record_path_in(&dir).exists(),
            "файл из будущего остаётся нетронутым"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_file_is_backed_up_and_load_returns_none() {
        let dir = tmp_dir("corrupt");
        let garbage = "{ это не json вовсе";
        std::fs::write(record_path_in(&dir), garbage).unwrap();

        assert_eq!(PetRecord::load_in(&dir).unwrap(), None);
        assert!(!record_path_in(&dir).exists(), "битый файл убран с дороги");

        // Улики на месте: ровно один бэкап с исходным содержимым.
        let backups: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("pet.json.corrupt-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), garbage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v1_file_on_disk_loads_and_upgrades_on_save() {
        let dir = tmp_dir("v1");
        std::fs::write(
            record_path_in(&dir),
            r#"{"name":"Старичок","attributes":{"walk_speed":100.0}}"#,
        )
        .unwrap();

        let rec = PetRecord::load_in(&dir).unwrap().expect("v1 читается");
        assert_eq!(rec.name, "Старичок");
        rec.save_in(&dir).unwrap();

        let text = std::fs::read_to_string(record_path_in(&dir)).unwrap();
        assert!(
            text.contains(&format!("\"schema_version\": {SCHEMA_VERSION}")),
            "после сохранения файл становится v2: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
