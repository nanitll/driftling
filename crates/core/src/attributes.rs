//! Характеристики питомца (ТЗ §3.3) и запись pet.json (schema v3).
//!
//! С фазы B источник истины — журнал событий (`journal.rs`): имя,
//! характеристики и summoned живут в событиях (Genesis/Renamed/
//! AttributesSet/...), состояние всегда пересворачивается из журнала при
//! старте. В pet.json остаётся только то, что не выводится из журнала:
//! идентификатор устройства для HLC-меток.
//!
//! Пользователь напрямую характеристики не меняет — только смотрит;
//! правка — исключительно через дебаг-панель (IPC `SetAttributes` =
//! событие `AttributesSet`), игровая «прокачка» — B5.

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
///   переживает рестарт демона, ТД-17);
/// - v3 — источник истины переехал в журнал (journal.jsonl); в pet.json
///   остался только `device_id`. Чтение v1/v2 — миграция: содержимое
///   уезжает в журнал событием Genesis (born_stage = Adult).
pub const SCHEMA_VERSION: u32 = 3;

/// Персистентная запись устройства (schema v3).
/// Хранится в `$XDG_DATA_HOME/driftling/pet.json`.
///
/// Снапшот-курсор журнала намеренно не храним: питомец всегда
/// пересворачивается из журнала при старте — файлы малы (TODO(E):
/// снапшот-оптимизация для больших журналов).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PetRecord {
    /// Версия схемы файла. Файл НОВЕЕ поддерживаемой версии load()
    /// отвергает явной ошибкой, а не тихим дефолтом.
    pub schema_version: u32,
    /// Идентификатор устройства для HLC-меток журнала: случайный hex,
    /// генерируется один раз при создании/миграции записи.
    #[serde(default)]
    pub device_id: String,
}

impl PetRecord {
    pub fn new(device_id: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            device_id,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod fs {
    use super::{PetAttributes, PetRecord, SCHEMA_VERSION};
    use crate::growth::Stage;
    use crate::journal::{random_device_id, Event, EventKind, HlcClock, Journal};
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

    /// Unix-время в мс — только для файлового слоя (имена бэкапов, метки
    /// миграции); симуляция время из ОС не читает (ТЗ §3.7).
    fn wall_now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Зонд версии: у v1-файлов поля нет вовсе.
    #[derive(serde::Deserialize)]
    struct Probe {
        schema_version: Option<u32>,
    }

    /// Форма pet.json v1/v2 — читается только ради миграции в журнал.
    /// Дефолты повторяют старую запись: v1 не знал dismiss — питомец призван.
    #[derive(serde::Deserialize)]
    #[serde(default)]
    struct LegacyRecord {
        name: String,
        attributes: PetAttributes,
        summoned: bool,
    }

    impl Default for LegacyRecord {
        fn default() -> Self {
            Self {
                name: "Driftling".to_string(),
                attributes: PetAttributes::default(),
                summoned: true,
            }
        }
    }

    /// Битый pet.json: переименовать в `pet.json.corrupt-<unixts>` (улики
    /// сохраняются) и стартовать с чистого листа, не перезаписывая оригинал.
    fn backup_corrupt(
        p: &Path,
        parse_err: &serde_json::Error,
    ) -> Result<Option<PetRecord>, String> {
        let ts = wall_now_ms() / 1000;
        let backup = p.with_file_name(format!("pet.json.corrupt-{ts}"));
        match std::fs::rename(p, &backup) {
            Ok(()) => Ok(None),
            // Бэкап не удался — оригинал не трогаем и не даём его молча
            // перезаписать дефолтом.
            Err(io_err) => Err(format!(
                "pet.json is corrupt ({parse_err}) and backup failed: {io_err}"
            )),
        }
    }

    /// Миграция v1/v2 -> v3: имя/характеристики/summoned уезжают в журнал
    /// (Genesis с born_stage = Adult — взрослый не вылупляется заново,
    /// + Dismissed, если питомец был убран), pet.json переписывается как v3.
    ///
    /// Идемпотентна: если Genesis уже в журнале (краш между шагами прошлой
    /// миграции), события не дублируются.
    fn migrate_legacy(dir: &Path, legacy: LegacyRecord) -> Result<PetRecord, String> {
        let device_id = random_device_id();
        let (existing, _warnings) = Journal::open(dir)?;
        let has_genesis = existing
            .iter()
            .any(|e| matches!(e.kind, EventKind::Genesis { .. }));
        if !has_genesis {
            let mut clock = HlcClock::new(device_id.clone());
            for e in &existing {
                clock.catch_up(&e.id);
            }
            let now_ms = wall_now_ms();
            Journal::append(
                dir,
                &Event {
                    id: clock.next(now_ms),
                    kind: EventKind::Genesis {
                        name: legacy.name,
                        attributes: legacy.attributes,
                        born_stage: Stage::Adult,
                    },
                },
            )?;
            if !legacy.summoned {
                Journal::append(
                    dir,
                    &Event {
                        id: clock.next(now_ms),
                        kind: EventKind::Dismissed,
                    },
                )?;
            }
        }
        let rec = PetRecord::new(device_id);
        rec.save_in(dir)?;
        Ok(rec)
    }

    impl PetRecord {
        /// Прочитать запись из штатного каталога данных.
        pub fn load() -> Result<Option<PetRecord>, String> {
            Self::load_in(&data_dir())
        }

        /// Прочитать запись из `dir`. Семантика (ТД-15,16):
        /// - нет файла — `Ok(None)` (создание записи — за демоном);
        /// - схема новее поддерживаемой — явная ошибка (файл от более
        ///   новой версии Driftling трогать нельзя);
        /// - v1/v2 — миграция в v3 с переносом содержимого в журнал
        ///   (см. [`migrate_legacy`]);
        /// - битый JSON — файл переименовывается в
        ///   `pet.json.corrupt-<unixts>` и `Ok(None)`.
        pub fn load_in(dir: &Path) -> Result<Option<PetRecord>, String> {
            let p = path_in(dir);
            let text = match std::fs::read_to_string(&p) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.to_string()),
            };
            // Ошибки ядра — технические, по-английски (ТД-30): ядро не
            // тянет i18n (wasm-чистота), наружу их заворачивают UI-слои.
            let version = match serde_json::from_str::<Probe>(&text) {
                Ok(probe) => probe.schema_version.unwrap_or(1),
                Err(parse_err) => return backup_corrupt(&p, &parse_err),
            };
            if version > SCHEMA_VERSION {
                return Err(format!(
                    "pet.json schema v{version} is newer than supported v{SCHEMA_VERSION} — \
                     file from a newer Driftling version, leaving it untouched"
                ));
            }
            if version < SCHEMA_VERSION {
                return match serde_json::from_str::<LegacyRecord>(&text) {
                    Ok(legacy) => migrate_legacy(dir, legacy).map(Some),
                    Err(parse_err) => backup_corrupt(&p, &parse_err),
                };
            }
            match serde_json::from_str::<PetRecord>(&text) {
                Ok(mut rec) => {
                    // Пустой device_id (обрезанный руками файл) — чиним:
                    // идентификатор генерируется один раз и сохраняется.
                    if rec.device_id.is_empty() {
                        rec.device_id = random_device_id();
                        rec.save_in(dir)?;
                    }
                    Ok(Some(rec))
                }
                Err(parse_err) => backup_corrupt(&p, &parse_err),
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
        let rec = PetRecord::new("cafe0123deadbeef".to_string());
        let text = serde_json::to_string(&rec).unwrap();
        let back: PetRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(rec, back);
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.device_id, "cafe0123deadbeef");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod fs_tests {
    use super::*;
    use crate::journal::{fold, EventKind, FoldCfg, Journal};
    use crate::Stage;
    use std::path::PathBuf;

    /// Уникальный временный каталог на тест — без мутации env (ТД-26).
    fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("driftling-attrs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assert_device_id_ok(id: &str) {
        assert_eq!(id.len(), 16, "16 hex-символов: {id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "hex: {id}");
    }

    #[test]
    fn save_and_load_roundtrip_in_dir() {
        let dir = tmp_dir("roundtrip");
        assert_eq!(PetRecord::load_in(&dir).unwrap(), None, "файла ещё нет");

        let rec = PetRecord::new(crate::journal::random_device_id());
        rec.save_in(&dir).unwrap();

        let back = PetRecord::load_in(&dir).unwrap().expect("файл сохранён");
        assert_eq!(back, rec);
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_device_id_ok(&back.device_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_is_explicit_error() {
        let dir = tmp_dir("newer");
        std::fs::write(
            record_path_in(&dir),
            format!(
                r#"{{"schema_version":{},"device_id":"aa"}}"#,
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

    /// Реальный v2-фикстур (то, что писал демон фазы A): миграция в v3
    /// переносит имя/характеристики/summoned в журнал.
    #[test]
    fn v2_file_migrates_to_v3_journal() {
        let dir = tmp_dir("v2");
        std::fs::write(
            record_path_in(&dir),
            r#"{
  "schema_version": 2,
  "name": "Старожил",
  "attributes": {
    "size": 128,
    "walk_speed": 64.0,
    "curiosity": 20,
    "sleepiness": 10,
    "sleep_min": 10.0,
    "sleep_max": 60.0
  },
  "summoned": false
}"#,
        )
        .unwrap();

        let rec = PetRecord::load_in(&dir)
            .unwrap()
            .expect("миграция вернула запись");
        assert_eq!(rec.schema_version, SCHEMA_VERSION);
        assert_device_id_ok(&rec.device_id);

        // pet.json переписан как v3 и больше не содержит имени.
        let text = std::fs::read_to_string(record_path_in(&dir)).unwrap();
        assert!(text.contains(&format!("\"schema_version\": {SCHEMA_VERSION}")));
        assert!(!text.contains("Старожил"), "имя уехало в журнал: {text}");

        // Журнал: Genesis (born_stage = Adult) + Dismissed (summoned=false).
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!(warnings, 0);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0].kind,
            EventKind::Genesis {
                born_stage: Stage::Adult,
                ..
            }
        ));
        assert!(matches!(events[1].kind, EventKind::Dismissed));
        assert_eq!(events[0].id.device, rec.device_id);

        // Fold отдаёт мигрированного питомца: имя, атрибуты, стадия, dismiss.
        let now = events[1].id.wall_ms;
        let pet = fold(&events, now, &FoldCfg::default());
        assert_eq!(pet.name, "Старожил");
        assert_eq!(pet.attributes.size, 128);
        assert_eq!(pet.stage, Stage::Adult, "мигрант не вылупляется заново");
        assert!(!pet.summoned, "dismiss пережил миграцию");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// v1-файл (без schema_version/summoned): те же рельсы миграции,
    /// summoned по умолчанию true — события Dismissed нет.
    #[test]
    fn v1_file_migrates_without_dismissed() {
        let dir = tmp_dir("v1");
        std::fs::write(
            record_path_in(&dir),
            r#"{"name":"Дедуля","attributes":{"walk_speed":100.0}}"#,
        )
        .unwrap();

        let rec = PetRecord::load_in(&dir).unwrap().expect("v1 мигрирует");
        assert_device_id_ok(&rec.device_id);

        let (events, _) = Journal::open(&dir).unwrap();
        assert_eq!(events.len(), 1, "только Genesis, питомец был призван");
        let pet = fold(&events, events[0].id.wall_ms, &FoldCfg::default());
        assert_eq!(pet.name, "Дедуля");
        assert_eq!(pet.attributes.walk_speed, 100.0);
        assert!(pet.summoned);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Краш между шагами прошлой миграции: Genesis уже в журнале —
    /// повторная миграция не плодит второго.
    #[test]
    fn migration_is_idempotent_after_crash() {
        let dir = tmp_dir("idem");
        std::fs::write(
            record_path_in(&dir),
            r#"{"schema_version":2,"name":"Крашик","summoned":true}"#,
        )
        .unwrap();
        let first = PetRecord::load_in(&dir).unwrap().unwrap();
        let (events_before, _) = Journal::open(&dir).unwrap();
        assert_eq!(events_before.len(), 1);

        // Откатываем pet.json к v2, журнал оставляем — «упали до перезаписи».
        std::fs::write(
            record_path_in(&dir),
            r#"{"schema_version":2,"name":"Крашик","summoned":true}"#,
        )
        .unwrap();
        let second = PetRecord::load_in(&dir).unwrap().unwrap();
        let (events_after, _) = Journal::open(&dir).unwrap();
        assert_eq!(events_after.len(), 1, "Genesis не задублирован");
        assert_eq!(events_before, events_after);
        // device_id перегенерирован — это допустимо (важна уникальность).
        assert_device_id_ok(&first.device_id);
        assert_device_id_ok(&second.device_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v3_with_empty_device_id_is_repaired() {
        let dir = tmp_dir("empty-dev");
        std::fs::write(
            record_path_in(&dir),
            format!(r#"{{"schema_version":{SCHEMA_VERSION},"device_id":""}}"#),
        )
        .unwrap();
        let rec = PetRecord::load_in(&dir).unwrap().unwrap();
        assert_device_id_ok(&rec.device_id);
        // Починка сохранена: повторное чтение видит тот же id.
        let again = PetRecord::load_in(&dir).unwrap().unwrap();
        assert_eq!(rec, again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
