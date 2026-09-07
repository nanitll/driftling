//! Спрайт-пак: настоящий пиксель-арт дрифтлинга вместо процедурного блоба
//! (фаза C). Кадры хранятся как текстовые сетки + манифест `pack.toml`;
//! этот же формат позже станет содержимым внешних `.driftpack`-архивов —
//! документация ниже нормативная.
//!
//! # Формат пака (версия 1)
//!
//! Пак — каталог вида:
//!
//! ```text
//! pack.toml                 манифест (см. ниже)
//! <stage>/<anim>_<n>.txt    кадры: egg/, baby/, child/, teen/, adult/
//! ```
//!
//! ## Кадр-сетка
//!
//! Текстовый файл: одна строка — один ряд пикселей, один символ — один
//! пиксель. Все строки одной длины; размер сетки обязан совпадать с
//! `native` своей стадии (квадрат). Пустые строки в конце игнорируются.
//! Символы:
//!
//! | Символ | Роль | Цвет при колоризации |
//! |---|---|---|
//! | `.` | прозрачность | `0x00000000` |
//! | `B` | тело | базовый цвет питомца (настройка пользователя) |
//! | `D` | тень тела | `darken(body, 0.78)` |
//! | `O` | контур | `darken(body, 0.45)` |
//! | `E` | глаз | фиксированный тёмный `0xff1e1e2e` |
//! | `S` | блик глаза | чистый белый |
//! | `C` | щека | розовый; на тёмном теле — осветлённое тело (гард контраста) |
//! | `W` | «тёплый белый» | белый с подмесом тела 18% (брюшко, скорлупа) |
//! | `A` | акцент | белый с подмесом тела 55% (крапинки, тень скорлупы, еда) |
//! | `X` | фиксированный тёмный | `0xff3a3244` (рот изнутри, «Zz») |
//!
//! Все цвета непрозрачные, альфа 0 или 255 — поэтому кадры одновременно
//! корректны и как straight-, и как premultiplied-ARGB (см. blit в text.rs).
//!
//! ## Манифест `pack.toml`
//!
//! ```toml
//! format = 1                # версия формата пака
//!
//! [stages.adult]
//! native = 32               # сторона сетки кадров стадии, px
//!
//! [stages.adult.anims.idle]
//! fps = 2.0                 # темп проигрывания семейства
//! frames = ["idle_0", "idle_1"]   # файлы <имя>.txt в каталоге стадии
//! ```
//!
//! Обязательные стадии — все пять. Семейства: у `egg` — `egg` и `hatch`;
//! у остальных стадий — `idle`, `walk`, `sleep`, `falling`, `dragged`,
//! `landing`, `eating`, `happy`, `sad`, `sick`.
//!
//! Необязательные семейства фазы G ([`EXTRA_ANIMS`]): `blink`, `sit`,
//! `stretch`, `wiggle`, `cling`, `climb`, `dizzy`. Если их в паке нет,
//! движок берёт откат (`climb` -> `walk`, `cling`/`blink`/`sit`/... ->
//! `idle`, `dizzy` -> `landing`), так что паки формата 1 без них валидны.
//!
//! `climb` и `cling` рисуются ИНАЧЕ остальных: это вид СО СПИНЫ — питомец
//! прижался к поверхности и держится лапками. Их же кадры показываются под
//! потолком, отражёнными по вертикали. Поворачивать обычную походку боком
//! нельзя: со стороны это читается как «лежит в воздухе».
//!
//! Кадры рисуются мордой ВПРАВО и ногами ВНИЗ; зеркало по взгляду и
//! поворот под поверхность (стена, потолок) делает рендер
//! (`driftling_core::Orient`) — отдельного арта под лазание не нужно.
//!
//! ## Загрузка
//!
//! Разбор ([`Pack::parse`]) валидирует манифест и все сетки; ошибки —
//! конкретные ([`PackError`]). Дальше [`Pack::sprite_set`] собирает
//! [`SpriteSet`]: сетка → колоризация базовым цветом → целочисленный
//! nearest-апскейл до целевого размера. Встроенный пак по умолчанию —
//! [`default_pack`] / [`default_sprite_set`].
//!
//! fps из манифеста доступен через [`Pack::fps`]; тайминг анимаций пока
//! живёт в sprite.rs (интеграция — отдельная фаза), но внешние паки уже
//! обязаны заполнять поле.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::growth::Stage;
use crate::palette;
use crate::sprite::{Frame, SpriteSet};

/// Единственная поддерживаемая версия формата пака.
pub const FORMAT_VERSION: u32 = 1;

/// Символы, допустимые в сетке кадра.
pub const GRID_CHARS: &str = ".BDOESCWAX";

// Фиксированные цвета колоризации. Тон глаза и розовый щёк намеренно
// совпадают с процедурным спрайтом (sprite.rs, там они приватные):
// оба пака должны читаться одинаково, пока блоб жив как фолбэк.
const EYE_INK: u32 = 0xff_1e_1e_2e;
const SHINE: u32 = 0xff_ff_ff_ff;
const CHEEK_PINK: u32 = 0xff_e8_9a_c7;
const FIXED_DARK: u32 = 0xff_3a_32_44;
/// Гард контраста щёк — тот же порог, что в sprite.rs.
const CHEEK_DARK_LUMA: f32 = 0.35;

/// Ошибки разбора пака. Формулировки конкретные: имя кадра/стадии всегда
/// в сообщении — это то, что увидит лог демона при битом .driftpack.
#[derive(Debug, Clone, PartialEq)]
pub enum PackError {
    /// Манифест не разобрался как TOML нужной схемы.
    Manifest(String),
    /// Версия формата не поддерживается.
    Format(u32),
    /// В манифесте нет обязательной стадии.
    MissingStage(&'static str),
    /// Неизвестное имя стадии в манифесте.
    UnknownStage(String),
    /// У стадии нет обязательного семейства анимаций.
    MissingAnim {
        stage: &'static str,
        anim: &'static str,
    },
    /// Семейство объявлено, но список кадров пуст.
    EmptyAnim { stage: String, anim: String },
    /// fps семейства не положительный или не конечный.
    BadFps { stage: String, anim: String },
    /// Кадр из манифеста не найден среди файлов пака.
    MissingFrame { key: String },
    /// Сетка кадра пуста.
    EmptyGrid { key: String },
    /// Строки сетки разной длины.
    RaggedGrid { key: String, row: usize },
    /// Недопустимый символ в сетке.
    BadChar {
        key: String,
        row: usize,
        col: usize,
        ch: char,
    },
    /// Размер сетки не совпадает с `native` стадии.
    BadSize {
        key: String,
        expected: u32,
        w: u32,
        h: u32,
    },
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::Manifest(e) => write!(f, "манифест пака не разобрался: {e}"),
            PackError::Format(v) => {
                write!(
                    f,
                    "формат пака {v} не поддерживается (ожидался {FORMAT_VERSION})"
                )
            }
            PackError::MissingStage(s) => write!(f, "в паке нет стадии «{s}»"),
            PackError::UnknownStage(s) => write!(f, "неизвестная стадия «{s}» в манифесте"),
            PackError::MissingAnim { stage, anim } => {
                write!(f, "у стадии «{stage}» нет семейства «{anim}»")
            }
            PackError::EmptyAnim { stage, anim } => {
                write!(f, "семейство «{stage}/{anim}» без кадров")
            }
            PackError::BadFps { stage, anim } => {
                write!(f, "у семейства «{stage}/{anim}» некорректный fps")
            }
            PackError::MissingFrame { key } => write!(f, "кадр «{key}» не найден в паке"),
            PackError::EmptyGrid { key } => write!(f, "кадр «{key}»: пустая сетка"),
            PackError::RaggedGrid { key, row } => {
                write!(f, "кадр «{key}»: строка {row} другой длины")
            }
            PackError::BadChar { key, row, col, ch } => {
                write!(
                    f,
                    "кадр «{key}»: недопустимый символ «{ch}» в ({row},{col})"
                )
            }
            PackError::BadSize {
                key,
                expected,
                w,
                h,
            } => {
                write!(
                    f,
                    "кадр «{key}»: сетка {w}x{h}, а native стадии — {expected}"
                )
            }
        }
    }
}

/// Разобранная сетка кадра: символы без колоризации.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub w: u32,
    pub h: u32,
    cells: Vec<u8>,
}

impl Grid {
    /// Символ пикселя (для тестов и инструментов).
    pub fn at(&self, x: u32, y: u32) -> char {
        self.cells[(y * self.w + x) as usize] as char
    }
}

/// Разобрать текстовую сетку кадра. `key` («стадия/имя») попадает в ошибки.
pub fn parse_grid(key: &str, text: &str) -> Result<Grid, PackError> {
    let rows: Vec<&str> = {
        let mut rows: Vec<&str> = text.lines().collect();
        while rows.last().is_some_and(|r| r.trim().is_empty()) {
            rows.pop();
        }
        rows
    };
    if rows.is_empty() {
        return Err(PackError::EmptyGrid {
            key: key.to_string(),
        });
    }
    let w = rows[0].chars().count();
    if w == 0 {
        return Err(PackError::EmptyGrid {
            key: key.to_string(),
        });
    }
    let mut cells = Vec::with_capacity(w * rows.len());
    for (row, line) in rows.iter().enumerate() {
        if line.chars().count() != w {
            return Err(PackError::RaggedGrid {
                key: key.to_string(),
                row,
            });
        }
        for (col, ch) in line.chars().enumerate() {
            if !GRID_CHARS.contains(ch) {
                return Err(PackError::BadChar {
                    key: key.to_string(),
                    row,
                    col,
                    ch,
                });
            }
            cells.push(ch as u8);
        }
    }
    Ok(Grid {
        w: w as u32,
        h: rows.len() as u32,
        cells,
    })
}

/// Смешение непрозрачных ARGB (`k` — доля `b`); альфа результата 0xff.
fn mix(a: u32, b: u32, k: f32) -> u32 {
    let ch = |sh: u32| {
        let ca = ((a >> sh) & 0xff) as f32;
        let cb = ((b >> sh) & 0xff) as f32;
        ((ca + (cb - ca) * k) as u32) & 0xff
    };
    0xff00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Палитра колоризации, выведенная из базового цвета тела.
#[derive(Debug, Clone, Copy)]
struct Colorway {
    body: u32,
    body_dark: u32,
    outline: u32,
    cheek: u32,
    warm_white: u32,
    accent: u32,
}

impl Colorway {
    fn from_argb(argb: u32) -> Self {
        let body = 0xff00_0000 | (argb & 0x00ff_ffff);
        let cheek = if palette::luminance(body) < CHEEK_DARK_LUMA {
            palette::lighten(body, 1.3)
        } else {
            CHEEK_PINK
        };
        Self {
            body,
            body_dark: palette::darken(body, 0.78),
            outline: palette::darken(body, 0.45),
            cheek,
            warm_white: mix(SHINE, body, 0.18),
            accent: mix(SHINE, body, 0.55),
        }
    }

    fn color(&self, ch: u8) -> u32 {
        match ch {
            b'.' => 0,
            b'B' => self.body,
            b'D' => self.body_dark,
            b'O' => self.outline,
            b'E' => EYE_INK,
            b'S' => SHINE,
            b'C' => self.cheek,
            b'W' => self.warm_white,
            b'A' => self.accent,
            b'X' => FIXED_DARK,
            // parse_grid не пропускает другие символы.
            _ => unreachable!("недопустимый символ сетки"),
        }
    }
}

/// Колоризация сетки базовым цветом тела -> кадр 1:1.
pub fn colorize(grid: &Grid, argb: u32) -> Frame {
    let cw = Colorway::from_argb(argb);
    Frame {
        w: grid.w,
        h: grid.h,
        argb: grid.cells.iter().map(|&ch| cw.color(ch)).collect(),
    }
}

/// Целочисленный nearest-апскейл: каждый пиксель — блок `k`x`k`.
pub fn scale_nearest(frame: &Frame, k: u32) -> Frame {
    let k = k.max(1);
    if k == 1 {
        return frame.clone();
    }
    let (w, h) = (frame.w * k, frame.h * k);
    let mut argb = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let src = ((y / k) * frame.w) as usize;
        for x in 0..w {
            argb.push(frame.argb[src + (x / k) as usize]);
        }
    }
    Frame { w, h, argb }
}

/// Семейство: fps из манифеста + разобранные сетки кадров.
#[derive(Debug, Clone, PartialEq)]
struct Anim {
    fps: f32,
    frames: Vec<Grid>,
}

/// Кадры и метаданные одной стадии.
#[derive(Debug, Clone, PartialEq)]
struct StagePack {
    native: u32,
    anims: BTreeMap<String, Anim>,
}

/// Разобранный и провалидированный пак.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    stages: BTreeMap<&'static str, StagePack>,
}

// ---- Схема манифеста (serde) ----

#[derive(Deserialize)]
struct ManifestToml {
    format: u32,
    stages: BTreeMap<String, StageToml>,
}

#[derive(Deserialize)]
struct StageToml {
    native: u32,
    anims: BTreeMap<String, AnimToml>,
}

#[derive(Deserialize)]
struct AnimToml {
    fps: f32,
    frames: Vec<String>,
}

/// Машинное имя стадии в манифесте/путях пака.
fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::Egg => "egg",
        Stage::Baby => "baby",
        Stage::Child => "child",
        Stage::Teen => "teen",
        Stage::Adult => "adult",
    }
}

const ALL_STAGES: [Stage; 5] = [
    Stage::Egg,
    Stage::Baby,
    Stage::Child,
    Stage::Teen,
    Stage::Adult,
];

/// Обязательные семейства не-яйцевых стадий (и порядок листов рендера).
pub const BODY_ANIMS: [&str; 10] = [
    "idle", "walk", "sleep", "falling", "dragged", "landing", "eating", "happy", "sad", "sick",
];
/// Необязательные семейства фазы G: если пак их не содержит, движок
/// откатывается к базовым (climb -> walk, cling/blink/sit/... -> idle,
/// dizzy -> landing). Внешние паки формата 1 остаются валидными.
pub const EXTRA_ANIMS: [&str; 10] = [
    "blink", "sit", "stretch", "wiggle", "cling", "climb", "dizzy", "hang", "swing", "vomit",
];
/// Обязательные семейства стадии яйца.
pub const EGG_ANIMS: [&str; 2] = ["egg", "hatch"];

impl Pack {
    /// Разобрать пак из манифеста и набора кадров `("стадия/имя", текст)`.
    /// Валидирует всё сразу: стадии, семейства, fps, размеры сеток.
    pub fn parse(manifest: &str, sources: &[(&str, &str)]) -> Result<Pack, PackError> {
        let m: ManifestToml =
            toml::from_str(manifest).map_err(|e| PackError::Manifest(e.to_string()))?;
        if m.format != FORMAT_VERSION {
            return Err(PackError::Format(m.format));
        }
        for name in m.stages.keys() {
            if !ALL_STAGES.iter().any(|s| stage_key(*s) == name) {
                return Err(PackError::UnknownStage(name.clone()));
            }
        }
        let mut stages = BTreeMap::new();
        for stage in ALL_STAGES {
            let key = stage_key(stage);
            let st = m.stages.get(key).ok_or(PackError::MissingStage(key))?;
            let required: &[&str] = if stage == Stage::Egg {
                &EGG_ANIMS
            } else {
                &BODY_ANIMS
            };
            for anim in required {
                if !st.anims.contains_key(*anim) {
                    return Err(PackError::MissingAnim { stage: key, anim });
                }
            }
            let mut anims = BTreeMap::new();
            for (anim_name, spec) in &st.anims {
                if spec.frames.is_empty() {
                    return Err(PackError::EmptyAnim {
                        stage: key.to_string(),
                        anim: anim_name.clone(),
                    });
                }
                if !(spec.fps.is_finite() && spec.fps > 0.0) {
                    return Err(PackError::BadFps {
                        stage: key.to_string(),
                        anim: anim_name.clone(),
                    });
                }
                let mut frames = Vec::with_capacity(spec.frames.len());
                for frame_name in &spec.frames {
                    let frame_key = format!("{key}/{frame_name}");
                    let text = sources
                        .iter()
                        .find(|(k, _)| *k == frame_key)
                        .map(|(_, t)| *t)
                        .ok_or_else(|| PackError::MissingFrame {
                            key: frame_key.clone(),
                        })?;
                    let grid = parse_grid(&frame_key, text)?;
                    if grid.w != st.native || grid.h != st.native {
                        return Err(PackError::BadSize {
                            key: frame_key,
                            expected: st.native,
                            w: grid.w,
                            h: grid.h,
                        });
                    }
                    frames.push(grid);
                }
                anims.insert(
                    anim_name.clone(),
                    Anim {
                        fps: spec.fps,
                        frames,
                    },
                );
            }
            stages.insert(
                key,
                StagePack {
                    native: st.native,
                    anims,
                },
            );
        }
        Ok(Pack { stages })
    }

    /// Нативная сторона кадров стадии, px.
    pub fn native(&self, stage: Stage) -> u32 {
        self.stages[stage_key(stage)].native
    }

    /// fps семейства из манифеста (None — нет такого семейства).
    pub fn fps(&self, stage: Stage, anim: &str) -> Option<f32> {
        self.stages[stage_key(stage)].anims.get(anim).map(|a| a.fps)
    }

    /// Имена семейств стадии в порядке манифеста.
    pub fn anim_names(&self, stage: Stage) -> Vec<&str> {
        self.stages[stage_key(stage)]
            .anims
            .keys()
            .map(|s| s.as_str())
            .collect()
    }

    /// Кадры семейства, колоризованные и отмасштабированные под стадию
    /// (тот же множитель, что у [`Pack::sprite_set`] при `target_px`).
    pub fn frames(&self, stage: Stage, anim: &str, target_px: u32, argb: u32) -> Vec<Frame> {
        let st = &self.stages[stage_key(stage)];
        let k = (target_px / st.native).max(1);
        match st.anims.get(anim) {
            Some(a) => a
                .frames
                .iter()
                .map(|g| scale_nearest(&colorize(g, argb), k))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Собрать [`SpriteSet`] стадии: колоризация цветом `argb` и
    /// целочисленный апскейл `max(1, target_px / native)`.
    ///
    /// Семейства яйца (`egg`/`hatching`) в наборах всех стадий берутся из
    /// арта стадии `egg` — на них ссылается `frame_look` при `Stage::Egg`
    /// и оверлее вылупления, пустых семейств в наборе быть не должно.
    /// В наборе стадии `egg` остальные семейства заполнены кадрами яйца:
    /// яйцо не ходит, но state-машине всегда есть что показать.
    pub fn sprite_set(&self, stage: Stage, target_px: u32, argb: u32) -> SpriteSet {
        let st = &self.stages[stage_key(stage)];
        let k = (target_px / st.native).max(1);
        let egg_st = &self.stages[stage_key(Stage::Egg)];
        let fam = |anims: &BTreeMap<String, Anim>, name: &str| -> Vec<Frame> {
            // Обязательность семейств проверена в parse; unwrap безопасен,
            // но держим явный фолбэк на «egg», чтобы не паниковать никогда.
            let a = anims.get(name).unwrap_or(&egg_st.anims["egg"]);
            a.frames
                .iter()
                .map(|g| scale_nearest(&colorize(g, argb), k))
                .collect()
        };
        // Необязательные семейства фазы G: нет в паке — пустой вектор,
        // SpriteSet::frame_look откатится к базовому семейству.
        let opt = |anims: &BTreeMap<String, Anim>, name: &str| -> Vec<Frame> {
            anims
                .get(name)
                .map(|a| {
                    a.frames
                        .iter()
                        .map(|g| scale_nearest(&colorize(g, argb), k))
                        .collect()
                })
                .unwrap_or_default()
        };
        let egg = fam(&egg_st.anims, "egg");
        let hatching = fam(&egg_st.anims, "hatch");
        if stage == Stage::Egg {
            return SpriteSet {
                size: st.native * k,
                idle: egg.clone(),
                walk: egg.clone(),
                sleep: egg.clone(),
                falling: egg.clone(),
                dragged: egg.clone(),
                landing: egg.clone(),
                eating: egg.clone(),
                sad: egg.clone(),
                sick: egg.clone(),
                happy: egg.clone(),
                egg,
                hatching,
                // Яйцо не лазает и не потягивается — у него один вид.
                blink: Vec::new(),
                sit: Vec::new(),
                stretch: Vec::new(),
                wiggle: Vec::new(),
                cling: Vec::new(),
                climb: Vec::new(),
                dizzy: Vec::new(),
                hang: Vec::new(),
                swing: Vec::new(),
                vomit: Vec::new(),
                grip_inset: Default::default(),
                feet_inset: Default::default(),
            }
            .with_grip_inset();
        }
        SpriteSet {
            size: st.native * k,
            idle: fam(&st.anims, "idle"),
            walk: fam(&st.anims, "walk"),
            sleep: fam(&st.anims, "sleep"),
            falling: fam(&st.anims, "falling"),
            dragged: fam(&st.anims, "dragged"),
            landing: fam(&st.anims, "landing"),
            eating: fam(&st.anims, "eating"),
            sad: fam(&st.anims, "sad"),
            sick: fam(&st.anims, "sick"),
            happy: fam(&st.anims, "happy"),
            egg,
            hatching,
            blink: opt(&st.anims, "blink"),
            sit: opt(&st.anims, "sit"),
            stretch: opt(&st.anims, "stretch"),
            wiggle: opt(&st.anims, "wiggle"),
            cling: opt(&st.anims, "cling"),
            climb: opt(&st.anims, "climb"),
            dizzy: opt(&st.anims, "dizzy"),
            hang: opt(&st.anims, "hang"),
            swing: opt(&st.anims, "swing"),
            vomit: opt(&st.anims, "vomit"),
            grip_inset: Default::default(),
            feet_inset: Default::default(),
        }
        .with_grip_inset()
    }
}

// ---- Встроенный пак по умолчанию ----

static PACK_TOML: &str = include_str!("../../../assets/pack-default/pack.toml");

/// Кадры встроенного пака; ключ — «стадия/имя».
static FRAME_SOURCES: &[(&str, &str)] = &[
    (
        "egg/egg_0",
        include_str!("../../../assets/pack-default/egg/egg_0.txt"),
    ),
    (
        "egg/egg_1",
        include_str!("../../../assets/pack-default/egg/egg_1.txt"),
    ),
    (
        "egg/hatch_0",
        include_str!("../../../assets/pack-default/egg/hatch_0.txt"),
    ),
    (
        "egg/hatch_1",
        include_str!("../../../assets/pack-default/egg/hatch_1.txt"),
    ),
    (
        "egg/hatch_2",
        include_str!("../../../assets/pack-default/egg/hatch_2.txt"),
    ),
    (
        "baby/blink_0",
        include_str!("../../../assets/pack-default/baby/blink_0.txt"),
    ),
    (
        "baby/climb_0",
        include_str!("../../../assets/pack-default/baby/climb_0.txt"),
    ),
    (
        "baby/climb_1",
        include_str!("../../../assets/pack-default/baby/climb_1.txt"),
    ),
    (
        "baby/climb_2",
        include_str!("../../../assets/pack-default/baby/climb_2.txt"),
    ),
    (
        "baby/climb_3",
        include_str!("../../../assets/pack-default/baby/climb_3.txt"),
    ),
    (
        "baby/cling_0",
        include_str!("../../../assets/pack-default/baby/cling_0.txt"),
    ),
    (
        "baby/cling_1",
        include_str!("../../../assets/pack-default/baby/cling_1.txt"),
    ),
    (
        "baby/dizzy_0",
        include_str!("../../../assets/pack-default/baby/dizzy_0.txt"),
    ),
    (
        "baby/dizzy_1",
        include_str!("../../../assets/pack-default/baby/dizzy_1.txt"),
    ),
    (
        "baby/dragged_0",
        include_str!("../../../assets/pack-default/baby/dragged_0.txt"),
    ),
    (
        "baby/eating_0",
        include_str!("../../../assets/pack-default/baby/eating_0.txt"),
    ),
    (
        "baby/eating_1",
        include_str!("../../../assets/pack-default/baby/eating_1.txt"),
    ),
    (
        "baby/falling_0",
        include_str!("../../../assets/pack-default/baby/falling_0.txt"),
    ),
    (
        "baby/hang_0",
        include_str!("../../../assets/pack-default/baby/hang_0.txt"),
    ),
    (
        "baby/hang_1",
        include_str!("../../../assets/pack-default/baby/hang_1.txt"),
    ),
    (
        "baby/happy_0",
        include_str!("../../../assets/pack-default/baby/happy_0.txt"),
    ),
    (
        "baby/happy_1",
        include_str!("../../../assets/pack-default/baby/happy_1.txt"),
    ),
    (
        "baby/idle_0",
        include_str!("../../../assets/pack-default/baby/idle_0.txt"),
    ),
    (
        "baby/idle_1",
        include_str!("../../../assets/pack-default/baby/idle_1.txt"),
    ),
    (
        "baby/landing_0",
        include_str!("../../../assets/pack-default/baby/landing_0.txt"),
    ),
    (
        "baby/profile_0",
        include_str!("../../../assets/pack-default/baby/profile_0.txt"),
    ),
    (
        "baby/profile_1",
        include_str!("../../../assets/pack-default/baby/profile_1.txt"),
    ),
    (
        "baby/sad_0",
        include_str!("../../../assets/pack-default/baby/sad_0.txt"),
    ),
    (
        "baby/sick_0",
        include_str!("../../../assets/pack-default/baby/sick_0.txt"),
    ),
    (
        "baby/sit_0",
        include_str!("../../../assets/pack-default/baby/sit_0.txt"),
    ),
    (
        "baby/sit_1",
        include_str!("../../../assets/pack-default/baby/sit_1.txt"),
    ),
    (
        "baby/sleep_0",
        include_str!("../../../assets/pack-default/baby/sleep_0.txt"),
    ),
    (
        "baby/sleep_1",
        include_str!("../../../assets/pack-default/baby/sleep_1.txt"),
    ),
    (
        "baby/stretch_0",
        include_str!("../../../assets/pack-default/baby/stretch_0.txt"),
    ),
    (
        "baby/stretch_1",
        include_str!("../../../assets/pack-default/baby/stretch_1.txt"),
    ),
    (
        "baby/swing_0",
        include_str!("../../../assets/pack-default/baby/swing_0.txt"),
    ),
    (
        "baby/swing_1",
        include_str!("../../../assets/pack-default/baby/swing_1.txt"),
    ),
    (
        "baby/swing_2",
        include_str!("../../../assets/pack-default/baby/swing_2.txt"),
    ),
    (
        "baby/swing_3",
        include_str!("../../../assets/pack-default/baby/swing_3.txt"),
    ),
    (
        "baby/vomit_0",
        include_str!("../../../assets/pack-default/baby/vomit_0.txt"),
    ),
    (
        "baby/vomit_1",
        include_str!("../../../assets/pack-default/baby/vomit_1.txt"),
    ),
    (
        "baby/walk_0",
        include_str!("../../../assets/pack-default/baby/walk_0.txt"),
    ),
    (
        "baby/walk_1",
        include_str!("../../../assets/pack-default/baby/walk_1.txt"),
    ),
    (
        "baby/walk_2",
        include_str!("../../../assets/pack-default/baby/walk_2.txt"),
    ),
    (
        "baby/walk_3",
        include_str!("../../../assets/pack-default/baby/walk_3.txt"),
    ),
    (
        "baby/wiggle_0",
        include_str!("../../../assets/pack-default/baby/wiggle_0.txt"),
    ),
    (
        "baby/wiggle_1",
        include_str!("../../../assets/pack-default/baby/wiggle_1.txt"),
    ),
    (
        "child/blink_0",
        include_str!("../../../assets/pack-default/child/blink_0.txt"),
    ),
    (
        "child/climb_0",
        include_str!("../../../assets/pack-default/child/climb_0.txt"),
    ),
    (
        "child/climb_1",
        include_str!("../../../assets/pack-default/child/climb_1.txt"),
    ),
    (
        "child/climb_2",
        include_str!("../../../assets/pack-default/child/climb_2.txt"),
    ),
    (
        "child/climb_3",
        include_str!("../../../assets/pack-default/child/climb_3.txt"),
    ),
    (
        "child/cling_0",
        include_str!("../../../assets/pack-default/child/cling_0.txt"),
    ),
    (
        "child/cling_1",
        include_str!("../../../assets/pack-default/child/cling_1.txt"),
    ),
    (
        "child/dizzy_0",
        include_str!("../../../assets/pack-default/child/dizzy_0.txt"),
    ),
    (
        "child/dizzy_1",
        include_str!("../../../assets/pack-default/child/dizzy_1.txt"),
    ),
    (
        "child/dragged_0",
        include_str!("../../../assets/pack-default/child/dragged_0.txt"),
    ),
    (
        "child/eating_0",
        include_str!("../../../assets/pack-default/child/eating_0.txt"),
    ),
    (
        "child/eating_1",
        include_str!("../../../assets/pack-default/child/eating_1.txt"),
    ),
    (
        "child/falling_0",
        include_str!("../../../assets/pack-default/child/falling_0.txt"),
    ),
    (
        "child/hang_0",
        include_str!("../../../assets/pack-default/child/hang_0.txt"),
    ),
    (
        "child/hang_1",
        include_str!("../../../assets/pack-default/child/hang_1.txt"),
    ),
    (
        "child/happy_0",
        include_str!("../../../assets/pack-default/child/happy_0.txt"),
    ),
    (
        "child/happy_1",
        include_str!("../../../assets/pack-default/child/happy_1.txt"),
    ),
    (
        "child/idle_0",
        include_str!("../../../assets/pack-default/child/idle_0.txt"),
    ),
    (
        "child/idle_1",
        include_str!("../../../assets/pack-default/child/idle_1.txt"),
    ),
    (
        "child/landing_0",
        include_str!("../../../assets/pack-default/child/landing_0.txt"),
    ),
    (
        "child/profile_0",
        include_str!("../../../assets/pack-default/child/profile_0.txt"),
    ),
    (
        "child/profile_1",
        include_str!("../../../assets/pack-default/child/profile_1.txt"),
    ),
    (
        "child/sad_0",
        include_str!("../../../assets/pack-default/child/sad_0.txt"),
    ),
    (
        "child/sick_0",
        include_str!("../../../assets/pack-default/child/sick_0.txt"),
    ),
    (
        "child/sit_0",
        include_str!("../../../assets/pack-default/child/sit_0.txt"),
    ),
    (
        "child/sit_1",
        include_str!("../../../assets/pack-default/child/sit_1.txt"),
    ),
    (
        "child/sleep_0",
        include_str!("../../../assets/pack-default/child/sleep_0.txt"),
    ),
    (
        "child/sleep_1",
        include_str!("../../../assets/pack-default/child/sleep_1.txt"),
    ),
    (
        "child/stretch_0",
        include_str!("../../../assets/pack-default/child/stretch_0.txt"),
    ),
    (
        "child/stretch_1",
        include_str!("../../../assets/pack-default/child/stretch_1.txt"),
    ),
    (
        "child/swing_0",
        include_str!("../../../assets/pack-default/child/swing_0.txt"),
    ),
    (
        "child/swing_1",
        include_str!("../../../assets/pack-default/child/swing_1.txt"),
    ),
    (
        "child/swing_2",
        include_str!("../../../assets/pack-default/child/swing_2.txt"),
    ),
    (
        "child/swing_3",
        include_str!("../../../assets/pack-default/child/swing_3.txt"),
    ),
    (
        "child/vomit_0",
        include_str!("../../../assets/pack-default/child/vomit_0.txt"),
    ),
    (
        "child/vomit_1",
        include_str!("../../../assets/pack-default/child/vomit_1.txt"),
    ),
    (
        "child/walk_0",
        include_str!("../../../assets/pack-default/child/walk_0.txt"),
    ),
    (
        "child/walk_1",
        include_str!("../../../assets/pack-default/child/walk_1.txt"),
    ),
    (
        "child/walk_2",
        include_str!("../../../assets/pack-default/child/walk_2.txt"),
    ),
    (
        "child/walk_3",
        include_str!("../../../assets/pack-default/child/walk_3.txt"),
    ),
    (
        "child/wiggle_0",
        include_str!("../../../assets/pack-default/child/wiggle_0.txt"),
    ),
    (
        "child/wiggle_1",
        include_str!("../../../assets/pack-default/child/wiggle_1.txt"),
    ),
    (
        "teen/blink_0",
        include_str!("../../../assets/pack-default/teen/blink_0.txt"),
    ),
    (
        "teen/climb_0",
        include_str!("../../../assets/pack-default/teen/climb_0.txt"),
    ),
    (
        "teen/climb_1",
        include_str!("../../../assets/pack-default/teen/climb_1.txt"),
    ),
    (
        "teen/climb_2",
        include_str!("../../../assets/pack-default/teen/climb_2.txt"),
    ),
    (
        "teen/climb_3",
        include_str!("../../../assets/pack-default/teen/climb_3.txt"),
    ),
    (
        "teen/cling_0",
        include_str!("../../../assets/pack-default/teen/cling_0.txt"),
    ),
    (
        "teen/cling_1",
        include_str!("../../../assets/pack-default/teen/cling_1.txt"),
    ),
    (
        "teen/dizzy_0",
        include_str!("../../../assets/pack-default/teen/dizzy_0.txt"),
    ),
    (
        "teen/dizzy_1",
        include_str!("../../../assets/pack-default/teen/dizzy_1.txt"),
    ),
    (
        "teen/dragged_0",
        include_str!("../../../assets/pack-default/teen/dragged_0.txt"),
    ),
    (
        "teen/eating_0",
        include_str!("../../../assets/pack-default/teen/eating_0.txt"),
    ),
    (
        "teen/eating_1",
        include_str!("../../../assets/pack-default/teen/eating_1.txt"),
    ),
    (
        "teen/falling_0",
        include_str!("../../../assets/pack-default/teen/falling_0.txt"),
    ),
    (
        "teen/hang_0",
        include_str!("../../../assets/pack-default/teen/hang_0.txt"),
    ),
    (
        "teen/hang_1",
        include_str!("../../../assets/pack-default/teen/hang_1.txt"),
    ),
    (
        "teen/happy_0",
        include_str!("../../../assets/pack-default/teen/happy_0.txt"),
    ),
    (
        "teen/happy_1",
        include_str!("../../../assets/pack-default/teen/happy_1.txt"),
    ),
    (
        "teen/idle_0",
        include_str!("../../../assets/pack-default/teen/idle_0.txt"),
    ),
    (
        "teen/idle_1",
        include_str!("../../../assets/pack-default/teen/idle_1.txt"),
    ),
    (
        "teen/landing_0",
        include_str!("../../../assets/pack-default/teen/landing_0.txt"),
    ),
    (
        "teen/profile_0",
        include_str!("../../../assets/pack-default/teen/profile_0.txt"),
    ),
    (
        "teen/profile_1",
        include_str!("../../../assets/pack-default/teen/profile_1.txt"),
    ),
    (
        "teen/sad_0",
        include_str!("../../../assets/pack-default/teen/sad_0.txt"),
    ),
    (
        "teen/sick_0",
        include_str!("../../../assets/pack-default/teen/sick_0.txt"),
    ),
    (
        "teen/sit_0",
        include_str!("../../../assets/pack-default/teen/sit_0.txt"),
    ),
    (
        "teen/sit_1",
        include_str!("../../../assets/pack-default/teen/sit_1.txt"),
    ),
    (
        "teen/sleep_0",
        include_str!("../../../assets/pack-default/teen/sleep_0.txt"),
    ),
    (
        "teen/sleep_1",
        include_str!("../../../assets/pack-default/teen/sleep_1.txt"),
    ),
    (
        "teen/stretch_0",
        include_str!("../../../assets/pack-default/teen/stretch_0.txt"),
    ),
    (
        "teen/stretch_1",
        include_str!("../../../assets/pack-default/teen/stretch_1.txt"),
    ),
    (
        "teen/swing_0",
        include_str!("../../../assets/pack-default/teen/swing_0.txt"),
    ),
    (
        "teen/swing_1",
        include_str!("../../../assets/pack-default/teen/swing_1.txt"),
    ),
    (
        "teen/swing_2",
        include_str!("../../../assets/pack-default/teen/swing_2.txt"),
    ),
    (
        "teen/swing_3",
        include_str!("../../../assets/pack-default/teen/swing_3.txt"),
    ),
    (
        "teen/vomit_0",
        include_str!("../../../assets/pack-default/teen/vomit_0.txt"),
    ),
    (
        "teen/vomit_1",
        include_str!("../../../assets/pack-default/teen/vomit_1.txt"),
    ),
    (
        "teen/walk_0",
        include_str!("../../../assets/pack-default/teen/walk_0.txt"),
    ),
    (
        "teen/walk_1",
        include_str!("../../../assets/pack-default/teen/walk_1.txt"),
    ),
    (
        "teen/walk_2",
        include_str!("../../../assets/pack-default/teen/walk_2.txt"),
    ),
    (
        "teen/walk_3",
        include_str!("../../../assets/pack-default/teen/walk_3.txt"),
    ),
    (
        "teen/wiggle_0",
        include_str!("../../../assets/pack-default/teen/wiggle_0.txt"),
    ),
    (
        "teen/wiggle_1",
        include_str!("../../../assets/pack-default/teen/wiggle_1.txt"),
    ),
    (
        "adult/blink_0",
        include_str!("../../../assets/pack-default/adult/blink_0.txt"),
    ),
    (
        "adult/climb_0",
        include_str!("../../../assets/pack-default/adult/climb_0.txt"),
    ),
    (
        "adult/climb_1",
        include_str!("../../../assets/pack-default/adult/climb_1.txt"),
    ),
    (
        "adult/climb_2",
        include_str!("../../../assets/pack-default/adult/climb_2.txt"),
    ),
    (
        "adult/climb_3",
        include_str!("../../../assets/pack-default/adult/climb_3.txt"),
    ),
    (
        "adult/cling_0",
        include_str!("../../../assets/pack-default/adult/cling_0.txt"),
    ),
    (
        "adult/cling_1",
        include_str!("../../../assets/pack-default/adult/cling_1.txt"),
    ),
    (
        "adult/dizzy_0",
        include_str!("../../../assets/pack-default/adult/dizzy_0.txt"),
    ),
    (
        "adult/dizzy_1",
        include_str!("../../../assets/pack-default/adult/dizzy_1.txt"),
    ),
    (
        "adult/dragged_0",
        include_str!("../../../assets/pack-default/adult/dragged_0.txt"),
    ),
    (
        "adult/eating_0",
        include_str!("../../../assets/pack-default/adult/eating_0.txt"),
    ),
    (
        "adult/eating_1",
        include_str!("../../../assets/pack-default/adult/eating_1.txt"),
    ),
    (
        "adult/falling_0",
        include_str!("../../../assets/pack-default/adult/falling_0.txt"),
    ),
    (
        "adult/hang_0",
        include_str!("../../../assets/pack-default/adult/hang_0.txt"),
    ),
    (
        "adult/hang_1",
        include_str!("../../../assets/pack-default/adult/hang_1.txt"),
    ),
    (
        "adult/happy_0",
        include_str!("../../../assets/pack-default/adult/happy_0.txt"),
    ),
    (
        "adult/happy_1",
        include_str!("../../../assets/pack-default/adult/happy_1.txt"),
    ),
    (
        "adult/idle_0",
        include_str!("../../../assets/pack-default/adult/idle_0.txt"),
    ),
    (
        "adult/idle_1",
        include_str!("../../../assets/pack-default/adult/idle_1.txt"),
    ),
    (
        "adult/landing_0",
        include_str!("../../../assets/pack-default/adult/landing_0.txt"),
    ),
    (
        "adult/profile_0",
        include_str!("../../../assets/pack-default/adult/profile_0.txt"),
    ),
    (
        "adult/profile_1",
        include_str!("../../../assets/pack-default/adult/profile_1.txt"),
    ),
    (
        "adult/sad_0",
        include_str!("../../../assets/pack-default/adult/sad_0.txt"),
    ),
    (
        "adult/sick_0",
        include_str!("../../../assets/pack-default/adult/sick_0.txt"),
    ),
    (
        "adult/sit_0",
        include_str!("../../../assets/pack-default/adult/sit_0.txt"),
    ),
    (
        "adult/sit_1",
        include_str!("../../../assets/pack-default/adult/sit_1.txt"),
    ),
    (
        "adult/sleep_0",
        include_str!("../../../assets/pack-default/adult/sleep_0.txt"),
    ),
    (
        "adult/sleep_1",
        include_str!("../../../assets/pack-default/adult/sleep_1.txt"),
    ),
    (
        "adult/stretch_0",
        include_str!("../../../assets/pack-default/adult/stretch_0.txt"),
    ),
    (
        "adult/stretch_1",
        include_str!("../../../assets/pack-default/adult/stretch_1.txt"),
    ),
    (
        "adult/swing_0",
        include_str!("../../../assets/pack-default/adult/swing_0.txt"),
    ),
    (
        "adult/swing_1",
        include_str!("../../../assets/pack-default/adult/swing_1.txt"),
    ),
    (
        "adult/swing_2",
        include_str!("../../../assets/pack-default/adult/swing_2.txt"),
    ),
    (
        "adult/swing_3",
        include_str!("../../../assets/pack-default/adult/swing_3.txt"),
    ),
    (
        "adult/vomit_0",
        include_str!("../../../assets/pack-default/adult/vomit_0.txt"),
    ),
    (
        "adult/vomit_1",
        include_str!("../../../assets/pack-default/adult/vomit_1.txt"),
    ),
    (
        "adult/walk_0",
        include_str!("../../../assets/pack-default/adult/walk_0.txt"),
    ),
    (
        "adult/walk_1",
        include_str!("../../../assets/pack-default/adult/walk_1.txt"),
    ),
    (
        "adult/walk_2",
        include_str!("../../../assets/pack-default/adult/walk_2.txt"),
    ),
    (
        "adult/walk_3",
        include_str!("../../../assets/pack-default/adult/walk_3.txt"),
    ),
    (
        "adult/wiggle_0",
        include_str!("../../../assets/pack-default/adult/wiggle_0.txt"),
    ),
    (
        "adult/wiggle_1",
        include_str!("../../../assets/pack-default/adult/wiggle_1.txt"),
    ),
];

/// Встроенный пак. Разбирается один раз; валидность гарантируют тесты
/// (`default_pack_is_valid`), поэтому ошибка здесь — сломанная сборка.
pub fn default_pack() -> Result<&'static Pack, PackError> {
    static PACK: OnceLock<Result<Pack, PackError>> = OnceLock::new();
    PACK.get_or_init(|| Pack::parse(PACK_TOML, FRAME_SOURCES))
        .as_ref()
        .map_err(|e| e.clone())
}

/// [`SpriteSet`] стадии из встроенного пака: цвет `argb`, целевой размер
/// `target_px` (фактический — ближайший целый множитель native).
pub fn default_sprite_set(stage: Stage, target_px: u32, argb: u32) -> Result<SpriteSet, PackError> {
    Ok(default_pack()?.sprite_set(stage, target_px, argb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{DEFAULT_PET_COLOR, PET_PRESETS};

    #[test]
    fn parse_grid_roundtrip() {
        let g = parse_grid("t/ok", ".BO\nDES\nCWA\n\n").unwrap();
        assert_eq!((g.w, g.h), (3, 3));
        assert_eq!(g.at(0, 0), '.');
        assert_eq!(g.at(2, 2), 'A');
    }

    #[test]
    fn parse_grid_rejects_garbage() {
        assert_eq!(
            parse_grid("t/e", "\n\n"),
            Err(PackError::EmptyGrid { key: "t/e".into() })
        );
        assert_eq!(
            parse_grid("t/r", "..\n...\n"),
            Err(PackError::RaggedGrid {
                key: "t/r".into(),
                row: 1
            })
        );
        assert_eq!(
            parse_grid("t/c", ".B\n.Z\n"),
            Err(PackError::BadChar {
                key: "t/c".into(),
                row: 1,
                col: 1,
                ch: 'Z'
            })
        );
    }

    #[test]
    fn colorize_maps_every_char() {
        let g = parse_grid("t/all", ".BDOE\nSCWAX\n").unwrap();
        let f = colorize(&g, DEFAULT_PET_COLOR);
        let body = DEFAULT_PET_COLOR;
        assert_eq!(f.argb[0], 0, "точка прозрачна");
        assert_eq!(f.argb[1], body);
        assert_eq!(f.argb[2], palette::darken(body, 0.78));
        assert_eq!(f.argb[3], palette::darken(body, 0.45));
        assert_eq!(f.argb[4], EYE_INK);
        assert_eq!(f.argb[5], SHINE);
        assert_eq!(f.argb[6], CHEEK_PINK);
        assert_eq!(f.argb[7], mix(SHINE, body, 0.18));
        assert_eq!(f.argb[8], mix(SHINE, body, 0.55));
        assert_eq!(f.argb[9], FIXED_DARK);
        // Альфа строго 0 или 255 — premultiplied-совместимость.
        assert!(f.argb.iter().all(|&p| p >> 24 == 0 || p >> 24 == 0xff));
    }

    #[test]
    fn cheeks_keep_contrast_on_dark_bodies() {
        let dark = 0xff_20_20_30;
        let g = parse_grid("t/c", "C").unwrap();
        assert_eq!(colorize(&g, dark).argb[0], palette::lighten(dark, 1.3));
        assert_eq!(colorize(&g, DEFAULT_PET_COLOR).argb[0], CHEEK_PINK);
    }

    #[test]
    fn scale_nearest_makes_blocks() {
        let g = parse_grid("t/s", ".B\nO.").unwrap();
        let f = scale_nearest(&colorize(&g, DEFAULT_PET_COLOR), 3);
        assert_eq!((f.w, f.h), (6, 6));
        // Верхний левый блок 3x3 прозрачный, правый — тело.
        assert_eq!(f.argb[0], 0);
        assert_eq!(f.argb[2], 0);
        assert_eq!(f.argb[3], DEFAULT_PET_COLOR);
        assert_eq!(f.argb[5 * 6], palette::darken(DEFAULT_PET_COLOR, 0.45));
        // k=1 и k=0 — идентичность.
        let one = colorize(&g, DEFAULT_PET_COLOR);
        assert_eq!(scale_nearest(&one, 1).argb, one.argb);
        assert_eq!(scale_nearest(&one, 0).argb, one.argb);
    }

    // ---- Встроенный пак ----

    #[test]
    fn default_pack_is_valid() {
        let pack = default_pack().expect("встроенный пак обязан разбираться");
        assert_eq!(pack.native(Stage::Egg), 24);
        assert_eq!(pack.native(Stage::Baby), 24);
        assert_eq!(pack.native(Stage::Child), 28);
        assert_eq!(pack.native(Stage::Teen), 32);
        assert_eq!(pack.native(Stage::Adult), 32);
        for stage in [Stage::Baby, Stage::Child, Stage::Teen, Stage::Adult] {
            for anim in BODY_ANIMS {
                assert!(pack.fps(stage, anim).is_some(), "{stage:?}/{anim}: нет fps");
            }
        }
        for anim in EGG_ANIMS {
            assert!(pack.fps(Stage::Egg, anim).unwrap() > 0.0);
        }
        // Ходьба — полный цикл из 4 кадров, идл дышит двумя.
        assert_eq!(
            pack.frames(Stage::Adult, "walk", 32, DEFAULT_PET_COLOR)
                .len(),
            4
        );
        assert_eq!(
            pack.frames(Stage::Adult, "idle", 32, DEFAULT_PET_COLOR)
                .len(),
            2
        );
        assert_eq!(
            pack.frames(Stage::Egg, "hatch", 24, DEFAULT_PET_COLOR)
                .len(),
            3
        );
    }

    #[test]
    fn default_sprite_sets_are_complete_and_scaled() {
        for stage in ALL_STAGES {
            let native = default_pack().unwrap().native(stage);
            // target 96 -> целый множитель от native.
            let set = default_sprite_set(stage, 96, DEFAULT_PET_COLOR).unwrap();
            let k = 96 / native;
            assert_eq!(set.size, native * k, "{stage:?}: размер набора");
            for (name, frames) in [
                ("idle", &set.idle),
                ("walk", &set.walk),
                ("sleep", &set.sleep),
                ("falling", &set.falling),
                ("dragged", &set.dragged),
                ("landing", &set.landing),
                ("egg", &set.egg),
                ("hatching", &set.hatching),
                ("eating", &set.eating),
                ("sad", &set.sad),
                ("sick", &set.sick),
                ("happy", &set.happy),
            ] {
                assert!(!frames.is_empty(), "{stage:?}/{name}: пусто");
                for f in frames {
                    assert!(
                        f.argb.iter().any(|&p| p >> 24 != 0),
                        "{stage:?}/{name}: пустой кадр"
                    );
                }
            }
            // Слишком маленький target тоже безопасен (k >= 1).
            let tiny = default_sprite_set(stage, 8, DEFAULT_PET_COLOR).unwrap();
            assert_eq!(tiny.size, native);
        }
    }

    #[test]
    fn default_pack_recolors_distinctly() {
        let mut bodies: Vec<Vec<u32>> = Vec::new();
        for &(argb, name) in PET_PRESETS {
            let set = default_sprite_set(Stage::Adult, 32, argb).unwrap();
            assert!(
                set.idle[0].argb.iter().any(|&p| p >> 24 != 0),
                "{name}: пустой идл"
            );
            bodies.push(set.idle[0].argb.clone());
        }
        for i in 0..bodies.len() {
            for j in i + 1..bodies.len() {
                assert_ne!(bodies[i], bodies[j], "пресеты {i} и {j} совпали");
            }
        }
    }

    #[test]
    fn egg_stage_set_reuses_egg_frames() {
        let set = default_sprite_set(Stage::Egg, 24, DEFAULT_PET_COLOR).unwrap();
        assert_eq!(set.idle[0].argb, set.egg[0].argb);
        assert_eq!(set.walk[0].argb, set.egg[0].argb);
        assert_eq!(set.hatching.len(), 3);
    }

    // ---- Ошибки манифеста ----

    const GRID_2X2: &str = ".B\nO.";

    fn mini_manifest(native: u32, fps: f32) -> String {
        let mut s = format!("format = 1\n[stages.egg]\nnative = {native}\n");
        s.push_str(&format!(
            "[stages.egg.anims.egg]\nfps = {fps}\nframes = [\"egg_0\"]\n"
        ));
        s.push_str(&format!(
            "[stages.egg.anims.hatch]\nfps = {fps}\nframes = [\"egg_0\"]\n"
        ));
        for st in ["baby", "child", "teen", "adult"] {
            s.push_str(&format!("[stages.{st}]\nnative = {native}\n"));
            for anim in BODY_ANIMS {
                s.push_str(&format!(
                    "[stages.{st}.anims.{anim}]\nfps = {fps}\nframes = [\"f_0\"]\n"
                ));
            }
        }
        s
    }

    fn mini_sources() -> Vec<(&'static str, &'static str)> {
        let mut v = vec![("egg/egg_0", GRID_2X2)];
        for st in ["baby", "child", "teen", "adult"] {
            v.push(match st {
                "baby" => ("baby/f_0", GRID_2X2),
                "child" => ("child/f_0", GRID_2X2),
                "teen" => ("teen/f_0", GRID_2X2),
                _ => ("adult/f_0", GRID_2X2),
            });
        }
        v
    }

    /// Фаза G: встроенный пак несёт новые семейства на всех стадиях,
    /// а SpriteSet отдаёт их кадры (не откат к базовым).
    #[test]
    fn default_pack_has_phase_g_families() {
        let pack = default_pack().unwrap();
        for stage in [Stage::Baby, Stage::Child, Stage::Teen, Stage::Adult] {
            for anim in EXTRA_ANIMS {
                assert!(
                    pack.fps(stage, anim).is_some(),
                    "{stage:?}/{anim}: нет в манифесте"
                );
            }
        }
        let set = pack.sprite_set(Stage::Adult, 96, DEFAULT_PET_COLOR);
        assert_eq!(set.climb.len(), 4);
        assert_eq!(set.cling.len(), 2);
        assert_eq!(set.blink.len(), 1);
        assert_eq!(set.dizzy.len(), 2);
        assert!(!set.sit.is_empty() && !set.stretch.is_empty() && !set.wiggle.is_empty());
    }

    /// Совместимость: пак формата 1 БЕЗ новых семейств грузится как раньше,
    /// а движок откатывается к базовым кадрам.
    #[test]
    fn pack_without_extra_families_still_loads() {
        let pack = default_pack().unwrap();
        let mut set = pack.sprite_set(Stage::Adult, 96, DEFAULT_PET_COLOR);
        set.climb.clear();
        set.cling.clear();
        set.dizzy.clear();
        let walk = set.frame_look(
            &crate::sprite::Look {
                state: crate::PetState::Climb,
                ..crate::sprite::Look::simple(crate::PetState::Climb, Stage::Adult)
            },
            0.0,
        );
        assert!(
            set.walk.iter().any(|f| core::ptr::eq(f, walk)),
            "лазание откатывается к ходьбе"
        );
    }

    #[test]
    fn mini_pack_parses_and_reports_errors() {
        let src = mini_sources();
        let ok = Pack::parse(&mini_manifest(2, 4.0), &src).unwrap();
        assert_eq!(ok.native(Stage::Adult), 2);
        assert_eq!(ok.fps(Stage::Adult, "idle"), Some(4.0));
        assert_eq!(ok.sprite_set(Stage::Adult, 8, DEFAULT_PET_COLOR).size, 8);

        // Не тот формат.
        let bad = mini_manifest(2, 4.0).replace("format = 1", "format = 9");
        assert_eq!(Pack::parse(&bad, &src), Err(PackError::Format(9)));
        // Не TOML вовсе.
        assert!(matches!(
            Pack::parse("узор { нет", &src),
            Err(PackError::Manifest(_))
        ));
        // Сетка не совпала с native.
        assert!(matches!(
            Pack::parse(&mini_manifest(3, 4.0), &src),
            Err(PackError::BadSize {
                expected: 3,
                w: 2,
                h: 2,
                ..
            })
        ));
        // fps нулевой.
        assert!(matches!(
            Pack::parse(&mini_manifest(2, 0.0), &src),
            Err(PackError::BadFps { .. })
        ));
        // Потеряли кадр.
        let no_adult: Vec<_> = src
            .iter()
            .copied()
            .filter(|(k, _)| *k != "adult/f_0")
            .collect();
        assert_eq!(
            Pack::parse(&mini_manifest(2, 4.0), &no_adult),
            Err(PackError::MissingFrame {
                key: "adult/f_0".into()
            })
        );
        // Чужая стадия в манифесте.
        let extra = format!(
            "{}[stages.elder]\nnative = 2\n[stages.elder.anims.idle]\nfps = 1.0\nframes = [\"f_0\"]\n",
            mini_manifest(2, 4.0)
        );
        assert_eq!(
            Pack::parse(&extra, &src),
            Err(PackError::UnknownStage("elder".into()))
        );
        // Стадия без обязательного семейства.
        let no_idle = mini_manifest(2, 4.0)
            .replace("[stages.adult.anims.idle]", "[stages.adult.anims.idle2]");
        assert_eq!(
            Pack::parse(&no_idle, &src),
            Err(PackError::MissingAnim {
                stage: "adult",
                anim: "idle"
            })
        );
        // Ошибки печатаются по-русски и не пустые.
        for e in [
            PackError::Format(9),
            PackError::MissingFrame { key: "a/b".into() },
            PackError::BadChar {
                key: "a/b".into(),
                row: 1,
                col: 2,
                ch: 'Z',
            },
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}
