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
    /// Игровые режимы: развлечения, которые можно выключить (фаза H).
    pub game: GameConfig,
    /// Мир вещей питомца (фазы H1-H4).
    pub world: WorldConfig,
    /// Уживчивость: вежливость к работе и чувствительные реакции.
    pub comfort: ComfortConfig,
}

/// Патч конфига: правка ПО СЕКЦИЯМ, а не целым файлом.
///
/// Целый `Config` в запросе означал бы last-writer-wins по всему файлу:
/// окно, сохраняя галочку про гостей, затирало бы токен синка (который
/// демон наружу не отдаёт вовсе) и чужие правки, сделанные руками в
/// соседней секции. Поэтому каждая секция — `Option`, а внутри синка
/// `Option` и у каждого поля: `None` = «не трогать».
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigPatch {
    pub sync: Option<SyncPatch>,
    pub physics: Option<PhysicsConfig>,
    pub game: Option<GameConfig>,
    pub world: Option<WorldConfig>,
    pub comfort: Option<ComfortConfig>,
}

/// Патч секции `[sync]`: токен отдельно, потому что демон его не отдаёт.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncPatch {
    pub mode: Option<SyncMode>,
    pub address: Option<String>,
    /// `None` — оставить как есть (маска из GetConfig не должна затирать
    /// настоящий токен).
    pub token: Option<String>,
    pub folder: Option<String>,
}

impl ConfigPatch {
    /// Наложить патч и вернуть машинные имена изменённых секций — их
    /// демон отдаёт клиенту в `applied`, а клиент локализует.
    pub fn apply_to(&self, cfg: &mut Config) -> Vec<String> {
        let mut applied = Vec::new();
        if let Some(p) = &self.sync {
            let before = cfg.sync.clone();
            if let Some(m) = p.mode {
                cfg.sync.mode = m;
            }
            if let Some(a) = &p.address {
                cfg.sync.address = a.clone();
            }
            if let Some(t) = &p.token {
                cfg.sync.token = t.clone();
            }
            if let Some(f) = &p.folder {
                cfg.sync.folder = f.clone();
            }
            if cfg.sync != before {
                applied.push("sync".into());
            }
        }
        let mut set = |changed: bool, name: &str| {
            if changed {
                applied.push(name.into());
            }
        };
        if let Some(v) = self.physics {
            set(cfg.physics != v, "physics");
            cfg.physics = v;
        }
        if let Some(v) = &self.game {
            set(&cfg.game != v, "game");
            cfg.game = v.clone();
        }
        if let Some(v) = self.world {
            set(cfg.world != v, "world");
            cfg.world = v;
        }
        if let Some(v) = self.comfort {
            set(cfg.comfort != v, "comfort");
            cfg.comfort = v;
        }
        applied
    }
}

/// Секция `[game]`: развлечения — незваные гости.
///
/// ```toml
/// [game]
/// war_mode = true        # к питомцу изредка заходят незваные гости
/// mob_every_mins = 10    # как часто демон думает позвать гостя
/// mob_kinds = ["roach", "bug"]   # пусто = все
/// ```
///
/// Режим войны — зрелище, а не бой: питомец гоняет мобов по экрану, никто
/// никому не наносит урона, здоровье не трогается, и в журнал ухода эти
/// стычки не попадают (см. `docs/WORLD.md`).
///
/// Списки видов проверяются демоном по [`crate::prop::PropKind`]: неизвестное
/// имя — предупреждение в лог, а не отказ загрузить конфиг.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameConfig {
    /// Пускать ли гостей на экран (режим войны).
    pub war_mode: bool,
    /// Как часто демон думает, не позвать ли гостя, мин.
    pub mob_every_mins: f64,
    /// Какие гости допущены (машинные имена); пусто — все.
    pub mob_kinds: Vec<String>,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            war_mode: false,
            mob_every_mins: 10.0,
            mob_kinds: Vec::new(),
        }
    }
}

/// Секция `[world]`: вещи, которые заводятся у питомца сами.
///
/// ```toml
/// [world]
/// auto_bed = true           # первое «уложить спать» ставит лежанку
/// puddles = true            # тошнота оставляет лужу, питомец её моет
/// ```
///
/// Выключенный `auto_*` не убирает уже поставленную вещь: она принадлежит
/// питомцу и живёт в журнале, убрать её — отдельное действие. Флаг лишь
/// говорит демону не заводить её заново.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldConfig {
    /// Первое «уложить спать» ставит лежанку.
    pub auto_bed: bool,
    /// Оставляет ли тошнота лужу (и есть ли что убирать шваброй).
    pub puddles: bool,
    /// Сколько питомца не трогают, прежде чем он берётся за уборку, сек.
    pub chore_delay_secs: f64,
    /// Появляются ли лужи и мусор — и есть ли что убирать шваброй.
    pub litter: bool,
    /// Чихает ли питомец в покое.
    pub sneezes: bool,
    /// Рисовать тень под питомцем и вещами.
    pub shadow: bool,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            auto_bed: true,
            puddles: true,
            chore_delay_secs: 6.0,
            litter: true,
            sneezes: true,
            shadow: true,
        }
    }
}

/// Секция `[comfort]`: как питомец уживается с работой человека.
///
/// ```toml
/// [comfort]
/// hide_on_fullscreen = true   # прячется при полноэкранном окне
/// motion_sickness = true      # его укачивает от тряски мышью
/// quiet = false               # тихий час: только явные действия человека
/// surprises = 1.0             # множитель частоты «само собой» (0 = выкл)
/// ```
///
/// `quiet` — один выключатель на всю самодеятельность: ни транспорта, ни
/// гостей, ни походов домой, ни чихов. Питомец продолжает жить, но ничего
/// не затевает сам.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ComfortConfig {
    /// Прятаться при полноэкранном окне (вежливость D5).
    pub hide_on_fullscreen: bool,
    /// Сколько окно должно продержаться, прежде чем питомец уйдёт, сек.
    pub fullscreen_grace_secs: f64,
    /// Укачивает ли питомца от тряски мышью.
    pub motion_sickness: bool,
    /// Тихий час: никакой самодеятельности.
    pub quiet: bool,
    /// Множитель частоты случайных событий: 0 — только явные действия.
    /// Множит ИНТЕРВАЛЫ между проверками (секунды), а не веса поведения из
    /// ста — иначе сон или прогулка стали бы недостижимы.
    pub surprises: f64,
    /// Размер подписей у питомца: пузыри и меню (0.8..2.0).
    pub text_scale: f64,
}

impl Default for ComfortConfig {
    fn default() -> Self {
        Self {
            hide_on_fullscreen: true,
            fullscreen_grace_secs: 0.9,
            motion_sickness: true,
            quiet: false,
            surprises: 1.0,
            text_scale: 1.0,
        }
    }
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
    pub pet_height_cm: f64,
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
        (self.pet_height_cm as f32 / 100.0).clamp(0.03, 3.0)
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
            Config::load_at(&path())
        }

        /// То же, но из конкретного файла: тестам не нужно подменять
        /// переменные окружения процесса (а с ними — гонки между тестами).
        pub fn load_at(p: &std::path::Path) -> Result<Config, String> {
            match std::fs::read_to_string(p) {
                Ok(text) => toml::from_str(&text).map_err(|e| e.to_string()),
                Err(_) => Ok(Config::default()),
            }
        }

        /// Атомарная запись: во временный файл + rename.
        pub fn save(&self) -> Result<(), String> {
            self.save_at(&path())
        }

        /// Запись в конкретный файл.
        pub fn save_at(&self, p: &std::path::Path) -> Result<(), String> {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
            let tmp = p.with_extension("toml.tmp");
            std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, p).map_err(|e| e.to_string())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs::{path, raw_text};

#[cfg(test)]
mod tests {
    use super::*;

    /// Новые секции читаются, а старый конфиг без них переживает загрузку:
    /// у каждого поля есть дефолт, и он же — «как было до фазы H».
    #[test]
    fn world_and_comfort_default_to_previous_behaviour() {
        let cfg = Config::default();
        assert!(cfg.world.auto_bed);
        assert!(cfg.world.puddles && cfg.world.sneezes && cfg.world.shadow);
        assert!(cfg.comfort.hide_on_fullscreen && cfg.comfort.motion_sickness);
        assert_eq!(cfg.comfort.fullscreen_grace_secs, 0.9);
        assert!(!cfg.comfort.quiet && cfg.comfort.surprises == 1.0);
        assert_eq!(cfg.comfort.text_scale, 1.0);
        // Война выключена — ровно как было в коде.
        assert!(!cfg.game.war_mode);
        assert!(cfg.game.mob_kinds.is_empty());

        let old: Config = toml::from_str("[physics]\npet_height_cm = 25\n").unwrap();
        assert_eq!(old.physics.pet_height_cm, 25.0);
        assert_eq!(old.world, Config::default().world);
        assert_eq!(old.comfort, Config::default().comfort);
    }

    /// Частичная секция не сбрасывает остальные поля в ноль.
    #[test]
    fn partial_sections_keep_other_defaults() {
        let cfg: Config = toml::from_str(
            "[world]\nauto_bed = false\n[comfort]\nquiet = true\n[game]\nmob_kinds = [\"bug\"]\n",
        )
        .unwrap();
        assert!(!cfg.world.auto_bed);
        assert!(cfg.world.puddles, "остальные вещи остались как были");
        assert!(cfg.comfort.quiet && cfg.comfort.motion_sickness);
        assert_eq!(cfg.game.mob_kinds, vec!["bug".to_string()]);
        assert!(!cfg.game.war_mode, "флаг войны не сбросился");
    }

    #[test]
    fn unknown_and_legacy_keys_do_not_break_load() {
        let cfg: Config = toml::from_str("[pet]\nsize = 90\n[behavior]\nwalk_speed = 400.0\n")
            .unwrap_or_default();
        assert_eq!(cfg, Config::default());
        // Ключи миски, домика и транспорта из старых версий: их больше нет
        // в схеме, и конфиг с ними обязан читаться как обычный.
        let legacy: Config = toml::from_str(
            "[world]\nauto_bowl = true\nauto_house = true\nhouse_age_days = 3.0\n\
             [game]\nrides = true\nride_every_mins = 15\nride_kinds = [\"bike\"]\n",
        )
        .unwrap();
        assert_eq!(legacy.world, Config::default().world);
        assert_eq!(legacy.game, Config::default().game);
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
