//! Процедурный плейсхолдер-спрайт: круглый «дрифтлинг» с глазами и лапками.
//! Фаза B добавляет тамагочи-семейства: яйцо, вылупление, еда, настроение,
//! стадии роста. Настоящие паки (.driftpack) придут в M4; интерфейс кадров
//! уже финальный.

use crate::behavior::{IdleAction, PetState};
use crate::growth::Stage;
use crate::palette::{self, DEFAULT_PET_COLOR};
use crate::pet::Direction;
use crate::physics::Surface;
use crate::stats::PetStats;

/// Кадр: ARGB8888. Процедурные спрайты держат альфу 0 или 255; текстовые
/// кадры (text.rs) хранят premultiplied-альфу — детали там же.
#[derive(Debug, Clone)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub argb: Vec<u32>,
}

/// Градация настроения для выбора семейства кадров (ТЗ §3.2, фаза B2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoodTier {
    Happy,
    Ok,
    Sad,
    Sick,
}

/// Кратковременная анимация-оверлей поверх базового состояния.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionLook {
    Eating,
    Hatching,
    /// Укачало и тошнит (фаза G5).
    Vomiting,
    /// Чихает (фаза G5): короткое сжатие — кадр приземления.
    Sneezing,
    /// Машет лапкой (фаза G6): приветствие и прощание.
    Waving,
}

/// Полное описание внешнего вида питомца в кадре: состояние поведения,
/// стадия роста, градация настроения и опциональный оверлей-экшен.
/// Фаза G добавляет поверхность (на стене питомец цепляется, а не стоит)
/// и мелкое занятие в покое.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub state: PetState,
    pub stage: Stage,
    pub mood: MoodTier,
    pub overlay: Option<ActionLook>,
    pub surface: Surface,
    pub idle_action: IdleAction,
    /// Питомец стоит на кромке окна, а не на земле (фаза G6): сидя он
    /// свешивает лапки с карниза.
    pub on_ledge: bool,
}

impl Look {
    /// Простой вид: только состояние и стадия (для тестов и совместимости).
    pub fn simple(state: PetState, stage: Stage) -> Look {
        Look {
            state,
            stage,
            mood: MoodTier::Ok,
            overlay: None,
            surface: Surface::Floor,
            idle_action: IdleAction::Stand,
            on_ledge: false,
        }
    }
}

/// Градация настроения из статов (ТЗ §3.2). Отдельного флага болезни в
/// статах пока нет — болезнь читается как низкое здоровье.
pub fn mood_tier(stats: &PetStats) -> MoodTier {
    if stats.health < 40.0 {
        MoodTier::Sick
    } else if stats.mood < 30.0 || stats.satiety < 20.0 {
        MoodTier::Sad
    } else if stats.mood > 80.0 && stats.satiety > 60.0 {
        MoodTier::Happy
    } else {
        MoodTier::Ok
    }
}

/// Масштаб спрайта по стадии роста (доля от базового размера).
pub fn stage_scale(stage: Stage) -> f32 {
    match stage {
        Stage::Egg => 0.55,
        Stage::Baby => 0.6,
        Stage::Child => 0.75,
        Stage::Teen => 0.9,
        Stage::Adult => 1.0,
    }
}

/// Прозрачные поля кадра, px: сколько пустоты от края спрайта до рисунка.
/// Нужны, чтобы на стене и потолке питомец прижимался к поверхности
/// вплотную, а не висел в воздухе на ширину пустого поля кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inset {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

/// Минимальные (по всем кадрам семейства) прозрачные поля.
/// Пустое семейство или полностью прозрачные кадры — нули.
pub fn frames_inset(frames: &[Frame]) -> Inset {
    let mut acc: Option<Inset> = None;
    for f in frames {
        let mut min_x = f.w;
        let mut max_x = 0u32;
        let mut min_y = f.h;
        let mut max_y = 0u32;
        for y in 0..f.h {
            for x in 0..f.w {
                if f.argb[(y * f.w + x) as usize] >> 24 != 0 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        if min_x > max_x {
            continue; // кадр пуст
        }
        let cur = Inset {
            left: min_x,
            right: f.w - 1 - max_x,
            top: min_y,
            bottom: f.h - 1 - max_y,
        };
        acc = Some(match acc {
            None => cur,
            Some(a) => Inset {
                left: a.left.min(cur.left),
                right: a.right.min(cur.right),
                top: a.top.min(cur.top),
                bottom: a.bottom.min(cur.bottom),
            },
        });
    }
    acc.unwrap_or_default()
}

/// Набор кадров под каждое состояние. Все семейства сгенерированы под один
/// размер `size`; масштаб стадии применяется при генерации набора.
#[derive(Debug, Clone)]
pub struct SpriteSet {
    pub size: u32,
    pub idle: Vec<Frame>,
    pub walk: Vec<Frame>,
    pub sleep: Vec<Frame>,
    pub falling: Vec<Frame>,
    pub dragged: Vec<Frame>,
    pub landing: Vec<Frame>,
    pub egg: Vec<Frame>,
    pub hatching: Vec<Frame>,
    pub eating: Vec<Frame>,
    pub sad: Vec<Frame>,
    pub sick: Vec<Frame>,
    pub happy: Vec<Frame>,
    // ---- Фаза G: необязательные семейства ----
    // Пустой вектор = у пака этого семейства нет: кадр берётся из отката
    // (см. `frame_look`), поэтому старые .driftpack продолжают работать.
    /// Моргание — короткая пауза в покое.
    pub blink: Vec<Frame>,
    /// Сидит, поджав лапки.
    pub sit: Vec<Frame>,
    /// Потягивается с зевком.
    pub stretch: Vec<Frame>,
    /// Топчется/водит антенной.
    pub wiggle: Vec<Frame>,
    /// Держится за стену или потолок без движения.
    pub cling: Vec<Frame>,
    /// Ползёт по стене или потолку.
    pub climb: Vec<Frame>,
    /// Звёздочки после удара о потолок.
    pub dizzy: Vec<Frame>,
    /// Висит под потолком на лапках (обезьянка), покачиваясь.
    pub hang: Vec<Frame>,
    /// Перебирается по потолку рука за рукой.
    pub swing: Vec<Frame>,
    /// Тошнит после укачивания.
    pub vomit: Vec<Frame>,
    /// Машет лапкой: здоровается и прощается.
    pub wave: Vec<Frame>,
    /// Сидит на карнизе, свесив лапки.
    pub dangle: Vec<Frame>,
    /// Прозрачные поля поз хвата (climb/cling) — ими питомец прижимается
    /// к стене: спереди (морда и лапки) это правое поле кадра.
    pub grip_inset: Inset,
    /// Прозрачные поля ходьбы — ими он прижимается к потолку (низ кадра,
    /// там ноги). Заполняются сборкой набора.
    pub feet_inset: Inset,
}

impl SpriteSet {
    /// Пересчитать поля прижатия: к стене — по кадрам лазания (профиль,
    /// перёд кадра), к потолку — по кадрам ходьбы (ноги внизу кадра).
    pub fn with_grip_inset(mut self) -> Self {
        let grip = if !self.climb.is_empty() {
            &self.climb
        } else if !self.cling.is_empty() {
            &self.cling
        } else {
            &self.walk
        };
        self.grip_inset = frames_inset(grip);
        // К потолку обращён ВЕРХ кадров hang (руки); откат — верх idle.
        self.feet_inset = frames_inset(if !self.hang.is_empty() {
            &self.hang
        } else if self.walk.is_empty() {
            &self.idle
        } else {
            &self.walk
        });
        self
    }
}

impl SpriteSet {
    /// Кадр под полный внешний вид в момент `t` секунд с входа в состояние.
    /// Приоритет: оверлей-экшен -> стадия яйца -> поверхность (стена/потолок)
    /// -> мелкое занятие/настроение в покое -> базовое состояние.
    /// Семейства фазы G необязательны: пустое откатывается к базовому.
    pub fn frame_look(&self, look: &Look, t: f32) -> &Frame {
        if let Some(action) = look.overlay {
            return match action {
                ActionLook::Eating => pick(&self.eating, 4.0, t),
                ActionLook::Hatching => pick(&self.hatching, 2.0, t),
                ActionLook::Vomiting => pick_or(&self.vomit, &self.sick, 3.0, t),
                ActionLook::Sneezing => pick(&self.landing, 6.0, t),
                ActionLook::Waving => pick_or(&self.wave, &self.happy, 3.0, t),
            };
        }
        if look.stage == Stage::Egg {
            return pick(&self.egg, 1.5, t);
        }
        match look.state {
            // Под потолком — обезьянка: висит на лапках и перебирается
            // рука за рукой (откат — обычные кадры, если пак без них).
            PetState::Idle if look.surface == Surface::Ceiling => {
                pick_or(&self.hang, &self.idle, 1.5, t)
            }
            PetState::Climb if look.surface == Surface::Ceiling => {
                pick_or(&self.swing, &self.walk, 5.0, t)
            }
            // На стене — своя поза хвата, повёрнутая рендером на четверть.
            PetState::Idle if look.surface != Surface::Floor => {
                pick_or(&self.cling, &self.idle, 1.5, t)
            }
            PetState::Idle => self.idle_frame(look, t),
            PetState::Walk => pick(&self.walk, 6.0, t),
            PetState::Climb => pick_or(&self.climb, &self.walk, 5.0, t),
            PetState::Sleep => pick(&self.sleep, 1.0, t),
            PetState::Falling => pick(&self.falling, 8.0, t),
            PetState::Dragged => pick(&self.dragged, 4.0, t),
            PetState::Landing => pick(&self.landing, 6.0, t),
            PetState::Bonk => pick_or(&self.dizzy, &self.landing, 4.0, t),
            // Катится кубарем: компактная поза падения, вращение
            // добавляет рендер (Pet::orient).
            PetState::Roll => pick(&self.falling, 8.0, t),
        }
    }

    /// Кадр покоя на полу: плохое самочувствие важнее мелких занятий —
    /// больной питомец не потягивается и не топчется.
    fn idle_frame(&self, look: &Look, t: f32) -> &Frame {
        match look.mood {
            MoodTier::Sad => return pick(&self.sad, 1.5, t),
            MoodTier::Sick => return pick(&self.sick, 1.5, t),
            _ => {}
        }
        match look.idle_action {
            IdleAction::Blink => pick_or(&self.blink, &self.idle, 1.0, t),
            // На карнизе сидят, свесив лапки за край.
            IdleAction::Sit if look.on_ledge => pick_or(&self.dangle, &self.sit, 1.2, t),
            IdleAction::Sit => pick_or(&self.sit, &self.idle, 1.5, t),
            IdleAction::Stretch => pick_or(&self.stretch, &self.idle, 2.5, t),
            IdleAction::Wiggle => pick_or(&self.wiggle, &self.idle, 4.0, t),
            IdleAction::Stand => match look.mood {
                MoodTier::Happy => pick(&self.happy, 3.0, t),
                _ => pick(&self.idle, 2.0, t),
            },
        }
    }

    /// Тонкая обёртка совместимости (до волны 2): взрослый, нейтральное
    /// настроение, без оверлеев.
    pub fn frame(&self, state: PetState, t: f32, facing: Direction) -> &Frame {
        let _ = facing; // зеркалирование делает рендер по флагу facing
        self.frame_look(&Look::simple(state, Stage::Adult), t)
    }
}

fn pick(frames: &[Frame], fps: f32, t: f32) -> &Frame {
    let idx = ((t * fps) as usize) % frames.len().max(1);
    &frames[idx]
}

/// Кадр из `primary`, а если семейства в паке нет — из `fallback`.
fn pick_or<'a>(primary: &'a [Frame], fallback: &'a [Frame], fps: f32, t: f32) -> &'a Frame {
    if primary.is_empty() {
        pick(fallback, fps, t)
    } else {
        pick(primary, fps, t)
    }
}

const EYE: u32 = 0xff_1e_1e_2e;
const EYE_SHINE: u32 = 0xff_ff_ff_ff;
/// Розовые щёки — читаются на любом светлом теле.
const CHEEK_PINK: u32 = 0xff_e8_9a_c7;
const SICK_TINT: u32 = 0xff_7d_c4_7d; // зеленоватый оттенок болезни
const CRUMB: u32 = 0xff_d9_a0_66; // крошка у рта
/// Гард контраста: тело темнее этой яркости гасит розовые щёки —
/// вместо них берётся осветлённый тон самого тела.
const CHEEK_DARK_LUMA: f32 = 0.35;

/// Рабочая палитра отрисовки, выведенная из базового цвета тела
/// (пользовательская настройка, см. palette.rs и журнальное Recolored).
#[derive(Debug, Clone, Copy)]
struct BodyColors {
    body: u32,
    body_dark: u32,
    cheek: u32,
    /// Светлая скорлупа яйца — тело, сильно разбавленное белым.
    shell: u32,
    shell_dark: u32,
    /// Крапинки скорлупы — сам цвет тела.
    speckle: u32,
    crack: u32,
}

impl BodyColors {
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
            cheek,
            shell: mix(EYE_SHINE, body, 0.18),
            shell_dark: mix(EYE_SHINE, body, 0.45),
            speckle: body,
            crack: palette::darken(body, 0.55),
        }
    }
}

/// Сгенерировать набор кадров размером `size` px (квадрат) для взрослой
/// формы в цвете по умолчанию. Обёртка над [`placeholder_for_stage`].
pub fn placeholder(size: u32) -> SpriteSet {
    placeholder_for_stage(size, Stage::Adult)
}

/// Набор кадров под стадию в цвете по умолчанию — обёртка над
/// [`placeholder_colored`] для мест, которым цвет безразличен.
pub fn placeholder_for_stage(base_size: u32, stage: Stage) -> SpriteSet {
    placeholder_colored(base_size, stage, DEFAULT_PET_COLOR)
}

/// Сгенерировать набор кадров под стадию роста и базовый цвет тела:
/// базовый размер умножается на [`stage_scale`], яйцо рисуется собственной
/// формой (не сжатым блобом), все тона выводятся из `argb` (BodyColors).
pub fn placeholder_colored(base_size: u32, stage: Stage, argb: u32) -> SpriteSet {
    let size = (((base_size as f32) * stage_scale(stage)).round() as u32).max(16);
    let c = &BodyColors::from_argb(argb);
    SpriteSet {
        size,
        idle: vec![
            blob(size, BlobStyle::default(), c),
            blob(
                size,
                BlobStyle {
                    eyes: Eyes::Closed,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        walk: vec![
            blob_walk(size, 0.06, 0, c),
            blob_walk(size, 0.0, 1, c),
            blob_walk(size, 0.06, 2, c),
            blob_walk(size, 0.0, 3, c),
        ],
        sleep: vec![
            blob(
                size,
                BlobStyle {
                    squash: 0.12,
                    eyes: Eyes::Closed,
                    zzz: true,
                    ..BlobStyle::default()
                },
                c,
            ),
            blob(
                size,
                BlobStyle {
                    squash: 0.16,
                    eyes: Eyes::Closed,
                    zzz: true,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        falling: vec![blob(
            size,
            BlobStyle {
                squash: -0.1,
                ..BlobStyle::default()
            },
            c,
        )],
        dragged: vec![blob(
            size,
            BlobStyle {
                squash: -0.05,
                ..BlobStyle::default()
            },
            c,
        )],
        landing: vec![blob(
            size,
            BlobStyle {
                squash: 0.22,
                ..BlobStyle::default()
            },
            c,
        )],
        egg: vec![egg_frame(size, 0.05, 0, c), egg_frame(size, -0.05, 0, c)],
        hatching: vec![
            egg_frame(size, 0.0, 1, c),
            egg_frame(size, 0.04, 2, c),
            egg_frame(size, -0.04, 3, c),
        ],
        eating: vec![
            blob(
                size,
                BlobStyle {
                    mouth: Mouth::Open,
                    crumb: true,
                    ..BlobStyle::default()
                },
                c,
            ),
            blob(
                size,
                BlobStyle {
                    mouth: Mouth::Closed,
                    crumb: true,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        sad: vec![
            blob(
                size,
                BlobStyle {
                    squash: 0.08,
                    eyes: Eyes::Droopy,
                    mouth: Mouth::Sad,
                    ..BlobStyle::default()
                },
                c,
            ),
            blob(
                size,
                BlobStyle {
                    squash: 0.11,
                    eyes: Eyes::Droopy,
                    mouth: Mouth::Sad,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        sick: vec![
            blob(
                size,
                BlobStyle {
                    eyes: Eyes::Droopy,
                    mouth: Mouth::Wavy,
                    tint: Some(SICK_TINT),
                    ..BlobStyle::default()
                },
                c,
            ),
            blob(
                size,
                BlobStyle {
                    squash: 0.04,
                    eyes: Eyes::Droopy,
                    mouth: Mouth::Wavy,
                    tint: Some(SICK_TINT),
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        happy: vec![
            blob(size, BlobStyle::default(), c),
            blob(
                size,
                BlobStyle {
                    hop: 0.06,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        // Фаза G: у процедурного блоба тоже есть свои варианты — иначе
        // фолбэк-питомец застывал бы столбом там, где арт-пак живёт.
        blink: vec![blob(
            size,
            BlobStyle {
                eyes: Eyes::Closed,
                ..BlobStyle::default()
            },
            c,
        )],
        sit: vec![blob(
            size,
            BlobStyle {
                squash: 0.18,
                ..BlobStyle::default()
            },
            c,
        )],
        stretch: vec![
            blob(
                size,
                BlobStyle {
                    squash: -0.14,
                    eyes: Eyes::Closed,
                    mouth: Mouth::Open,
                    ..BlobStyle::default()
                },
                c,
            ),
            blob(
                size,
                BlobStyle {
                    squash: -0.06,
                    eyes: Eyes::Closed,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        wiggle: vec![
            blob(
                size,
                BlobStyle {
                    hop: 0.04,
                    ..BlobStyle::default()
                },
                c,
            ),
            blob(
                size,
                BlobStyle {
                    squash: 0.06,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        cling: vec![blob(
            size,
            BlobStyle {
                squash: 0.1,
                ..BlobStyle::default()
            },
            c,
        )],
        climb: vec![
            blob_walk(size, 0.04, 0, c),
            blob_walk(size, 0.0, 2, c),
            blob_walk(size, 0.04, 1, c),
            blob_walk(size, 0.0, 3, c),
        ],
        dizzy: vec![
            blob(
                size,
                BlobStyle {
                    squash: 0.2,
                    eyes: Eyes::Closed,
                    mouth: Mouth::Wavy,
                    ..BlobStyle::default()
                },
                c,
            ),
            blob(
                size,
                BlobStyle {
                    squash: 0.16,
                    eyes: Eyes::Droopy,
                    mouth: Mouth::Wavy,
                    ..BlobStyle::default()
                },
                c,
            ),
        ],
        hang: Vec::new(),
        swing: Vec::new(),
        vomit: Vec::new(),
        wave: Vec::new(),
        dangle: Vec::new(),
        grip_inset: Inset::default(),
        feet_inset: Inset::default(),
    }
    .with_grip_inset()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Eyes {
    Open,
    Closed,
    Droopy,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mouth {
    None,
    Open,
    Closed,
    Sad,
    Wavy,
}

/// Параметры отрисовки блоба; собираются в семейства в `placeholder_for_stage`.
#[derive(Debug, Clone, Copy)]
struct BlobStyle {
    /// Прижатие тела: >0 — сплющен, <0 — вытянут.
    squash: f32,
    eyes: Eyes,
    /// «Z z» над головой у спящего.
    zzz: bool,
    /// Фаза шага при ходьбе (лапки в противофазе).
    walk_step: Option<u8>,
    mouth: Mouth,
    /// Крошка у рта (кадры еды).
    crumb: bool,
    /// Подмешать цвет к телу (болезнь).
    tint: Option<u32>,
    /// Подъём тела над землёй в долях размера (прыжок радости).
    hop: f32,
}

impl Default for BlobStyle {
    fn default() -> Self {
        Self {
            squash: 0.0,
            eyes: Eyes::Open,
            zzz: false,
            walk_step: None,
            mouth: Mouth::None,
            crumb: false,
            tint: None,
            hop: 0.0,
        }
    }
}

fn blob_walk(size: u32, squash: f32, step: u8, c: &BodyColors) -> Frame {
    blob(
        size,
        BlobStyle {
            squash,
            walk_step: Some(step),
            ..BlobStyle::default()
        },
        c,
    )
}

/// Рисуем эллипс-тело с прижатием `squash`, глаза, щёки и лапки.
fn blob(size: u32, st: BlobStyle, c: &BodyColors) -> Frame {
    let s = size as f32;
    let mut argb = vec![0u32; (size * size) as usize];

    let cx = s / 2.0;
    let rx = s * 0.38;
    let ry = s * 0.34 * (1.0 - st.squash);
    // Тело стоит на нижней кромке кадра (минус место под лапки);
    // hop поднимает всё тело над землёй.
    let foot_h = s * 0.06;
    let lift_all = st.hop * s;
    let cy = s - foot_h - ry - lift_all;

    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - cx) / rx;
            let dy = (y as f32 - cy) / ry;
            let d = dx * dx + dy * dy;
            if d <= 1.0 {
                let i = (y * size + x) as usize;
                argb[i] = if d > 0.82 { c.body_dark } else { c.body };
            }
        }
    }

    // Лапки: две полукруглые ножки; при ходьбе шагают в противофазе.
    let (lift_l, lift_r) = match st.walk_step {
        Some(0) => (foot_h * 0.9, 0.0),
        Some(2) => (0.0, foot_h * 0.9),
        _ => (0.0, 0.0),
    };
    for (fx, lift) in [(cx - rx * 0.45, lift_l), (cx + rx * 0.45, lift_r)] {
        fill_circle(
            &mut argb,
            size,
            fx,
            s - foot_h / 2.0 - lift - lift_all,
            foot_h * 0.9,
            c.body_dark,
        );
    }

    // Оттенок болезни: подмешивается к телу до лица, чтобы глаза остались чистыми.
    if let Some(tint) = st.tint {
        for px in argb.iter_mut() {
            if *px >> 24 != 0 {
                *px = mix(*px, tint, 0.35);
            }
        }
    }

    // Глаза.
    let ey = cy - ry * 0.15;
    for (side, ex) in [(-1i32, cx - rx * 0.38), (1, cx + rx * 0.38)] {
        match st.eyes {
            Eyes::Open => {
                fill_circle(&mut argb, size, ex, ey, s * 0.045, EYE);
                fill_circle(
                    &mut argb,
                    size,
                    ex + s * 0.012,
                    ey - s * 0.012,
                    s * 0.015,
                    EYE_SHINE,
                );
            }
            Eyes::Closed => {
                // Закрытый глаз — короткая дуга.
                for dx in -3i32..=3 {
                    put(&mut argb, size, (ex + dx as f32) as i32, ey as i32, EYE);
                }
            }
            Eyes::Droopy => {
                // Поникший глаз: линия со свисающим внешним краем.
                for dx in -3i32..=3 {
                    let droop = if dx * side > 0 { dx * side / 2 } else { 0 };
                    put(
                        &mut argb,
                        size,
                        (ex + dx as f32) as i32,
                        ey as i32 + droop,
                        EYE,
                    );
                }
            }
        }
    }

    // Щёки: розовые на светлом теле, осветлённый тон тела — на тёмном.
    for ex in [cx - rx * 0.6, cx + rx * 0.6] {
        fill_circle(&mut argb, size, ex, ey + ry * 0.35, s * 0.03, c.cheek);
    }

    // Рот.
    let my = ey + ry * 0.35;
    match st.mouth {
        Mouth::None => {}
        Mouth::Open => fill_circle(&mut argb, size, cx, my, s * 0.035, EYE),
        Mouth::Closed => {
            for dx in -3i32..=3 {
                put(&mut argb, size, cx as i32 + dx, my as i32, EYE);
            }
        }
        Mouth::Sad => {
            // Дуга уголками вниз.
            for dx in -4i32..=4 {
                put(
                    &mut argb,
                    size,
                    cx as i32 + dx,
                    my as i32 + dx * dx / 8,
                    EYE,
                );
            }
        }
        Mouth::Wavy => {
            // Волнистый рот — питомцу мутит.
            const WAVE: [i32; 4] = [0, 1, 0, -1];
            for dx in -4i32..=4 {
                let off = WAVE[((dx + 4) % 4) as usize];
                put(&mut argb, size, cx as i32 + dx, my as i32 + off, EYE);
            }
        }
    }

    // Крошка у рта (кадры еды).
    if st.crumb {
        fill_circle(
            &mut argb,
            size,
            cx + rx * 0.35,
            my + s * 0.04,
            s * 0.02,
            CRUMB,
        );
    }

    // «Z z» над головой у спящего.
    if st.zzz {
        let zx = (cx + rx * 0.7) as i32;
        let zy = (cy - ry - s * 0.08) as i32;
        draw_z(&mut argb, size, zx, zy, 5, EYE);
        draw_z(&mut argb, size, zx + 7, zy - 8, 3, EYE);
    }

    Frame {
        w: size,
        h: size,
        argb,
    }
}

/// Яйцо: собственная форма (не сжатый блоб) — скорлупа сужается кверху,
/// крапинки в цвет тела. `tilt` — покачивание сдвигом верхушки,
/// `cracks` 0..=3 — растущие трещины вылупления.
fn egg_frame(size: u32, tilt: f32, cracks: u8, c: &BodyColors) -> Frame {
    let s = size as f32;
    let mut argb = vec![0u32; (size * size) as usize];

    let cx0 = s / 2.0;
    let rx = s * 0.30;
    let ry = s * 0.38;
    let cy = s - ry - s * 0.04;

    for y in 0..size {
        let fy = y as f32;
        let vy = (fy - cy) / ry;
        if vy.abs() > 1.0 {
            continue;
        }
        // Верхняя половина уже нижней — силуэт яйца.
        let pinch = if vy < 0.0 { 1.0 + 0.22 * vy } else { 1.0 };
        // Покачивание: верх смещается сильнее низа.
        let cx = cx0 + tilt * (cy - fy);
        for x in 0..size {
            let vx = (x as f32 - cx) / (rx * pinch);
            let d = vx * vx + vy * vy;
            if d <= 1.0 {
                let i = (y * size + x) as usize;
                argb[i] = if d > 0.82 { c.shell_dark } else { c.shell };
            }
        }
    }

    // Крапинки: фиксированные позиции в координатах яйца (детерминизм).
    const SPECKLES: [(f32, f32); 5] = [
        (-0.35, -0.1),
        (0.25, -0.45),
        (0.1, 0.2),
        (-0.15, 0.45),
        (0.4, 0.15),
    ];
    for (ox, oy) in SPECKLES {
        let sy = cy + oy * ry;
        let sx = cx0 + ox * rx + tilt * (cy - sy);
        fill_circle_inside(&mut argb, size, sx, sy, s * 0.03, c.speckle);
    }

    // Трещины: ломаные от макушки вниз, растут с каждым кадром вылупления.
    let top = cy - ry * 0.9;
    if cracks >= 1 {
        crack_line(
            &mut argb,
            size,
            cx0,
            top,
            &[(2.0, 4.0), (-2.0, 4.0), (3.0, 4.0)],
            s,
            c.crack,
        );
    }
    if cracks >= 2 {
        crack_line(
            &mut argb,
            size,
            cx0 - rx * 0.4,
            top + ry * 0.25,
            &[(-2.0, 3.0), (2.0, 4.0), (-3.0, 4.0)],
            s,
            c.crack,
        );
    }
    if cracks >= 3 {
        crack_line(
            &mut argb,
            size,
            cx0 + rx * 0.45,
            top + ry * 0.35,
            &[(3.0, 3.0), (-2.0, 4.0), (2.0, 5.0), (-3.0, 4.0)],
            s,
            c.crack,
        );
    }

    Frame {
        w: size,
        h: size,
        argb,
    }
}

/// Ломаная трещина: шаги в долях 1/32 размера, рисуем только по скорлупе.
fn crack_line(argb: &mut [u32], size: u32, x0: f32, y0: f32, steps: &[(f32, f32)], s: f32, c: u32) {
    let k = s / 32.0;
    let (mut x, mut y) = (x0, y0);
    for (dx, dy) in steps {
        let (nx, ny) = (x + dx * k, y + dy * k);
        line_inside(argb, size, x, y, nx, ny, c);
        (x, y) = (nx, ny);
    }
}

/// Отрезок, рисуемый только поверх уже непрозрачных пикселей.
fn line_inside(argb: &mut [u32], size: u32, x0: f32, y0: f32, x1: f32, y1: f32, c: u32) {
    let n = ((x1 - x0).abs().max((y1 - y0).abs()).ceil() as i32).max(1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = (x0 + (x1 - x0) * t) as i32;
        let y = (y0 + (y1 - y0) * t) as i32;
        put_inside(argb, size, x, y, c);
    }
}

fn put(argb: &mut [u32], size: u32, x: i32, y: i32, c: u32) {
    if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
        argb[(y as u32 * size + x as u32) as usize] = c;
    }
}

/// Как `put`, но только поверх уже непрозрачных пикселей (не за силуэтом).
fn put_inside(argb: &mut [u32], size: u32, x: i32, y: i32, c: u32) {
    if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
        let i = (y as u32 * size + x as u32) as usize;
        if argb[i] >> 24 != 0 {
            argb[i] = c;
        }
    }
}

fn fill_circle(argb: &mut [u32], size: u32, cx: f32, cy: f32, r: f32, c: u32) {
    fill_circle_impl(argb, size, cx, cy, r, c, false);
}

/// Круг только поверх непрозрачных пикселей (крапинки скорлупы).
fn fill_circle_inside(argb: &mut [u32], size: u32, cx: f32, cy: f32, r: f32, c: u32) {
    fill_circle_impl(argb, size, cx, cy, r, c, true);
}

fn fill_circle_impl(argb: &mut [u32], size: u32, cx: f32, cy: f32, r: f32, c: u32, inside: bool) {
    let (x0, x1) = ((cx - r) as i32, (cx + r) as i32);
    let (y0, y1) = ((cy - r) as i32, (cy + r) as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= r * r {
                if inside {
                    put_inside(argb, size, x, y, c);
                } else {
                    put(argb, size, x, y, c);
                }
            }
        }
    }
}

/// Смешение непрозрачных ARGB-цветов: `k` — доля `b`.
fn mix(a: u32, b: u32, k: f32) -> u32 {
    let ch = |sh: u32| {
        let ca = ((a >> sh) & 0xff) as f32;
        let cb = ((b >> sh) & 0xff) as f32;
        ((ca + (cb - ca) * k) as u32) & 0xff
    };
    0xff00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Буква Z из трёх штрихов размера `n`.
fn draw_z(argb: &mut [u32], size: u32, x: i32, y: i32, n: i32, c: u32) {
    for i in 0..n {
        put(argb, size, x + i, y, c);
        put(argb, size, x + i, y + n - 1, c);
        put(argb, size, x + (n - 1 - i), y + i, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGES: [Stage; 5] = [
        Stage::Egg,
        Stage::Baby,
        Stage::Child,
        Stage::Teen,
        Stage::Adult,
    ];

    fn families(set: &SpriteSet) -> [(&'static str, &Vec<Frame>); 12] {
        [
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
        ]
    }

    #[test]
    fn frames_have_pixels() {
        let set = placeholder(96);
        for (name, frames) in families(&set) {
            assert!(!frames.is_empty(), "{name}: нет кадров");
            for f in frames.iter() {
                assert_eq!((f.w, f.h), (96, 96));
                assert!(
                    f.argb.iter().any(|&p| p >> 24 != 0),
                    "{name}: кадр не должен быть пустым"
                );
            }
        }
    }

    #[test]
    fn every_stage_has_nonempty_families() {
        for stage in STAGES {
            let set = placeholder_for_stage(96, stage);
            assert_eq!(
                set.size,
                ((96.0 * stage_scale(stage)).round() as u32).max(16)
            );
            for (name, frames) in families(&set) {
                assert!(!frames.is_empty(), "{stage:?}/{name}: нет кадров");
                for f in frames.iter() {
                    assert!(
                        f.argb.iter().any(|&p| p >> 24 != 0),
                        "{stage:?}/{name}: кадр не должен быть пустым"
                    );
                }
            }
        }
    }

    #[test]
    fn frame_selection_cycles() {
        let set = placeholder(64);
        let a = set.frame(PetState::Walk, 0.0, Direction::Right) as *const _;
        let b = set.frame(PetState::Walk, 0.5, Direction::Right) as *const _;
        assert_ne!(a, b);
    }

    #[test]
    fn mood_tier_thresholds() {
        let base = PetStats::default();
        assert_eq!(mood_tier(&base), MoodTier::Happy);
        assert_eq!(
            mood_tier(&PetStats {
                health: 39.0,
                ..base
            }),
            MoodTier::Sick
        );
        assert_eq!(mood_tier(&PetStats { mood: 29.0, ..base }), MoodTier::Sad);
        assert_eq!(
            mood_tier(&PetStats {
                satiety: 19.0,
                ..base
            }),
            MoodTier::Sad
        );
        assert_eq!(mood_tier(&PetStats { mood: 70.0, ..base }), MoodTier::Ok);
        assert_eq!(
            mood_tier(&PetStats {
                mood: 85.0,
                satiety: 61.0,
                ..base
            }),
            MoodTier::Happy
        );
    }

    fn in_family(frame: &Frame, family: &[Frame]) -> bool {
        family.iter().any(|f| core::ptr::eq(f, frame))
    }

    #[test]
    fn frame_look_selects_families() {
        let set = placeholder_for_stage(96, Stage::Egg);
        let egg_look = Look::simple(PetState::Idle, Stage::Egg);
        assert!(in_family(set.frame_look(&egg_look, 0.0), &set.egg));
        assert!(in_family(
            set.frame_look(
                &Look {
                    overlay: Some(ActionLook::Hatching),
                    ..egg_look
                },
                0.0
            ),
            &set.hatching
        ));

        let set = placeholder(96);
        let idle = Look::simple(PetState::Idle, Stage::Adult);
        assert!(in_family(set.frame_look(&idle, 0.0), &set.idle));
        assert!(in_family(
            set.frame_look(
                &Look {
                    mood: MoodTier::Sad,
                    ..idle
                },
                0.0
            ),
            &set.sad
        ));
        assert!(in_family(
            set.frame_look(
                &Look {
                    mood: MoodTier::Sick,
                    ..idle
                },
                0.0
            ),
            &set.sick
        ));
        assert!(in_family(
            set.frame_look(
                &Look {
                    mood: MoodTier::Happy,
                    ..idle
                },
                0.0
            ),
            &set.happy
        ));
        assert!(in_family(
            set.frame_look(
                &Look {
                    overlay: Some(ActionLook::Eating),
                    ..idle
                },
                0.0
            ),
            &set.eating
        ));
        // Настроение не ломает не-Idle состояния.
        assert!(in_family(
            set.frame_look(
                &Look {
                    state: PetState::Sleep,
                    mood: MoodTier::Sad,
                    ..idle
                },
                0.0
            ),
            &set.sleep
        ));
    }

    #[test]
    fn stage_scale_is_monotonic() {
        let scales: Vec<f32> = STAGES.iter().map(|s| stage_scale(*s)).collect();
        assert!(scales.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(stage_scale(Stage::Adult), 1.0);
    }

    fn greenness(f: &Frame) -> u64 {
        f.argb
            .iter()
            .filter(|&&p| p >> 24 != 0)
            .map(|&p| ((p >> 8) & 0xff) as u64)
            .sum::<u64>()
    }

    #[test]
    fn sick_frames_are_tinted() {
        let set = placeholder(96);
        // У больного тело зеленее здорового: сравним пиксели тел.
        assert!(greenness(&set.sick[0]) > greenness(&set.idle[0]));
    }

    // ---- Цвет тела (пользовательская настройка) ----

    /// Каждый пресет даёт непустые и попарно различные кадры — и для
    /// блоба, и для яйца (скорлупа тоже выводится из цвета).
    #[test]
    fn presets_paint_distinct_nonempty_frames() {
        let mut idles: Vec<Vec<u32>> = Vec::new();
        let mut eggs: Vec<Vec<u32>> = Vec::new();
        for &(argb, name) in palette::PET_PRESETS {
            let set = placeholder_colored(96, Stage::Adult, argb);
            for (fam, frames) in families(&set) {
                for f in frames.iter() {
                    assert!(
                        f.argb.iter().any(|&p| p >> 24 != 0),
                        "{name}/{fam}: кадр не должен быть пустым"
                    );
                }
            }
            idles.push(set.idle[0].argb.clone());
            eggs.push(set.egg[0].argb.clone());
        }
        for i in 0..idles.len() {
            for j in i + 1..idles.len() {
                assert_ne!(idles[i], idles[j], "тела пресетов {i} и {j} совпали");
                assert_ne!(eggs[i], eggs[j], "яйца пресетов {i} и {j} совпали");
            }
        }
    }

    /// Обёртки совместимости рисуют ровно дефолтный цвет.
    #[test]
    fn default_wrappers_use_default_color() {
        let a = placeholder_for_stage(96, Stage::Adult);
        let b = placeholder_colored(96, Stage::Adult, DEFAULT_PET_COLOR);
        assert_eq!(a.idle[0].argb, b.idle[0].argb);
        let egg_a = placeholder_for_stage(96, Stage::Egg);
        let egg_b = placeholder_colored(96, Stage::Egg, DEFAULT_PET_COLOR);
        assert_eq!(egg_a.egg[0].argb, egg_b.egg[0].argb);
    }

    /// Гард контраста щёк: на светлых телах — розовые, на тёмном —
    /// осветлённый тон самого тела. Кривая альфа входа нормализуется.
    #[test]
    fn cheeks_keep_contrast_on_dark_bodies() {
        for &(argb, name) in palette::PET_PRESETS {
            let c = BodyColors::from_argb(argb);
            assert_eq!(c.cheek, CHEEK_PINK, "{name}: светлое тело — розовые щёки");
        }
        let dark = 0xff_20_20_30;
        let c = BodyColors::from_argb(dark);
        assert_eq!(c.cheek, palette::lighten(dark, 1.3));
        assert_ne!(c.cheek, CHEEK_PINK);
        // Альфа входа не важна: тело всегда непрозрачное.
        assert_eq!(BodyColors::from_argb(0x00_20_20_30).body, dark);
    }

    /// Оттенок болезни подмешивается поверх любого цвета тела.
    #[test]
    fn sick_tint_applies_over_any_preset() {
        for &(argb, name) in palette::PET_PRESETS {
            let set = placeholder_colored(96, Stage::Adult, argb);
            assert!(
                greenness(&set.sick[0]) > greenness(&set.idle[0]),
                "{name}: больной должен зеленеть"
            );
        }
    }
}
