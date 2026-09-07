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
    // Зарезервировано под остальные настройки приложения (мониторы,
    // выбор пака — M4). Автозапуск управляется .desktop-файлом напрямую.
    /// Синхронизация между устройствами (фаза E, ТЗ §3.5).
    pub sync: SyncConfig,
    /// Физика мира (фаза G3).
    pub physics: PhysicsConfig,
}

/// Секция `[physics]`: единственная ручка — насколько питомец большой
/// В ЖИЗНИ. Из неё выводится масштаб мира (пикселей в метре), а значит и
/// ускорение свободного падения, и вес, и дальность броска.
///
/// ```toml
/// [physics]
/// pet_height_cm = 30    # рост взрослого питомца «в жизни»
/// ```
///
/// Меньше рост — мельче мир: на экране в 1080 px помещается меньше метров,
/// и то же самое падение выглядит быстрее. 30 см при спрайте 96 px дают
/// комнату примерно 6×3.4 м — привычные пропорции.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhysicsConfig {
    /// Рост взрослого питомца в сантиметрах.
    pub pet_height_cm: f32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            pet_height_cm: 30.0,
        }
    }
}

impl PhysicsConfig {
    /// Рост в метрах, с защитой от нулей и абсурда в файле.
    pub fn height_m(&self) -> f32 {
        (self.pet_height_cm / 100.0).clamp(0.03, 3.0)
    }
}

/// Режим синка (ТЗ §3.5): одна настройка переключает всё.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Standalone: без сети, как раньше.
    #[default]
    Off,
    /// Свой `driftling-server`: HTTP push/pull журнала + lease присутствия.
    Server,
    /// «Дешёвый уровень»: каталог журналов синкает Syncthing/Nextcloud,
    /// демон только горячо перечитывает чужие файлы.
    Folder,
}

impl SyncMode {
    /// Машинное имя режима — для статуса ctl и логов (UI локализует сам).
    pub fn as_str(self) -> &'static str {
        match self {
            SyncMode::Off => "off",
            SyncMode::Server => "server",
            SyncMode::Folder => "folder",
        }
    }
}

/// Секция `[sync]` config.toml. Все поля с дефолтами: старые конфиги без
/// секции читаются как «синк выключен».
///
/// ```toml
/// [sync]
/// mode = "server"                  # off | server | folder
/// address = "http://vps:8787"      # server: адрес driftling-server
/// token = "…"                      # server: Bearer-токен аккаунта
/// folder = "/home/u/Sync/driftling" # folder: каталог journal.*.jsonl
/// ```
///
/// Режим `folder` без `folder` наблюдает собственный каталог данных
/// (тогда синкер должен переносить ТОЛЬКО `journal.*.jsonl` — pet.json
/// хранит device_id и синкаться не должен). С указанным `folder` журнал
/// живёт в нём (pet.json и sync-state остаются локальными) — рекомендуемый
/// вариант.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SyncConfig {
    pub mode: SyncMode,
    /// Адрес driftling-server, `http://host:port`. TLS в демоне нет —
    /// селфхост ходит по LAN/VPN или локальному reverse-proxy.
    pub address: String,
    /// Bearer-токен аккаунта (выдаёт `driftling-server account add`).
    pub token: String,
    /// Каталог журналов для режима `folder`; пусто = каталог данных.
    pub folder: String,
}

impl SyncConfig {
    /// Синк вообще включён?
    pub fn enabled(&self) -> bool {
        self.mode != SyncMode::Off
    }

    /// Проблемы конфигурации, о которых стоит предупредить (не ошибки:
    /// демон работает дальше, синк просто не стартует/деградирует).
    /// Технические строки — по-английски, как все ошибки ядра (ТД-30).
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        match self.mode {
            SyncMode::Off => {}
            SyncMode::Server => {
                if self.address.trim().is_empty() {
                    out.push("sync.mode = \"server\" but sync.address is empty".to_string());
                } else if self.address.trim().starts_with("https://") {
                    out.push(
                        "sync.address uses https:// — the daemon speaks plain http \
                         (terminate TLS on a local proxy or use LAN/VPN)"
                            .to_string(),
                    );
                }
                if self.token.trim().is_empty() {
                    out.push("sync.mode = \"server\" but sync.token is empty".to_string());
                }
            }
            SyncMode::Folder => {
                if self.folder.trim().is_empty() {
                    out.push(
                        "sync.mode = \"folder\" without sync.folder: watching the data dir \
                         (sync ONLY journal.*.jsonl there, never pet.json)"
                            .to_string(),
                    );
                }
            }
        }
        out
    }
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
        assert_eq!(
            cfg.sync.mode,
            SyncMode::Off,
            "старый конфиг = синк выключен"
        );
    }

    #[test]
    fn sync_section_parses_and_roundtrips() {
        let cfg: Config = toml::from_str(
            "[sync]\nmode = \"server\"\naddress = \"http://vps:8787\"\ntoken = \"secret\"\n",
        )
        .unwrap();
        assert_eq!(cfg.sync.mode, SyncMode::Server);
        assert_eq!(cfg.sync.address, "http://vps:8787");
        assert_eq!(cfg.sync.token, "secret");
        assert_eq!(cfg.sync.folder, "");
        assert!(cfg.sync.enabled());

        // Раундтрип: save-формат читается обратно без потерь.
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);

        let cfg: Config =
            toml::from_str("[sync]\nmode = \"folder\"\nfolder = \"/sync/drl\"\n").unwrap();
        assert_eq!(cfg.sync.mode, SyncMode::Folder);
        assert_eq!(cfg.sync.folder, "/sync/drl");
    }

    #[test]
    fn sync_mode_unknown_value_is_a_readable_error() {
        // Кривое значение mode — ошибка текстом (политика Config::load),
        // а не тихий каприз.
        let err = toml::from_str::<Config>("[sync]\nmode = \"cloud\"\n").unwrap_err();
        assert!(err.to_string().contains("cloud") || !err.to_string().is_empty());
    }

    #[test]
    fn sync_warnings_catch_misconfig() {
        let mut sync = SyncConfig {
            mode: SyncMode::Server,
            ..SyncConfig::default()
        };
        let w = sync.warnings();
        assert_eq!(w.len(), 2, "пустые address и token: {w:?}");

        sync.address = "https://vps".into();
        sync.token = "t".into();
        let w = sync.warnings();
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("https"), "{w:?}");

        sync.address = "http://vps:8787".into();
        assert!(sync.warnings().is_empty());

        let folder = SyncConfig {
            mode: SyncMode::Folder,
            ..SyncConfig::default()
        };
        assert_eq!(
            folder.warnings().len(),
            1,
            "папка не указана — предупреждаем"
        );
        assert!(SyncConfig::default().warnings().is_empty(), "off молчит");
    }

    #[test]
    fn sync_mode_names_are_stable() {
        assert_eq!(SyncMode::Off.as_str(), "off");
        assert_eq!(SyncMode::Server.as_str(), "server");
        assert_eq!(SyncMode::Folder.as_str(), "folder");
    }

    #[test]
    fn legacy_attributes_migrate_and_clamp() {
        let attrs = legacy_attributes(
            "[pet]\nsize = 90\n[behavior]\nwalk_speed = 400.0\nw_idle_to_walk = 100\nw_idle_to_sleep = 0\n",
        )
        .expect("legacy-ключи должны найтись");
        assert_eq!(attrs.size, 90);
        // Скорость режется потолком движка (фаза G: было 400, стало 160);
        // «успокоение» до прогулочной — уже дело демона (PetAttributes::tamed).
        assert_eq!(attrs.walk_speed, crate::attributes::MAX_WALK_SPEED);
        assert_eq!(attrs.curiosity, 100);
        assert_eq!(attrs.sleepiness, 0);

        assert!(legacy_attributes("").is_none());
        assert!(legacy_attributes("[app]\nx = 1\n").is_none());
    }
}
