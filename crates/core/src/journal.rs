//! Журнал событий ухода — **единственный источник истины** состояния
//! тамагочи (ТЗ §3.2, §3.5; фазы B1/B2/B4 ROADMAP-1.0).
//!
//! Статы никогда не хранятся — они детерминированная функция
//! `(журнал, время)`: [`fold`] сворачивает отсортированные события в
//! [`DerivedPet`]. Слияние журналов двух устройств — [`merge`], объединение
//! множеств по `id` (G-Set CRDT): коммутативно, идемпотентно, конфликты
//! невозможны по построению. Формат закладывается здесь, фаза E добавит
//! только транспорт.
//!
//! Правила чистоты (ТЗ §3.7): никакого чтения часов и ОС в логике —
//! время приходит параметрами (`wall_ms`, `now_ms`); файловое хранилище
//! отгорожено `cfg(not(target_arch = "wasm32"))`.
//!
//! # API для интеграции (демон/настройки/сервер, фаза E)
//!
//! Хранилище пер-девайсное (ТЗ §3.5, «синк через папку»): каждое
//! устройство дописывает ТОЛЬКО свой `journal.<device_id>.jsonl`,
//! чужие файлы — read-only входы слияния.
//!
//! - [`Journal::open_dir`]`(dir, own_device)` — миграция легаси-файла
//!   `journal.jsonl` (rename в файл своего устройства) + слияние журналов
//!   всех устройств: `(события отсортированы, счётчик битых строк)`;
//! - [`Journal::append`]`(dir, own_device, event)` — дописать событие в
//!   файл своего устройства (create + append + flush + fsync);
//! - [`Journal::open`]`(dir)` — read-only слияние без миграции (doctor,
//!   внешние читатели);
//! - [`HlcClock`] — выдача HLC-меток: `next(now_ms)`; после старта —
//!   [`HlcClock::catch_up`] по каждому id журнала, чтобы новые метки были
//!   строго больше уже записанных даже при откате настенных часов;
//! - [`merge`] — слияние журналов; курсоры и выборка недостающего для
//!   транспорта — модуль [`crate::sync`];
//! - [`fold`]`(events, now_ms, cfg)` — свёртка в [`DerivedPet`];
//! - [`FoldCfg::default`] — игровые константы (decay, кормление, рост);
//! - [`random_device_id`] — идентификатор устройства для pet.json v3.
//!
//! TODO(E): снапшот-курсор для больших журналов (сейчас fold всегда идёт
//! с начала — файлы малы, событий единицы в день).

use serde::{Deserialize, Serialize};

use crate::attributes::PetAttributes;
use crate::growth::{self, Stage};
use crate::palette::DEFAULT_PET_COLOR;
use crate::stats::PetStats;

const MIN_MS: u64 = 60 * 1000;
const HOUR_MS: u64 = 60 * MIN_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

/// Шаг кусочно-линейной симуляции в fold, минут. Меньше шаг — точнее
/// границы эпизодов (нули, зоны здоровья); детерминизм не зависит от шага.
const SIM_STEP_MIN: f32 = 1.0;

// ---------------------------------------------------------------------------
// HLC
// ---------------------------------------------------------------------------

/// Метка гибридных логических часов (модель jlongster/Actual Budget).
/// Производный `Ord` сравнивает поля по порядку объявления —
/// `(wall_ms, counter, device)` — это и есть полный порядок журнала.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hlc {
    /// Unix-время в миллисекундах (физическая компонента).
    pub wall_ms: u64,
    /// Логический счётчик (tie-break внутри одной миллисекунды).
    pub counter: u16,
    /// Идентификатор устройства (см. [`random_device_id`]).
    pub device: String,
}

/// Генератор HLC-меток одного устройства: метки строго возрастают, даже
/// если настенные часы прыгнули назад (NTP, ручной перевод).
///
/// Время не читается изнутри — `now_ms` приходит параметром (ТЗ §3.7).
#[derive(Debug, Clone)]
pub struct HlcClock {
    last: Hlc,
}

impl HlcClock {
    pub fn new(device: impl Into<String>) -> Self {
        Self {
            last: Hlc {
                wall_ms: 0,
                counter: 0,
                device: device.into(),
            },
        }
    }

    /// Следующая метка: `wall = max(now_ms, last.wall)`; в пределах одной
    /// миллисекунды растёт счётчик (при переполнении — уходим в следующую мс).
    ///
    /// TODO(E): drift-guard ±60 с — события «из будущего» отвергать с
    /// понятной ошибкой «проверь часы на устройстве X» (ТЗ §3.5).
    pub fn next(&mut self, now_ms: u64) -> Hlc {
        let wall = now_ms.max(self.last.wall_ms);
        let (wall, counter) = if wall == self.last.wall_ms {
            match self.last.counter.checked_add(1) {
                Some(c) => (wall, c),
                None => (wall + 1, 0),
            }
        } else {
            (wall, 0)
        };
        self.last.wall_ms = wall;
        self.last.counter = counter;
        self.last.clone()
    }

    /// Идентификатор устройства этих часов — он же владелец файла
    /// `journal.<device>.jsonl` (контракт single-writer, ТЗ §3.5).
    pub fn device(&self) -> &str {
        &self.last.device
    }

    /// Подтянуть часы под уже виденную метку (свою из журнала при старте
    /// или чужую при синке): следующий `next()` будет строго больше `seen`
    /// по `(wall_ms, counter)` — повторная выдача старых id исключена.
    pub fn catch_up(&mut self, seen: &Hlc) {
        if (seen.wall_ms, seen.counter) > (self.last.wall_ms, self.last.counter) {
            self.last.wall_ms = seen.wall_ms;
            self.last.counter = seen.counter;
        }
    }
}

/// Случайный идентификатор устройства для HLC-меток: 16 hex-символов,
/// генерируется один раз на установку и хранится в pet.json (schema v3).
pub fn random_device_id() -> String {
    format!("{:016x}", fastrand::u64(..))
}

// ---------------------------------------------------------------------------
// События
// ---------------------------------------------------------------------------

/// Событие ухода. Журнал append-only; слияние — объединение множеств по
/// `id` ([`merge`]). `id` глобально уникален по построению HLC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: Hlc,
    pub kind: EventKind,
}

/// Виды событий. Сериализация — внутренний тег `type`, чтобы строки
/// journal.jsonl оставались самоописываемыми и расширяемыми.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventKind {
    /// Рождение питомца; время рождения = `id.wall_ms`.
    /// `born_stage`: [`Stage::Egg`] для новых питомцев, [`Stage::Adult`]
    /// при миграции существующего pet.json v1/v2 — взрослый не «вылупляется
    /// заново» (стадия не опускается ниже born_stage, growth.rs).
    Genesis {
        name: String,
        attributes: PetAttributes,
        born_stage: Stage,
    },
    /// Покормили: `treat = false` — обычная еда, `true` — вкусняшка
    /// (настроение↑, злоупотребление бьёт по здоровью — приём P1).
    Fed { treat: bool },
    /// Поиграли (настроение↑, энергия↓).
    Played,
    /// Уложили спать из меню; сам сон энергии не даёт — её вернёт [`EventKind::Slept`].
    PutToSleep,
    /// Демон пишет по окончании периода сна: сколько минут проспал
    /// (восстановление энергии).
    Slept { minutes: f32 },
    /// Погладили (настроение↑ с часовым потолком — анти-фарм).
    Petted,
    /// Переименовали.
    Renamed { name: String },
    /// Призвали на экран.
    Summoned,
    /// Убрали с экрана (ТД-17).
    Dismissed,
    /// Характеристики выставлены напрямую — только дебаг-панель (IPC
    /// `SetAttributes`); игровая прокачка — B5.
    AttributesSet { attributes: PetAttributes },
    /// Перекрасили питомца: базовый цвет тела (ARGB8888). Косметическая
    /// пользовательская настройка (не дебаг); альфу fold нормализует в ff.
    /// Событие аддитивное: старые журналы его просто не содержат — тогда
    /// действует [`DEFAULT_PET_COLOR`].
    Recolored { argb: u32 },
}

// ---------------------------------------------------------------------------
// Слияние (G-Set)
// ---------------------------------------------------------------------------

/// Слить чужой журнал в свой: объединение множеств по `id` + сортировка.
/// Для корректных журналов (id уникальны — гарантия HLC) операция
/// коммутативна и идемпотентна — это будущий движок синка (фаза E).
pub fn merge(into: &mut Vec<Event>, from: &[Event]) {
    into.extend(from.iter().cloned());
    into.sort_by(|a, b| a.id.cmp(&b.id));
    into.dedup_by(|a, b| a.id == b.id);
}

// ---------------------------------------------------------------------------
// Fold: журнал -> состояние
// ---------------------------------------------------------------------------

/// Игровые константы свёртки (ТЗ §3.2). Все числа — стартовый баланс,
/// правится здесь в одном месте.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldCfg {
    /// Ускорение роста для дебага: множитель эффективного возраста
    /// (1.0 = реальное время; 60.0 — «минута за час»).
    pub growth_scale: f64,
    /// Сытость 100 -> 0 за столько часов.
    pub satiety_decay_hours: f32,
    /// Энергия 100 -> 0 за столько часов.
    pub energy_decay_hours: f32,
    /// Настроение 100 -> 0 за столько часов.
    pub mood_decay_hours: f32,
    /// Разрыв между событиями дольше этого (минут) считается «оффлайном».
    pub offline_gap_min: f32,
    /// Пол оффлайн-пересчёта: разрыв не роняет стат ниже этого уровня
    /// («голодный, но живой»); уже просевший стат разрыв не трогает.
    pub offline_floor: f32,
    /// Обычная еда: мгновенная часть сытости...
    pub feed_instant: f32,
    /// ...и отложенная (капает в пул, анти-перекорм — модель VPet).
    pub feed_trickle: f32,
    /// Скорость усвоения пула, ед/мин.
    pub trickle_per_min: f32,
    /// Вкусняшка: мгновенная сытость.
    pub treat_satiety: f32,
    /// Вкусняшка: мгновенное настроение.
    pub treat_mood: f32,
    /// Столько вкусняшек за скользящие сутки — без последствий.
    pub treat_limit_24h: u32,
    /// Каждая сверх лимита: столько здоровья долой.
    pub treat_overuse_health: f32,
    /// Игра: настроение↑.
    pub play_mood: f32,
    /// Игра: энергия↓ (пол 0).
    pub play_energy_cost: f32,
    /// Поглаживание: настроение↑ за раз...
    pub pet_mood: f32,
    /// ...но не больше этого за скользящий час (анти-фарм).
    pub pet_mood_cap_hourly: f32,
    /// Сон: энергия за минуту сна.
    pub sleep_energy_per_min: f32,
    /// Здоровье: падение в час, пока сытость == 0 или настроение == 0.
    pub health_drop_per_hour: f32,
    /// Здоровье: восстановление в час, пока сытость и настроение выше порога.
    pub health_regen_per_hour: f32,
    /// Порог «хорошего ухода» для восстановления здоровья.
    pub health_regen_above: f32,
    /// Здоровье ниже этого = болезнь (смерти нет, ТЗ §3.2).
    pub ill_below: f32,
}

impl Default for FoldCfg {
    fn default() -> Self {
        Self {
            growth_scale: 1.0,
            satiety_decay_hours: 8.0,
            energy_decay_hours: 10.0,
            mood_decay_hours: 12.0,
            offline_gap_min: 30.0,
            offline_floor: 20.0,
            feed_instant: 17.5,
            feed_trickle: 17.5,
            trickle_per_min: 0.5,
            treat_satiety: 10.0,
            treat_mood: 20.0,
            treat_limit_24h: 3,
            treat_overuse_health: 3.0,
            play_mood: 25.0,
            play_energy_cost: 15.0,
            pet_mood: 4.0,
            pet_mood_cap_hourly: 12.0,
            sleep_energy_per_min: 0.8,
            health_drop_per_hour: 6.0,
            health_regen_per_hour: 3.0,
            health_regen_above: 60.0,
            ill_below: 40.0,
        }
    }
}

/// Результат свёртки журнала: всё, что демону нужно знать о питомце.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedPet {
    pub name: String,
    pub attributes: PetAttributes,
    pub stage: Stage,
    pub stats: PetStats,
    /// Питомец призван на экран (false = убран через dismiss, ТД-17).
    pub summoned: bool,
    /// Ошибки ухода: сколько раз видимый стат падал в 0 (по разу на
    /// «нулевой эпизод» на стат). Определит ветку взрослой формы (C2).
    pub care_mistakes: u32,
    /// Время рождения (wall_ms первого Genesis).
    pub born_ms: u64,
    /// Болезнь: скрытое здоровье ниже порога (смерти нет).
    pub ill: bool,
    /// Базовый цвет тела (ARGB, альфа ff): [`DEFAULT_PET_COLOR`],
    /// пока питомца не перекрасили ([`EventKind::Recolored`]).
    pub color: u32,
}

/// Индексы видимых статов в массивах пола/эпизодов.
const SAT: usize = 0;
const ENERGY: usize = 1;
const MOOD: usize = 2;

/// Рабочее состояние свёртки (приватное: снаружи виден только DerivedPet).
struct FoldState {
    name: String,
    attributes: PetAttributes,
    stats: PetStats,
    summoned: bool,
    born_ms: Option<u64>,
    born_stage: Stage,
    care_mistakes: u32,
    /// Стат сейчас в «нулевом эпизоде» (ошибка уже засчитана).
    zero_episode: [bool; 3],
    /// Несъеденная половина еды, капающая в сытость (анти-перекорм).
    pending_food: f32,
    /// Времена вкусняшек за скользящие сутки (лимит без последствий).
    treats: Vec<u64>,
    /// Поглаживания за скользящий час: (когда, сколько настроения дало).
    pets: Vec<(u64, f32)>,
    /// Базовый цвет тела (ARGB, альфа ff).
    color: u32,
}

impl FoldState {
    fn new() -> Self {
        Self {
            // Нейтральный фолбэк на случай журнала без Genesis; штатно имя
            // задаёт Genesis (локализованное имя даёт демон, ТД-30).
            name: "Driftling".to_string(),
            attributes: PetAttributes::default(),
            stats: PetStats::default(),
            summoned: true,
            born_ms: None,
            born_stage: Stage::Egg,
            care_mistakes: 0,
            zero_episode: [false; 3],
            pending_food: 0.0,
            treats: Vec::new(),
            pets: Vec::new(),
            color: DEFAULT_PET_COLOR,
        }
    }

    /// Учёт ошибок ухода: +1 при каждом падении видимого стата в 0
    /// (по разу на нулевой эпизод; подъём выше нуля закрывает эпизод).
    fn note_zero_episodes(&mut self) {
        let values = [self.stats.satiety, self.stats.energy, self.stats.mood];
        for (flag, value) in self.zero_episode.iter_mut().zip(values) {
            let zero = value <= 0.0;
            if zero && !*flag {
                self.care_mistakes += 1;
                *flag = true;
            } else if !zero && *flag {
                *flag = false;
            }
        }
    }
}

/// Один шаг непрерывной симуляции (`dt_min` ≤ [`SIM_STEP_MIN`]).
/// `floors` — пол каждого видимого стата на этом разрыве (0 или оффлайн-пол).
fn sim_step(st: &mut FoldState, dt_min: f32, floors: &[f32; 3], cfg: &FoldCfg) {
    // Отложенная половина еды всасывается постепенно (анти-перекорм).
    if st.pending_food > 0.0 {
        let fed = st.pending_food.min(cfg.trickle_per_min * dt_min);
        st.pending_food -= fed;
        st.stats.satiety += fed;
    }
    // Линейная деградация: «100 -> 0 за N часов» (ТЗ §3.2).
    st.stats.satiety -= dt_min * 100.0 / (cfg.satiety_decay_hours * 60.0);
    st.stats.energy -= dt_min * 100.0 / (cfg.energy_decay_hours * 60.0);
    st.stats.mood -= dt_min * 100.0 / (cfg.mood_decay_hours * 60.0);
    st.stats.satiety = st.stats.satiety.clamp(floors[SAT], 100.0);
    st.stats.energy = st.stats.energy.clamp(floors[ENERGY], 100.0);
    st.stats.mood = st.stats.mood.clamp(floors[MOOD], 100.0);
    // Скрытое здоровье: нули бьют, хороший уход лечит.
    let rate_per_hour = if st.stats.satiety <= 0.0 || st.stats.mood <= 0.0 {
        -cfg.health_drop_per_hour
    } else if st.stats.satiety > cfg.health_regen_above && st.stats.mood > cfg.health_regen_above {
        cfg.health_regen_per_hour
    } else {
        0.0
    };
    st.stats.health = (st.stats.health + rate_per_hour * dt_min / 60.0).clamp(0.0, 100.0);
    st.note_zero_episodes();
}

/// Продвинуть симуляцию через разрыв между событиями (или хвост до `now`).
fn advance(st: &mut FoldState, from_ms: u64, to_ms: u64, cfg: &FoldCfg) {
    if to_ms <= from_ms {
        return;
    }
    let gap_min = (to_ms - from_ms) as f32 / MIN_MS as f32;
    // Оффлайн-пол (ТЗ §3.2): большой разрыв применяет деградацию, но не
    // добивает статы ниже пола «из-за этого разрыва»; стат, просевший ещё
    // до разрыва, остаётся где был. Короткие разрывы честно ведут в 0.
    let floors = if gap_min > cfg.offline_gap_min {
        [
            st.stats.satiety.min(cfg.offline_floor),
            st.stats.energy.min(cfg.offline_floor),
            st.stats.mood.min(cfg.offline_floor),
        ]
    } else {
        [0.0; 3]
    };
    let mut remaining = gap_min;
    while remaining > 0.0 {
        let dt = remaining.min(SIM_STEP_MIN);
        sim_step(st, dt, &floors, cfg);
        remaining -= dt;
    }
}

/// Применить мгновенный эффект события.
fn apply(st: &mut FoldState, ev: &Event, cfg: &FoldCfg) {
    let t = ev.id.wall_ms;
    match &ev.kind {
        EventKind::Genesis {
            name,
            attributes,
            born_stage,
        } => {
            if st.born_ms.is_none() {
                st.born_ms = Some(t);
                st.born_stage = *born_stage;
                st.name = name.clone();
                st.attributes = *attributes;
            }
            // TODO(E): политика слияния двух Genesis с разных устройств —
            // сейчас побеждает первый по HLC, остальные игнорируются.
        }
        EventKind::Fed { treat } => {
            if *treat {
                st.stats.satiety = (st.stats.satiety + cfg.treat_satiety).min(100.0);
                st.stats.mood = (st.stats.mood + cfg.treat_mood).min(100.0);
                st.treats.retain(|&ts| ts + DAY_MS > t);
                st.treats.push(t);
                if st.treats.len() as u32 > cfg.treat_limit_24h {
                    // Перекорм сладким: каждая лишняя за сутки бьёт по
                    // здоровью (приём Tamagotchi P1).
                    st.stats.health = (st.stats.health - cfg.treat_overuse_health).max(0.0);
                }
            } else {
                st.stats.satiety = (st.stats.satiety + cfg.feed_instant).min(100.0);
                // TODO(баланс): пул не ограничен — спам едой растянет
                // усвоение; лимит появится с балансировкой B3.
                st.pending_food += cfg.feed_trickle;
            }
        }
        EventKind::Played => {
            st.stats.mood = (st.stats.mood + cfg.play_mood).min(100.0);
            st.stats.energy = (st.stats.energy - cfg.play_energy_cost).max(0.0);
        }
        // Сам факт укладывания; энергию вернёт Slept по окончании сна.
        EventKind::PutToSleep => {}
        EventKind::Slept { minutes } => {
            let gain = minutes.max(0.0) * cfg.sleep_energy_per_min;
            st.stats.energy = (st.stats.energy + gain).min(100.0);
        }
        EventKind::Petted => {
            st.pets.retain(|&(ts, _)| ts + HOUR_MS > t);
            let granted: f32 = st.pets.iter().map(|&(_, g)| g).sum();
            let grant = cfg
                .pet_mood
                .min((cfg.pet_mood_cap_hourly - granted).max(0.0));
            if grant > 0.0 {
                st.stats.mood = (st.stats.mood + grant).min(100.0);
                st.pets.push((t, grant));
            }
        }
        EventKind::Renamed { name } => st.name = name.clone(),
        EventKind::Summoned => st.summoned = true,
        EventKind::Dismissed => st.summoned = false,
        EventKind::AttributesSet { attributes } => st.attributes = *attributes,
        // Альфа принудительно ff: цвет тела всегда непрозрачен.
        EventKind::Recolored { argb } => st.color = 0xff00_0000 | (argb & 0x00ff_ffff),
    }
    st.note_zero_episodes();
}

/// Детерминированная свёртка журнала в состояние питомца.
///
/// `events` должны быть отсортированы по `id` (гарантия [`Journal::open`]
/// и [`merge`]); `now_ms` — текущее настенное время. Чистая функция: одни
/// и те же аргументы всегда дают бит-в-бит одинаковый [`DerivedPet`].
pub fn fold(events: &[Event], now_ms: u64, cfg: &FoldCfg) -> DerivedPet {
    debug_assert!(
        events.windows(2).all(|w| w[0].id <= w[1].id),
        "fold ожидает журнал, отсортированный по id"
    );
    let mut st = FoldState::new();
    let mut cursor: Option<u64> = None;
    for ev in events {
        let t = ev.id.wall_ms;
        if let Some(c) = cursor {
            advance(&mut st, c, t, cfg);
        }
        apply(&mut st, ev, cfg);
        cursor = Some(cursor.map_or(t, |c| c.max(t)));
    }
    if let Some(c) = cursor {
        advance(&mut st, c, now_ms, cfg);
    }
    let born_ms = st
        .born_ms
        .or_else(|| events.first().map(|e| e.id.wall_ms))
        .unwrap_or(now_ms);
    // Дебаг-ускорение роста: масштабируется эффективный возраст.
    let age_scaled_ms = (now_ms.saturating_sub(born_ms) as f64 * cfg.growth_scale.max(0.0)) as u64;
    let ill = st.stats.health < cfg.ill_below;
    DerivedPet {
        name: st.name,
        attributes: st.attributes,
        stage: growth::stage(age_scaled_ms, st.born_stage),
        stats: st.stats,
        summoned: st.summoned,
        care_mistakes: st.care_mistakes,
        born_ms,
        ill,
        color: st.color,
    }
}

// ---------------------------------------------------------------------------
// Хранилище: journal.<device_id>.jsonl (append-only файл на устройство)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod fs {
    use super::Event;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    /// Легаси-путь одиночного журнала (формат до фазы E). Оставлен ради
    /// миграции и внешних читателей; [`Journal::open_dir`] уводит этот
    /// файл в журнал своего устройства.
    pub fn journal_path_in(dir: &Path) -> PathBuf {
        dir.join("journal.jsonl")
    }

    /// Журнал конкретного устройства: `journal.<device_id>.jsonl` внутри
    /// произвольного каталога данных (DI для тестов, ТД-26; штатный
    /// каталог — `attributes::data_dir()`).
    ///
    /// Контракт синка (ТЗ §3.5): устройство дописывает ТОЛЬКО свой файл
    /// (single-writer — конфликты файлового синка недостижимы по
    /// построению), чужие файлы — read-only входы слияния при чтении.
    pub fn device_journal_path_in(dir: &Path, device: &str) -> PathBuf {
        dir.join(format!("journal.{device}.jsonl"))
    }

    /// CRC32 (IEEE, как в gzip/png), побитово и без таблиц: строки журнала
    /// короткие, скорость роли не играет — важен ноль зависимостей.
    pub(super) fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    /// Строка журнала на диске: `<JSON события>\t#<crc32 hex8>`.
    /// serde_json экранирует управляющие символы, поэтому сырой TAB в
    /// строке может быть только нашим разделителем суффикса.
    pub(super) fn encode_line(ev: &Event) -> Result<String, String> {
        let json = serde_json::to_string(ev).map_err(|e| e.to_string())?;
        let crc = crc32(json.as_bytes());
        Ok(format!("{json}\t#{crc:08x}"))
    }

    /// Разбор строки журнала. CRC-суффикс ОПЦИОНАЛЕН: строки старого
    /// формата (без суффикса) остаются валидными; при наличии суффикса
    /// чексумма проверяется — несовпадение или кривой суффикс = битая
    /// строка (`None` -> предупреждение у читателя). Это защита от
    /// «рваного хвоста» и тихой порчи при снапшоте файловым синкером.
    pub(super) fn decode_line(line: &str) -> Option<Event> {
        let payload = match line.rsplit_once('\t') {
            None => line,
            Some((payload, tail)) => {
                let hex = tail.strip_prefix('#')?;
                if hex.len() != 8 {
                    return None;
                }
                let stored = u32::from_str_radix(hex, 16).ok()?;
                if crc32(payload.as_bytes()) != stored {
                    return None;
                }
                payload
            }
        };
        serde_json::from_str(payload).ok()
    }

    /// Все журнальные файлы каталога — легаси `journal.jsonl` и
    /// `journal.<device>.jsonl`, отсортированы по имени: порядок чтения
    /// (а значит и результат слияния при равных id) детерминирован.
    fn journal_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("journal dir read failed: {e}")),
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("journal dir read failed: {e}"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let is_device = name.len() > "journal..jsonl".len()
                && name.starts_with("journal.")
                && name.ends_with(".jsonl");
            if (name == "journal.jsonl" || is_device) && entry.path().is_file() {
                files.push(entry.path());
            }
        }
        files.sort();
        Ok(files)
    }

    /// Прочитать один файл журнала в общий аккумулятор: битые строки
    /// пропускаются и считаются (`warnings`), пустые игнорируются.
    fn read_into(path: &Path, events: &mut Vec<Event>, warnings: &mut u32) -> Result<(), String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            // Файл могли убрать между листингом и чтением (файловый
            // синкер, параллельная миграция) — не ошибка.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("journal read failed ({}): {e}", path.display())),
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match decode_line(line) {
                Some(ev) => events.push(ev),
                None => *warnings += 1,
            }
        }
        Ok(())
    }

    /// Дозаписать строки в файл одним write (+ flush + fsync). Если
    /// предыдущая запись оборвана без перевода строки, блок начинается
    /// с '\n' — битым остаётся только старый хвост.
    fn append_lines(path: &Path, lines: &[String]) -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)
            .map_err(|e| format!("journal open failed: {e}"))?;
        let mut block = String::new();
        let len = file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        if len > 0 {
            let mut last = [0u8; 1];
            file.seek(SeekFrom::End(-1)).map_err(|e| e.to_string())?;
            file.read_exact(&mut last).map_err(|e| e.to_string())?;
            if last != *b"\n" {
                block.push('\n');
            }
        }
        for line in lines {
            block.push_str(line);
            block.push('\n');
        }
        file.write_all(block.as_bytes())
            .map_err(|e| format!("journal append failed: {e}"))?;
        file.flush().map_err(|e| e.to_string())?;
        file.sync_data().map_err(|e| e.to_string())
    }

    /// Миграция одиночного `journal.jsonl` в файл своего устройства.
    ///
    /// Обычный путь — атомарный rename. Если файл устройства уже
    /// существует (каталог склеен из бэкапов), строки легаси-файла
    /// дописываются в него байт-в-байт и легаси удаляется: дубли схлопнет
    /// чтение (журнал — множество по id), строки без чексумм остаются
    /// валидными. Краш между append и remove не теряет данных — оба файла
    /// читаются при каждом открытии.
    fn migrate_legacy(dir: &Path, own_device: &str) -> Result<(), String> {
        let legacy = journal_path_in(dir);
        if !legacy.exists() {
            return Ok(());
        }
        let own = device_journal_path_in(dir, own_device);
        if !own.exists() {
            return std::fs::rename(&legacy, &own)
                .map_err(|e| format!("journal migration failed: {e}"));
        }
        let text = std::fs::read_to_string(&legacy)
            .map_err(|e| format!("journal migration failed: {e}"))?;
        let lines: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        if !lines.is_empty() {
            append_lines(&own, &lines)?;
        }
        std::fs::remove_file(&legacy).map_err(|e| format!("journal migration failed: {e}"))
    }

    /// Файловый журнал: append-only файл **на устройство**
    /// (`journal.<device_id>.jsonl`), по событию на строку с опциональной
    /// CRC32-чексуммой. Свой файл только дописывается, чужие (принесённые
    /// Syncthing/Nextcloud) читаются как read-only входы слияния — уровень
    /// «синк через папку» из ТЗ §3.5 работает без сервера.
    pub struct Journal;

    impl Journal {
        /// Read-only слияние всех журнальных файлов каталога (легаси
        /// `journal.jsonl` + все `journal.*.jsonl`): `(события по id без
        /// дублей, счётчик пропущенных битых строк)`. Ничего не пишет и не
        /// мигрирует — путь читателей (ctl doctor); демон и миграция
        /// pet.json открывают журнал через [`Journal::open_dir`].
        ///
        /// Битая строка (рваный хвост, несошедшаяся чексумма) — не повод
        /// терять журнал: она пропускается и считается, остальное
        /// читается. Нет каталога/файлов — пустой журнал.
        pub fn open(dir: &Path) -> Result<(Vec<Event>, u32), String> {
            let mut events = Vec::new();
            let mut warnings = 0u32;
            for path in journal_files(dir)? {
                read_into(&path, &mut events, &mut warnings)?;
            }
            // Стабильная сортировка поверх чтения файлов по имени:
            // слияние детерминировано, дубль id схлопывается.
            events.sort_by(|a, b| a.id.cmp(&b.id));
            events.dedup_by(|a, b| a.id == b.id);
            Ok((events, warnings))
        }

        /// Открыть журнал устройством `own_device`: миграция легаси-файла
        /// в `journal.<own_device>.jsonl`, затем слияние всех файлов
        /// каталога как в [`Journal::open`].
        pub fn open_dir(dir: &Path, own_device: &str) -> Result<(Vec<Event>, u32), String> {
            migrate_legacy(dir, own_device)?;
            Self::open(dir)
        }

        /// Дописать событие в файл СВОЕГО устройства: создать при
        /// необходимости, append + flush + fsync — краш не теряет уже
        /// подтверждённое событие (ТЗ §7). Строка получает CRC32-суффикс
        /// (`\t#hex8`).
        ///
        /// Контракт single-writer: `ev.id.device` обязан совпадать с
        /// `own_device` — чужие события в свой файл не пишутся никогда
        /// (их доставляет транспорт синка, фаза E).
        pub fn append(dir: &Path, own_device: &str, ev: &Event) -> Result<(), String> {
            if ev.id.device != own_device {
                return Err(format!(
                    "journal append refused: event device {:?} != own device {own_device:?} \
                     (single-writer contract)",
                    ev.id.device
                ));
            }
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let line = encode_line(ev)?;
            append_lines(&device_journal_path_in(dir, own_device), &[line])
        }

        /// Материализовать на диске события, ПРИВЕЗЁННЫЕ транспортом синка
        /// (pull с сервера, фаза E): каждое дописывается в файл СВОЕГО
        /// устройства-автора (`journal.<ev.id.device>.jsonl`), с теми же
        /// CRC32-суффиксами и fsync, что и [`Journal::append`].
        ///
        /// Это НЕ нарушение single-writer (ТЗ §3.5): правило «устройство
        /// дописывает только свой файл» действует на уровне транспорта
        /// файлового синка — локальная реплика чужого append-only лога,
        /// пополняемая pull-ом, и есть тот самый read-only вход слияния,
        /// просто доставленный HTTP, а не Syncthing. В режиме «папки»
        /// демон этот метод не зовёт — там чужие файлы приносит синкер.
        ///
        /// Дедупликацию гарантирует вызывающий (передаёт только события,
        /// которых нет в каталоге): чтение схлопнуло бы дубли строк, но
        /// файлы росли бы зря. События группируются по устройству — один
        /// append-блок (и fsync) на файл.
        pub fn append_remote(dir: &Path, events: &[Event]) -> Result<(), String> {
            if events.is_empty() {
                return Ok(());
            }
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let mut per_device: std::collections::BTreeMap<&str, Vec<String>> =
                std::collections::BTreeMap::new();
            for ev in events {
                if ev.id.device.is_empty() {
                    return Err("journal append_remote refused: empty device id".to_string());
                }
                per_device
                    .entry(ev.id.device.as_str())
                    .or_default()
                    .push(encode_line(ev)?);
            }
            for (device, lines) in per_device {
                append_lines(&device_journal_path_in(dir, device), &lines)?;
            }
            Ok(())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs::{device_journal_path_in, journal_path_in, Journal};

// ---------------------------------------------------------------------------
// Тесты
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = MIN_MS;
    const HOUR: u64 = HOUR_MS;

    fn ev(t: u64, kind: EventKind) -> Event {
        Event {
            id: Hlc {
                wall_ms: t,
                counter: 0,
                device: "test".into(),
            },
            kind,
        }
    }

    fn ev_on(device: &str, t: u64, counter: u16, kind: EventKind) -> Event {
        Event {
            id: Hlc {
                wall_ms: t,
                counter,
                device: device.into(),
            },
            kind,
        }
    }

    fn genesis(t: u64) -> Event {
        ev(
            t,
            EventKind::Genesis {
                name: "Тестик".into(),
                attributes: PetAttributes::default(),
                born_stage: Stage::Egg,
            },
        )
    }

    /// События-заполнители каждые 30 мин: разрывы остаются «короткими»
    /// (без оффлайн-пола), сами события статов не трогают.
    fn fillers(from_min: u64, to_min: u64) -> Vec<Event> {
        (1..)
            .map(|i| from_min + i * 30)
            .take_while(|&m| m <= to_min)
            .map(|m| ev(m * MIN, EventKind::PutToSleep))
            .collect()
    }

    fn approx(value: f32, expected: f32, eps: f32) -> bool {
        (value - expected).abs() <= eps
    }

    // ---- HLC ----

    #[test]
    fn hlc_ord_is_wall_counter_device() {
        let a = Hlc {
            wall_ms: 1,
            counter: 5,
            device: "z".into(),
        };
        let b = Hlc {
            wall_ms: 2,
            counter: 0,
            device: "a".into(),
        };
        assert!(a < b, "wall_ms важнее счётчика и устройства");
        let c = Hlc {
            wall_ms: 1,
            counter: 6,
            device: "a".into(),
        };
        assert!(a < c, "при равном wall_ms решает счётчик");
        let d = Hlc {
            wall_ms: 1,
            counter: 5,
            device: "za".into(),
        };
        assert!(a < d, "последний tie-break — устройство");
    }

    #[test]
    fn hlc_clock_is_monotonic_against_backward_wall_clock() {
        let mut clock = HlcClock::new("dev");
        let first = clock.next(1_000);
        // Настенные часы прыгнули назад — метки всё равно растут.
        let second = clock.next(500);
        let third = clock.next(400);
        assert!(first < second && second < third);
        assert_eq!(second.wall_ms, 1_000);
        assert_eq!(second.counter, 1);
        assert_eq!(third.counter, 2);
        // Часы догнали — счётчик сбрасывается.
        let fourth = clock.next(2_000);
        assert_eq!((fourth.wall_ms, fourth.counter), (2_000, 0));
    }

    #[test]
    fn hlc_clock_counter_overflow_moves_to_next_ms() {
        let mut clock = HlcClock::new("dev");
        clock.catch_up(&Hlc {
            wall_ms: 5,
            counter: u16::MAX,
            device: "other".into(),
        });
        let next = clock.next(5);
        assert_eq!((next.wall_ms, next.counter), (6, 0));
    }

    #[test]
    fn hlc_clock_catch_up_keeps_ids_after_seen() {
        let mut clock = HlcClock::new("aaa");
        let seen = Hlc {
            wall_ms: 9_000,
            counter: 3,
            device: "zzz".into(),
        };
        clock.catch_up(&seen);
        let next = clock.next(100); // часы этого устройства отстают
        assert!(next > seen, "новая метка строго больше виденной");
        assert_eq!(next.device, "aaa", "устройство остаётся своим");
    }

    // ---- Сериализация ----

    #[test]
    fn event_json_is_tagged_and_roundtrips() {
        let event = ev(42, EventKind::Fed { treat: true });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"Fed""#), "внутренний тег: {json}");
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);

        let slept = ev(43, EventKind::Slept { minutes: 12.5 });
        let back: Event = serde_json::from_str(&serde_json::to_string(&slept).unwrap()).unwrap();
        assert_eq!(back, slept);

        let recolored = ev(
            44,
            EventKind::Recolored {
                argb: 0xff_e8_94_4a,
            },
        );
        let json = serde_json::to_string(&recolored).unwrap();
        assert!(json.contains(r#""type":"Recolored""#), "{json}");
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, recolored);
    }

    // ---- merge ----

    #[test]
    fn merge_is_commutative() {
        let a = vec![
            ev_on("a", 1, 0, EventKind::Petted),
            ev_on(
                "a",
                4,
                0,
                EventKind::Recolored {
                    argb: 0xff_5f_bf_8f,
                },
            ),
            ev_on("a", 5, 0, EventKind::Played),
        ];
        let b = vec![
            ev_on("b", 2, 0, EventKind::Fed { treat: false }),
            ev_on("b", 5, 0, EventKind::Dismissed),
        ];
        let mut ab = a.clone();
        merge(&mut ab, &b);
        let mut ba = b.clone();
        merge(&mut ba, &a);
        assert_eq!(ab, ba);
        assert_eq!(ab.len(), 5);
    }

    #[test]
    fn merge_is_idempotent() {
        let mut log = vec![
            ev_on("a", 1, 0, EventKind::Petted),
            ev_on("a", 2, 0, EventKind::Played),
        ];
        let copy = log.clone();
        merge(&mut log, &copy);
        assert_eq!(log, copy, "слияние с самим собой ничего не меняет");
        merge(&mut log, &copy);
        assert_eq!(log, copy);
    }

    #[test]
    fn merge_sorts_by_id() {
        let mut log = vec![ev_on("a", 9, 0, EventKind::Petted)];
        merge(
            &mut log,
            &[
                ev_on("b", 3, 1, EventKind::Played),
                ev_on("b", 3, 0, EventKind::Summoned),
            ],
        );
        let ids: Vec<u64> = log.iter().map(|e| e.id.wall_ms).collect();
        assert_eq!(ids, vec![3, 3, 9]);
        assert!(log[0].id.counter < log[1].id.counter);
    }

    // ---- fold: базовые свойства ----

    #[test]
    fn fold_is_deterministic_bit_for_bit() {
        let mut events = vec![
            genesis(0),
            ev(10 * MIN, EventKind::Fed { treat: false }),
            ev(20 * MIN, EventKind::Fed { treat: true }),
            ev(
                30 * MIN,
                EventKind::Recolored {
                    argb: 0xff_e8_94_4a,
                },
            ),
            ev(40 * MIN, EventKind::Played),
            ev(41 * MIN, EventKind::Petted),
            ev(
                3 * HOUR,
                EventKind::Renamed {
                    name: "Йо".into()
                },
            ),
            ev(5 * HOUR, EventKind::PutToSleep),
            ev(6 * HOUR, EventKind::Slept { minutes: 60.0 }),
            ev(30 * HOUR, EventKind::Dismissed),
            ev(31 * HOUR, EventKind::Summoned),
        ];
        events.sort_by(|a, b| a.id.cmp(&b.id));
        let now = 50 * HOUR;
        let cfg = FoldCfg::default();
        let first = fold(&events, now, &cfg);
        let second = fold(&events, now, &cfg);
        assert_eq!(first, second);
    }

    #[test]
    fn fold_of_empty_journal_is_blank_pet() {
        let pet = fold(&[], 12_345, &FoldCfg::default());
        assert_eq!(pet.name, "Driftling");
        assert_eq!(pet.stage, Stage::Egg);
        assert_eq!(pet.stats, PetStats::default());
        assert_eq!(pet.born_ms, 12_345);
        assert_eq!(pet.care_mistakes, 0);
        assert!(pet.summoned && !pet.ill);
        assert_eq!(pet.color, DEFAULT_PET_COLOR);
    }

    // ---- fold: цвет тела ----

    /// Genesis цвета не несёт — действует дефолт; Recolored меняет цвет,
    /// последняя перекраска побеждает, кривая альфа нормализуется в ff.
    #[test]
    fn recolor_updates_color_and_normalizes_alpha() {
        let mut events = vec![genesis(0)];
        let cfg = FoldCfg::default();
        assert_eq!(fold(&events, MIN, &cfg).color, DEFAULT_PET_COLOR);

        events.push(ev(
            MIN,
            EventKind::Recolored {
                argb: 0x00_e8_94_4a,
            },
        ));
        assert_eq!(fold(&events, 2 * MIN, &cfg).color, 0xff_e8_94_4a);

        events.push(ev(
            2 * MIN,
            EventKind::Recolored {
                argb: 0xff_5f_bf_8f,
            },
        ));
        let pet = fold(&events, 3 * MIN, &cfg);
        assert_eq!(pet.color, 0xff_5f_bf_8f, "последняя перекраска побеждает");
        // Цвет — косметика: статы и рост перекраска не трогает.
        assert_eq!(pet.care_mistakes, 0);
    }

    #[test]
    fn fold_applies_rename_summon_and_attributes() {
        let attrs = PetAttributes {
            size: 200,
            ..PetAttributes::default()
        };
        let events = vec![
            genesis(0),
            ev(
                MIN,
                EventKind::Renamed {
                    name: "Дрифт".into(),
                },
            ),
            ev(2 * MIN, EventKind::AttributesSet { attributes: attrs }),
            ev(3 * MIN, EventKind::Dismissed),
        ];
        let pet = fold(&events, 4 * MIN, &FoldCfg::default());
        assert_eq!(pet.name, "Дрифт");
        assert_eq!(pet.attributes.size, 200);
        assert!(!pet.summoned);

        let mut events = events;
        events.push(ev(5 * MIN, EventKind::Summoned));
        let pet = fold(&events, 5 * MIN, &FoldCfg::default());
        assert!(pet.summoned);
    }

    // ---- fold: деградация и оффлайн-пол ----

    #[test]
    fn decay_rates_match_config_at_boundaries() {
        // Короткие разрывы (30 мин) — честная деградация без пола.
        let mut events = vec![genesis(0)];
        events.extend(fillers(0, 481));
        let pet = fold(&events, 481 * MIN, &FoldCfg::default());
        // Сытость: 100 -> 0 ровно за 8 ч (481-я минута добивает остаток).
        assert_eq!(pet.stats.satiety, 0.0);
        assert_eq!(pet.care_mistakes, 1, "ноль сытости = одна ошибка ухода");
        // Энергия за 10 ч, настроение за 12 ч — в точке 481 мин они ещё живы.
        assert!(approx(pet.stats.energy, 100.0 - 481.0 / 6.0, 0.05));
        assert!(approx(pet.stats.mood, 100.0 - 481.0 * 100.0 / 720.0, 0.05));
        assert!(!pet.ill);
    }

    #[test]
    fn offline_gap_floors_stats_at_20() {
        let events = vec![genesis(0)];
        let pet = fold(&events, 48 * HOUR, &FoldCfg::default());
        // «Голодный, но живой»: двое суток забвения — все статы ровно на полу.
        assert_eq!(pet.stats.satiety, 20.0);
        assert_eq!(pet.stats.energy, 20.0);
        assert_eq!(pet.stats.mood, 20.0);
        assert_eq!(pet.stats.health, 100.0, "нулей не было — здоровье цело");
        assert_eq!(pet.care_mistakes, 0);
        assert!(!pet.ill);
    }

    #[test]
    fn offline_gap_does_not_touch_already_low_stat() {
        // 7 часов короткими разрывами: сытость проседает до ~12.5...
        let mut events = vec![genesis(0)];
        events.extend(fillers(0, 420));
        let before = fold(&events, 420 * MIN, &FoldCfg::default());
        assert!(approx(before.stats.satiety, 12.5, 0.05));
        // ...и сутки оффлайна её не трогают (пол = min(стат, 20)).
        let pet = fold(&events, 420 * MIN + 24 * HOUR, &FoldCfg::default());
        assert!(approx(pet.stats.satiety, before.stats.satiety, 0.001));
        assert_eq!(pet.stats.energy, 20.0);
        assert_eq!(pet.stats.mood, 20.0);
        assert_eq!(pet.care_mistakes, 0, "оффлайн не роняет статы в ноль");
    }

    #[test]
    fn short_gaps_do_drop_to_zero_and_count_once_per_episode() {
        // Короткие разрывы до 10.5 ч: сытость в нуле с ~8 ч, энергия с ~10 ч.
        let mut events = vec![genesis(0)];
        events.extend(fillers(0, 630));
        let pet = fold(&events, 630 * MIN, &FoldCfg::default());
        assert_eq!(pet.stats.satiety, 0.0);
        assert_eq!(pet.stats.energy, 0.0);
        assert!(approx(pet.stats.mood, 12.5, 0.05));
        assert_eq!(
            pet.care_mistakes, 2,
            "по одной ошибке на эпизод: сытость и энергия"
        );
        // Здоровье просело за ~2.5 ч нулевой сытости.
        assert!(approx(pet.stats.health, 85.0, 0.3));
    }

    #[test]
    fn zero_episode_recounts_after_recovery() {
        let mut events = vec![genesis(0)];
        events.extend(fillers(0, 509)); // сытость в нуле с ~480-й минуты
        events.push(ev(510 * MIN, EventKind::Fed { treat: false }));
        events.extend(fillers(510, 750)); // еда усвоилась и снова сгорела
        let pet = fold(&events, 750 * MIN, &FoldCfg::default());
        // Ошибки: сытость (480), энергия (600), сытость снова (~678),
        // настроение (720) — повторное падение = новый эпизод.
        assert_eq!(pet.care_mistakes, 4);
        assert_eq!(pet.stats.satiety, 0.0);
    }

    // ---- fold: кормление ----

    #[test]
    fn feeding_gives_instant_half_then_trickles() {
        let mut events = vec![genesis(0)];
        events.extend(fillers(0, 239)); // сытость к 240-й минуте ~50
        events.push(ev(240 * MIN, EventKind::Fed { treat: false }));
        let cfg = FoldCfg::default();

        let at_feed = fold(&events, 240 * MIN, &cfg);
        assert!(
            approx(at_feed.stats.satiety, 67.5, 0.1),
            "мгновенная половина: 50 + 17.5, а не все 35: {}",
            at_feed.stats.satiety
        );
        // Через 35 мин пул (17.5 по 0.5/мин) досыпан, минус деградация.
        let trickled = fold(&events, 275 * MIN, &cfg);
        assert!(approx(
            trickled.stats.satiety,
            67.5 + 17.5 - 35.0 * 100.0 / 480.0,
            0.1
        ));
        // Через час после еды: весь эффект 35 минус деградация часа.
        let later = fold(&events, 300 * MIN, &cfg);
        assert!(approx(
            later.stats.satiety,
            50.0 + 35.0 - 60.0 * 100.0 / 480.0,
            0.1
        ));
    }

    #[test]
    fn treats_hit_health_only_beyond_daily_limit() {
        let mut events = vec![genesis(0)];
        for m in 1..=4u64 {
            events.push(ev(m * MIN, EventKind::Fed { treat: true }));
        }
        let cfg = FoldCfg::default();
        let pet = fold(&events, 4 * MIN, &cfg);
        assert_eq!(
            pet.stats.health, 97.0,
            "первые три без последствий, четвёртая -3"
        );
        events.push(ev(5 * MIN, EventKind::Fed { treat: true }));
        let pet = fold(&events, 5 * MIN, &cfg);
        assert!(
            approx(pet.stats.health, 94.05, 0.05),
            "пятая ещё -3 (плюс минутная регенерация): {}",
            pet.stats.health
        );
    }

    #[test]
    fn treat_window_is_trailing_24h() {
        let mut events = vec![genesis(0)];
        for m in 1..=3u64 {
            events.push(ev(m * MIN, EventKind::Fed { treat: true }));
        }
        // Четвёртая — через 26 часов: суточное окно уже пусто.
        events.push(ev(26 * HOUR, EventKind::Fed { treat: true }));
        let pet = fold(&events, 26 * HOUR, &FoldCfg::default());
        assert_eq!(pet.stats.health, 100.0, "лимит скользящий, штрафа нет");
        // Оффлайн-пол в том же прогоне: статы упёрлись в 20 и подросли едой.
        assert_eq!(pet.stats.satiety, 30.0);
        assert_eq!(pet.stats.mood, 40.0);
    }

    // ---- fold: игра, поглаживание, сон ----

    #[test]
    fn play_boosts_mood_and_costs_energy() {
        let mut events = vec![genesis(0)];
        events.extend(fillers(0, 539)); // энергия к 540-й минуте ~10
        events.push(ev(540 * MIN, EventKind::Played));
        let pet = fold(&events, 540 * MIN, &FoldCfg::default());
        assert_eq!(pet.stats.energy, 0.0, "пол нуля: 10 - 15 -> 0");
        assert!(approx(pet.stats.mood, 100.0 - 75.0 + 25.0, 0.1));
        assert_eq!(
            pet.care_mistakes, 2,
            "сытость в нуле с 8 ч + заигранная в ноль энергия"
        );
    }

    #[test]
    fn petting_mood_is_capped_per_hour() {
        let mut events = vec![genesis(0), ev(360 * MIN, EventKind::Played)];
        for m in 361..=364u64 {
            events.push(ev(m * MIN, EventKind::Petted));
        }
        let cfg = FoldCfg::default();
        let pet = fold(&events, 364 * MIN, &cfg);
        // 4 поглаживания по +4, но потолок 12/час: 100 - decay + 25 + 12.
        let expected = 100.0 - 364.0 * 100.0 / 720.0 + 25.0 + 12.0;
        assert!(
            approx(pet.stats.mood, expected, 0.15),
            "потолок поглаживаний: {} vs {expected}",
            pet.stats.mood
        );
        // Окно скользящее: спустя час лимит снова свободен.
        events.push(ev(425 * MIN, EventKind::Petted));
        let pet = fold(&events, 425 * MIN, &cfg);
        let expected = 100.0 - 425.0 * 100.0 / 720.0 + 25.0 + 12.0 + 4.0;
        assert!(approx(pet.stats.mood, expected, 0.2));
    }

    #[test]
    fn sleeping_restores_energy_with_cap() {
        let mut events = vec![genesis(0), ev(540 * MIN, EventKind::PutToSleep)];
        events.push(ev_on(
            "test",
            540 * MIN,
            1,
            EventKind::Slept { minutes: 50.0 },
        ));
        let cfg = FoldCfg::default();
        let pet = fold(&events, 540 * MIN, &cfg);
        // Один большой разрыв упёр энергию в пол 20, сон вернул 50*0.8.
        assert_eq!(pet.stats.energy, 60.0);
        // Пересып не выше 100; кривые минуты из журнала не ломают fold.
        events.push(ev(541 * MIN, EventKind::Slept { minutes: 100_000.0 }));
        events.push(ev(542 * MIN, EventKind::Slept { minutes: -5.0 }));
        let pet = fold(&events, 542 * MIN, &cfg);
        assert!(approx(pet.stats.energy, 100.0 - 100.0 / 600.0, 0.01));
    }

    // ---- fold: здоровье и болезнь ----

    #[test]
    fn zeroed_stats_drain_health_into_illness() {
        let mut events = vec![genesis(0)];
        events.extend(fillers(0, 1200)); // 20 часов заброшенности
        let pet = fold(&events, 1200 * MIN, &FoldCfg::default());
        // Сытость в нуле с 8 ч: 12 ч * -6/ч = -72 здоровья.
        assert!(approx(pet.stats.health, 28.0, 0.3));
        assert!(pet.ill, "здоровье ниже 40 = болезнь (смерти нет)");
        assert_eq!(pet.care_mistakes, 3, "сытость, энергия, настроение");
    }

    #[test]
    fn good_care_regenerates_health() {
        let mut events = vec![genesis(0)];
        for m in 1..=5u64 {
            events.push(ev(m * MIN, EventKind::Fed { treat: true }));
        }
        let cfg = FoldCfg::default();
        let hurt = fold(&events, 5 * MIN, &cfg);
        assert!(hurt.stats.health < 95.0);
        // Час с сытостью и настроением выше 60 — +3 здоровья.
        let healed = fold(&events, 65 * MIN, &cfg);
        assert!(
            approx(healed.stats.health, hurt.stats.health + 3.0, 0.1),
            "{} -> {}",
            hurt.stats.health,
            healed.stats.health
        );
    }

    // ---- fold: рост ----

    #[test]
    fn growth_follows_real_time_gates() {
        let events = vec![genesis(0)];
        let cfg = FoldCfg::default();
        assert_eq!(fold(&events, 2 * MIN, &cfg).stage, Stage::Egg);
        assert_eq!(fold(&events, 5 * MIN, &cfg).stage, Stage::Baby);
        assert_eq!(fold(&events, 25 * HOUR, &cfg).stage, Stage::Child);
        assert_eq!(fold(&events, 73 * HOUR, &cfg).stage, Stage::Teen);
        assert_eq!(fold(&events, 7 * 24 * HOUR, &cfg).stage, Stage::Adult);
    }

    #[test]
    fn growth_scale_accelerates_gates_for_debug() {
        let events = vec![genesis(0)];
        let cfg = FoldCfg {
            growth_scale: 1440.0, // минута реального времени = сутки
            ..FoldCfg::default()
        };
        assert_eq!(fold(&events, MIN, &cfg).stage, Stage::Child);
        assert_eq!(fold(&events, 6 * MIN, &cfg).stage, Stage::Adult);
        let real = FoldCfg::default();
        assert_eq!(fold(&events, 6 * MIN, &real).stage, Stage::Baby);
    }

    #[test]
    fn stage_never_drops_below_born_stage() {
        let events = vec![ev(
            0,
            EventKind::Genesis {
                name: "Старожил".into(),
                attributes: PetAttributes::default(),
                born_stage: Stage::Adult,
            },
        )];
        let pet = fold(&events, 1, &FoldCfg::default());
        assert_eq!(pet.stage, Stage::Adult, "мигрант не вылупляется заново");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod fs_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// «Своё» устройство тестов.
    const DEV: &str = "disk";

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("driftling-journal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_on(device: &str, t: u64, kind: EventKind) -> Event {
        Event {
            id: Hlc {
                wall_ms: t,
                counter: 0,
                device: device.into(),
            },
            kind,
        }
    }

    fn sample(t: u64, kind: EventKind) -> Event {
        sample_on(DEV, t, kind)
    }

    /// Записать файл журнала «как есть»: старый формат без чексумм
    /// (легаси-файл или файл чужого устройства, принесённый синкером).
    fn write_raw(path: &Path, events: &[Event]) {
        let mut text = String::new();
        for ev in events {
            text.push_str(&serde_json::to_string(ev).unwrap());
            text.push('\n');
        }
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn crc32_matches_known_vector() {
        // Классический проверочный вектор CRC-32/ISO-HDLC.
        assert_eq!(fs::crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn append_then_open_roundtrips_sorted() {
        let dir = tmp_dir("roundtrip");
        assert_eq!(Journal::open(&dir).unwrap(), (vec![], 0), "файлов ещё нет");
        let events = [
            sample(2, EventKind::Petted),
            sample(1, EventKind::Fed { treat: false }),
            sample(3, EventKind::Slept { minutes: 5.5 }),
        ];
        for ev in &events {
            Journal::append(&dir, DEV, ev).unwrap();
        }
        // Запись ушла в файл своего устройства, легаси-файл не создан.
        assert!(device_journal_path_in(&dir, DEV).is_file());
        assert!(!journal_path_in(&dir).exists());
        let (back, warnings) = Journal::open(&dir).unwrap();
        assert_eq!(warnings, 0);
        let ids: Vec<u64> = back.iter().map(|e| e.id.wall_ms).collect();
        assert_eq!(ids, vec![1, 2, 3], "open сортирует по id");
        assert_eq!(
            Journal::open_dir(&dir, DEV).unwrap(),
            (back, 0),
            "open_dir без легаси-файла эквивалентен open"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_writes_crc_suffix_on_every_line() {
        let dir = tmp_dir("crc-write");
        Journal::append(&dir, DEV, &sample(1, EventKind::Petted)).unwrap();
        Journal::append(&dir, DEV, &sample(2, EventKind::Played)).unwrap();
        let text = std::fs::read_to_string(device_journal_path_in(&dir, DEV)).unwrap();
        for line in text.lines() {
            let (_, tail) = line.rsplit_once("\t#").expect("суффикс на месте");
            assert_eq!(tail.len(), 8, "hex8: {line}");
            assert!(tail.chars().all(|c| c.is_ascii_hexdigit()));
        }
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!((events.len(), warnings), (2, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ключевой сценарий чексуммы: порча байта в середине файла, при
    /// которой JSON остаётся валидным, — парсер бы проглотил, CRC ловит.
    #[test]
    fn crc_detects_corruption_mid_file() {
        let dir = tmp_dir("crc-corrupt");
        for t in 1..=3u64 {
            Journal::append(&dir, DEV, &sample(t, EventKind::Petted)).unwrap();
        }
        let path = device_journal_path_in(&dir, DEV);
        let text = std::fs::read_to_string(&path).unwrap();
        let mangled = text.replace("\"wall_ms\":2", "\"wall_ms\":8");
        assert_ne!(mangled, text, "порча попала в среднюю строку");
        std::fs::write(&path, mangled).unwrap();

        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!(warnings, 1, "битая строка посчитана");
        let ids: Vec<u64> = events.iter().map(|e| e.id.wall_ms).collect();
        assert_eq!(ids, vec![1, 3], "строка пропущена, соседи целы");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_crc_suffix_is_a_warning() {
        let dir = tmp_dir("crc-broken");
        Journal::append(&dir, DEV, &sample(1, EventKind::Petted)).unwrap();
        let path = device_journal_path_in(&dir, DEV);
        // Суффикс из 9 hex-символов — кривой формат, строка бита.
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\t#", "\t#f");
        std::fs::write(&path, text).unwrap();
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!((events.len(), warnings), (0, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_lines_without_crc_stay_valid() {
        let dir = tmp_dir("no-crc");
        // Старый формат: строки без чексуммы читаются без предупреждений...
        write_raw(
            &device_journal_path_in(&dir, DEV),
            &[sample(1, EventKind::Petted), sample(2, EventKind::Played)],
        );
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!((events.len(), warnings), (2, 0));
        // ...и смешанный файл (дозапись нового формата) тоже.
        Journal::append(&dir, DEV, &sample(3, EventKind::Summoned)).unwrap();
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!((events.len(), warnings), (3, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_journal_migrates_by_rename() {
        let dir = tmp_dir("legacy-rename");
        write_raw(
            &journal_path_in(&dir),
            &[sample(1, EventKind::Petted), sample(2, EventKind::Played)],
        );
        let (events, warnings) = Journal::open_dir(&dir, DEV).unwrap();
        assert_eq!((events.len(), warnings), (2, 0));
        assert!(!journal_path_in(&dir).exists(), "легаси-файл переименован");
        assert!(device_journal_path_in(&dir, DEV).is_file());
        // Повторное открытие идемпотентно, append продолжает тот же файл.
        Journal::append(&dir, DEV, &sample(3, EventKind::Summoned)).unwrap();
        let (events, warnings) = Journal::open_dir(&dir, DEV).unwrap();
        assert_eq!((events.len(), warnings), (3, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Каталог, склеенный из бэкапов: и легаси-файл, и файл устройства.
    /// Миграция дописывает легаси-строки в свой файл и убирает легаси;
    /// дубль id схлопывается чтением.
    #[test]
    fn legacy_merges_into_existing_own_file() {
        let dir = tmp_dir("legacy-merge");
        Journal::append(&dir, DEV, &sample(1, EventKind::Petted)).unwrap();
        write_raw(
            &journal_path_in(&dir),
            &[sample(1, EventKind::Petted), sample(2, EventKind::Played)],
        );
        let (events, warnings) = Journal::open_dir(&dir, DEV).unwrap();
        assert_eq!((events.len(), warnings), (2, 0), "дубль схлопнут");
        assert!(!journal_path_in(&dir).exists(), "легаси-файл убран");
        let (again, warnings) = Journal::open_dir(&dir, DEV).unwrap();
        assert_eq!((again, warnings), (events, 0), "повтор стабилен");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Файл чужого устройства (принесён Syncthing/Nextcloud): читается и
    /// сливается, но не переписывается — read-only вход (ТЗ §3.5).
    #[test]
    fn foreign_device_files_merge_read_only() {
        let dir = tmp_dir("foreign");
        Journal::append(&dir, DEV, &sample(5, EventKind::Played)).unwrap();
        let foreign = device_journal_path_in(&dir, "phone");
        write_raw(
            &foreign,
            &[
                sample_on("phone", 3, EventKind::Fed { treat: false }),
                sample_on("phone", 7, EventKind::Petted),
            ],
        );
        let before = std::fs::read(&foreign).unwrap();
        let (events, warnings) = Journal::open_dir(&dir, DEV).unwrap();
        assert_eq!(warnings, 0);
        let ids: Vec<u64> = events.iter().map(|e| e.id.wall_ms).collect();
        assert_eq!(ids, vec![3, 5, 7], "слияние сортирует по id");
        assert_eq!(
            std::fs::read(&foreign).unwrap(),
            before,
            "чужой файл не тронут"
        );
        // Детерминизм слияния: повторное чтение даёт бит-в-бит то же.
        assert_eq!(Journal::open_dir(&dir, DEV).unwrap().0, events);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_refuses_foreign_device_event() {
        let dir = tmp_dir("refuse");
        let err = Journal::append(&dir, DEV, &sample_on("phone", 1, EventKind::Petted))
            .expect_err("чужое событие в свой файл — нарушение single-writer");
        assert!(err.contains("phone"), "ошибка называет виновника: {err}");
        assert!(!device_journal_path_in(&dir, "phone").exists());
        assert!(!device_journal_path_in(&dir, DEV).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_is_read_only_and_does_not_migrate() {
        let dir = tmp_dir("open-ro");
        write_raw(&journal_path_in(&dir), &[sample(1, EventKind::Petted)]);
        write_raw(
            &device_journal_path_in(&dir, "phone"),
            &[sample_on("phone", 2, EventKind::Played)],
        );
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!((events.len(), warnings), (2, 0), "видит все файлы");
        assert!(
            journal_path_in(&dir).exists(),
            "open ничего не переименовывает"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn torn_tail_is_skipped_with_warning_not_error() {
        let dir = tmp_dir("torn");
        Journal::append(&dir, DEV, &sample(1, EventKind::Petted)).unwrap();
        Journal::append(&dir, DEV, &sample(2, EventKind::Played)).unwrap();
        // Краш посреди записи: оборванный хвост без перевода строки.
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(device_journal_path_in(&dir, DEV))
            .unwrap();
        file.write_all(br#"{"id":{"wall_ms":3,"cou"#).unwrap();
        drop(file);

        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!(events.len(), 2, "целые события не теряются");
        assert_eq!(warnings, 1, "битая строка посчитана");

        // Следующий append не приклеивается к оборванному хвосту.
        Journal::append(&dir, DEV, &sample(4, EventKind::Summoned)).unwrap();
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(warnings, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_lines_are_not_warnings() {
        let dir = tmp_dir("blank");
        let line = serde_json::to_string(&sample(1, EventKind::Petted)).unwrap();
        std::fs::write(device_journal_path_in(&dir, DEV), format!("\n{line}\n\n")).unwrap();
        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!((events.len(), warnings), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_ids_on_disk_collapse_on_open() {
        let dir = tmp_dir("dup");
        let ev = sample(1, EventKind::Petted);
        Journal::append(&dir, DEV, &ev).unwrap();
        Journal::append(&dir, DEV, &ev).unwrap();
        let (events, _) = Journal::open(&dir).unwrap();
        assert_eq!(events.len(), 1, "журнал — множество событий по id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// append_remote (транспорт синка, фаза E): привезённые pull-ом события
    /// раскладываются по файлам своих устройств-авторов, читаются обратно
    /// в общее слияние и защищены теми же чексуммами.
    #[test]
    fn append_remote_materializes_per_author_files() {
        let dir = tmp_dir("remote");
        Journal::append(&dir, DEV, &sample(5, EventKind::Petted)).unwrap();
        let remote = vec![
            sample_on("laptop", 1, EventKind::Played),
            sample_on("desktop", 2, EventKind::Petted),
            sample_on("laptop", 3, EventKind::Petted),
        ];
        Journal::append_remote(&dir, &remote).unwrap();

        assert!(device_journal_path_in(&dir, "laptop").is_file());
        assert!(device_journal_path_in(&dir, "desktop").is_file());
        // Строки чужих файлов — с CRC-суффиксом, как у своих.
        let text = std::fs::read_to_string(device_journal_path_in(&dir, "laptop")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|l| l.contains("\t#")), "{text}");

        let (events, warnings) = Journal::open(&dir).unwrap();
        assert_eq!(warnings, 0);
        let ids: Vec<(u64, String)> = events
            .iter()
            .map(|e| (e.id.wall_ms, e.id.device.clone()))
            .collect();
        assert_eq!(
            ids,
            vec![
                (1, "laptop".into()),
                (2, "desktop".into()),
                (3, "laptop".into()),
                (5, DEV.into())
            ]
        );
        // Пустой батч — no-op без ошибок и новых файлов.
        Journal::append_remote(&dir, &[]).unwrap();
        assert!(Journal::append_remote(&dir, &[sample_on("", 9, EventKind::Petted)]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
