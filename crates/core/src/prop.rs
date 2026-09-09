//! Предметы мира питомца (фаза H1, ТЗ — `docs/WORLD.md`).
//!
//! Швабра, миска, лежанка, мяч, домик, велосипед и вражеский моб — это одна
//! сущность: у неё есть место в мире, вид, физика и способ взаимодействия.
//! Здесь живут данные и физика этой сущности; как она выглядит — решает
//! демон (процедурные кадры или арт-пак), что с ней делает питомец —
//! [`crate::task`].
//!
//! Ключевые решения:
//!
//! - предмет падает и лежит по той же модели, что и питомец
//!   ([`crate::body`], [`crate::physics`]) — отдельной физики нет;
//! - «твёрдый» предмет становится РЕЛЬЕФОМ: его верхняя кромка попадает в
//!   те же платформы, что и кромки окон, и питомец ходит по нему без единой
//!   новой строчки в физике опор;
//! - постоянные предметы живут в журнале событий (значит, синкаются между
//!   устройствами), временные — только в памяти демона.

use crate::behavior::BehaviorConfig;
use crate::geometry::{Rect, Vec2};
use crate::pet::World;
use crate::physics::{support_below, Platform, Surface};
use serde::{Deserialize, Serialize};

/// Что это за предмет. Поведение задаётся данными в [`PropKind::class`],
/// а не кодом на каждый случай.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropKind {
    /// Швабра: питомец достаёт её, чтобы убрать лужу (фаза H0).
    Mop,
    /// Миска: голодный сам идёт к ней есть.
    Bowl,
    /// Лежанка: спит в ней, а не на голом полу.
    Bed,
    /// Мячик: бросишь — догонит и принесёт.
    Ball,
    /// Домик: свой угол. В нём прячутся, спят и из него выходят здороваться.
    House,
    /// Скейт: самый простой транспорт, катится по полу.
    Skate,
    /// Велосипед: быстрее скейта, у края тормозит.
    Bike,
    /// Мопед: ещё быстрее, с дымком.
    Moped,
    /// Автомобиль: питомца видно в окне.
    Car,
    /// Вертолёт: свободный полёт с зависанием.
    Copter,
    /// Самолёт: полёт по дуге через весь экран.
    Plane,
}

/// Профиль транспорта (фаза H5): чем он отличается от остальных.
/// Управление питомцу не нужно — он катается сам; транспорт это зрелище,
/// а не способ быстрее двигаться по экрану.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vehicle {
    /// Скорость хода, м/с «в жизни» (переводится через [`crate::body`]).
    pub speed_mps: f32,
    /// Летает (иначе катится по полу).
    pub air: bool,
    /// Сколько катается, сек (от и до).
    pub ride_secs: (f32, f32),
    /// Где сидит питомец: доля высоты транспорта от его верхней кромки
    /// (0 — на самом верху, 1 — у земли).
    pub seat: f32,
    /// Амплитуда покачивания на ходу, доля высоты питомца.
    pub bob: f32,
    /// Питомца видно ЗА кузовом (автомобиль: он в окне).
    pub rider_behind: bool,
}

/// Неизменные свойства вида предмета.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropClass {
    /// К чему предмет привязан.
    pub anchor: Surface,
    /// Доля от размера питомца: ШИРИНА предмета.
    pub size_scale: f32,
    /// Высота относительно ширины (кадры предметов не квадратные).
    pub aspect: f32,
    /// Плотность относительно тела питомца (для массы и броска).
    pub density_scale: f32,
    /// Можно ли таскать мышью.
    pub draggable: bool,
    /// Верхняя кромка — платформа для питомца.
    pub solid: bool,
    /// Писать ли в журнал (переживает рестарт и синкается).
    pub persist: bool,
    /// Коэффициент восстановления при ударе (у мяча высокий).
    pub restitution: f32,
    /// Улетает ли вещь по скорости руки. Домик не улетает: его ставят,
    /// а не швыряют через весь экран.
    pub throwable: bool,
}

impl PropKind {
    pub fn class(self) -> PropClass {
        match self {
            PropKind::Mop => PropClass {
                anchor: Surface::Floor,
                size_scale: 0.95,
                aspect: 1.0,
                density_scale: 0.25,
                draggable: false,
                solid: false,
                persist: false,
                restitution: 0.1,
                throwable: true,
            },
            PropKind::Bowl => PropClass {
                anchor: Surface::Floor,
                size_scale: 0.62,
                aspect: 2.0 / 3.0,
                density_scale: 0.6,
                draggable: true,
                solid: false,
                persist: true,
                restitution: 0.15,
                throwable: true,
            },
            PropKind::Bed => PropClass {
                anchor: Surface::Floor,
                size_scale: 1.45,
                aspect: 0.4,
                density_scale: 0.35,
                draggable: true,
                // На лежанку можно забраться — она часть рельефа.
                solid: true,
                persist: true,
                restitution: 0.05,
                throwable: true,
            },
            PropKind::House => PropClass {
                anchor: Surface::Floor,
                size_scale: 2.1,
                aspect: 0.95,
                density_scale: 1.4,
                draggable: true,
                // Крыша — такая же опора, как кромка окна.
                solid: true,
                persist: true,
                restitution: 0.02,
                throwable: false,
            },
            // Транспорт: не хранится в журнале (это событие, а не быт),
            // мышью не таскается — питомец сам на него садится.
            PropKind::Skate
            | PropKind::Bike
            | PropKind::Moped
            | PropKind::Car
            | PropKind::Copter
            | PropKind::Plane => PropClass {
                anchor: Surface::Floor,
                size_scale: match self {
                    PropKind::Car => 1.9,
                    PropKind::Plane => 2.5,
                    PropKind::Copter => 2.0,
                    PropKind::Moped => 1.4,
                    PropKind::Bike => 1.3,
                    _ => 1.1,
                },
                aspect: match self {
                    PropKind::Skate => 0.28,
                    PropKind::Plane => 0.5,
                    PropKind::Copter => 0.62,
                    PropKind::Car => 0.55,
                    _ => 0.6,
                },
                density_scale: 1.2,
                draggable: false,
                solid: false,
                persist: false,
                restitution: 0.05,
                throwable: false,
            },
            PropKind::Ball => PropClass {
                anchor: Surface::Floor,
                size_scale: 0.38,
                aspect: 1.0,
                density_scale: 0.15,
                draggable: true,
                solid: false,
                // Мяч живёт, пока с ним играют: его не хранят в журнале.
                persist: false,
                restitution: 0.62,
                throwable: true,
            },
        }
    }

    /// Профиль транспорта; None — это не транспорт.
    pub fn vehicle(self) -> Option<Vehicle> {
        let v = match self {
            PropKind::Mop | PropKind::Bowl | PropKind::Bed | PropKind::Ball | PropKind::House => {
                return None
            }
            PropKind::Skate => Vehicle {
                speed_mps: 1.6,
                air: false,
                ride_secs: (14.0, 24.0),
                seat: 0.22,
                bob: 0.04,
                rider_behind: false,
            },
            PropKind::Bike => Vehicle {
                speed_mps: 2.2,
                air: false,
                ride_secs: (16.0, 28.0),
                seat: 0.36,
                bob: 0.05,
                rider_behind: false,
            },
            PropKind::Moped => Vehicle {
                speed_mps: 3.2,
                air: false,
                ride_secs: (16.0, 26.0),
                seat: 0.42,
                bob: 0.03,
                rider_behind: false,
            },
            PropKind::Car => Vehicle {
                speed_mps: 2.8,
                air: false,
                ride_secs: (18.0, 30.0),
                seat: 0.86,
                bob: 0.02,
                rider_behind: true,
            },
            PropKind::Copter => Vehicle {
                speed_mps: 2.4,
                air: true,
                ride_secs: (20.0, 34.0),
                seat: 0.62,
                bob: 0.06,
                rider_behind: false,
            },
            PropKind::Plane => Vehicle {
                speed_mps: 4.5,
                air: true,
                ride_secs: (14.0, 22.0),
                seat: 0.58,
                bob: 0.03,
                rider_behind: false,
            },
        };
        Some(v)
    }

    /// Все виды транспорта — по этому списку он и выбирается случайно.
    pub const VEHICLES: [PropKind; 6] = [
        PropKind::Skate,
        PropKind::Bike,
        PropKind::Moped,
        PropKind::Car,
        PropKind::Copter,
        PropKind::Plane,
    ];

    /// Машинное имя (журнал, логи, тесты).
    pub fn as_str(self) -> &'static str {
        match self {
            PropKind::Mop => "mop",
            PropKind::Bowl => "bowl",
            PropKind::Bed => "bed",
            PropKind::Ball => "ball",
            PropKind::House => "house",
            PropKind::Skate => "skate",
            PropKind::Bike => "bike",
            PropKind::Moped => "moped",
            PropKind::Car => "car",
            PropKind::Copter => "copter",
            PropKind::Plane => "plane",
        }
    }
}

/// Что сейчас с предметом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropState {
    /// Лежит на своей опоре.
    Rest,
    /// Летит (уронили, бросили).
    Falling,
    /// В руке человека.
    Held,
    /// В лапках питомца (швабра во время уборки).
    Carried,
    /// На нём едут: позицию ведёт поездка, своей физики нет (H5).
    Ridden,
}

/// Предмет в мире.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    /// Идентификатор: у постоянных — из журнальной метки, у временных —
    /// счётчик демона.
    pub id: u64,
    pub kind: PropKind,
    /// Опорная точка: центр нижней кромки (как у питомца на полу).
    pub pos: Vec2,
    pub vel: Vec2,
    pub state: PropState,
    /// Ширина спрайта в пикселях (высота — через `class().aspect`).
    pub size: f32,
}

impl Prop {
    /// Новый предмет размера под питомца `pet_size`, лежащий в точке `pos`.
    pub fn new(id: u64, kind: PropKind, pos: Vec2, pet_size: f32) -> Self {
        Self {
            id,
            kind,
            pos,
            vel: Vec2::default(),
            state: PropState::Rest,
            size: (pet_size * kind.class().size_scale).max(6.0),
        }
    }

    /// Высота предмета в пикселях.
    pub fn height(&self) -> f32 {
        self.size * self.kind.class().aspect
    }

    /// Прямоугольник предмета (рендер, хит-область, платформа).
    pub fn bounds(&self) -> Rect {
        let h = self.height();
        Rect::new(self.pos.x - self.size / 2.0, self.pos.y - h, self.size, h)
    }

    /// Платформа для физики питомца — только у «твёрдых» предметов.
    pub fn platform(&self) -> Option<Platform> {
        self.kind.class().solid.then(|| Platform {
            rect: self.bounds(),
            id: self.id,
        })
    }

    /// Шаг физики: падение с сопротивлением, приземление на опору (пол,
    /// кромка окна или другой твёрдый предмет), отскок и затухание.
    /// В руке (`Held`/`Carried`) физика выключена — позицию ведёт хозяин.
    pub fn tick(&mut self, world: &World, cfg: &BehaviorConfig, dt: f32) {
        if matches!(
            self.state,
            PropState::Held | PropState::Carried | PropState::Ridden
        ) {
            return;
        }
        let class = self.kind.class();
        let mut left = dt.clamp(0.0, 1.5);
        while left > 0.0 {
            let step = left.min(0.05);
            left -= step;
            let feet_before = self.pos.y;
            if self.state == PropState::Falling {
                let speed = self.vel.x.hypot(self.vel.y);
                if speed > 0.0 {
                    // Лёгкий предмет тормозит воздухом заметнее тяжёлого.
                    let k = cfg.body.drag_k_px() / class.density_scale.max(0.05);
                    self.vel = self.vel * (1.0 - (k * speed * step).min(1.0));
                }
                self.vel.y += cfg.gravity() * step;
            }
            self.pos = self.pos + self.vel * step;
            self.clamp_to_screen(world);

            let support = support_below(world, self.pos.x, feet_before);
            if self.pos.y >= support {
                self.pos.y = support;
                let impact = self.vel.y;
                let bounce = impact * class.restitution;
                if bounce > cfg.body.mps_to_px(0.5) {
                    // Отскок: мяч прыгает, миска почти нет.
                    self.vel.y = -bounce;
                    self.vel.x *= 0.85;
                    self.state = PropState::Falling;
                } else {
                    self.vel.y = 0.0;
                    // Трение о пол гасит горизонтальный ход.
                    self.vel.x *= 1.0 - (cfg.body.roll_friction * step).min(1.0);
                    if self.vel.x.abs() < cfg.body.mps_to_px(0.1) {
                        self.vel.x = 0.0;
                        self.state = PropState::Rest;
                    }
                }
            } else if self.state != PropState::Falling {
                // Опору увезли (закрыли окно) — предмет падает.
                self.state = PropState::Falling;
            }
        }
    }

    /// Отпустить предмет из руки со скоростью броска.
    pub fn release(&mut self, vel: Vec2) {
        self.vel = vel;
        self.state = PropState::Falling;
    }

    /// Удержать предмет в точке (рука человека или лапки питомца).
    pub fn hold(&mut self, pos: Vec2, by_pet: bool) {
        self.pos = pos;
        self.vel = Vec2::default();
        self.state = if by_pet {
            PropState::Carried
        } else {
            PropState::Held
        };
    }

    fn clamp_to_screen(&mut self, world: &World) {
        let half = self.size / 2.0;
        let (lo, hi) = (
            world.screen.x + half,
            (world.screen.right() - half).max(world.screen.x + half),
        );
        if self.pos.x < lo || self.pos.x > hi {
            self.pos.x = self.pos.x.clamp(lo, hi);
            self.vel.x = -self.vel.x * 0.4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn world() -> World {
        World::new(Rect::new(0.0, 0.0, 1920.0, 1080.0))
    }

    fn cfg() -> BehaviorConfig {
        BehaviorConfig::default()
    }

    /// Брошенный предмет падает, приземляется на пол и успокаивается.
    #[test]
    fn dropped_prop_falls_and_settles() {
        let w = world();
        let c = cfg();
        let mut p = Prop::new(1, PropKind::Bowl, Vec2::new(500.0, 300.0), 96.0);
        p.release(Vec2::new(120.0, 0.0));
        for _ in 0..600 {
            p.tick(&w, &c, 1.0 / 120.0);
        }
        assert_eq!(p.state, PropState::Rest);
        assert_eq!(p.pos.y, w.ground_y(), "лежит на полу");
        assert!(p.pos.x > 500.0, "улетел по броску вперёд");
        assert_eq!(p.vel.x, 0.0, "трение остановило");
    }

    /// Мяч прыгает заметно выше миски: коэффициент восстановления работает.
    #[test]
    fn ball_bounces_higher_than_a_bowl() {
        let w = world();
        let c = cfg();
        let mut peak = Vec::new();
        for kind in [PropKind::Ball, PropKind::Bowl] {
            let mut p = Prop::new(1, kind, Vec2::new(500.0, 200.0), 96.0);
            p.release(Vec2::default());
            let mut touched = false;
            let mut top = w.ground_y();
            for _ in 0..900 {
                p.tick(&w, &c, 1.0 / 120.0);
                touched |= p.pos.y >= w.ground_y() - 0.5;
                if touched {
                    top = top.min(p.pos.y);
                }
            }
            peak.push(w.ground_y() - top);
        }
        assert!(
            peak[0] > peak[1] * 3.0,
            "мяч должен скакать: {:?} против {:?}",
            peak[0],
            peak[1]
        );
    }

    /// Твёрдый предмет — это рельеф: его кромка становится платформой,
    /// на которой питомец находит опору.
    #[test]
    fn solid_prop_is_terrain() {
        let mut w = world();
        let bed = Prop::new(7, PropKind::Bed, Vec2::new(800.0, 1080.0), 96.0);
        let plat = bed.platform().expect("лежанка твёрдая");
        assert_eq!(plat.id, 7);
        w.platforms.push(plat);
        // Над лежанкой опора — её верх, а не пол.
        let top = bed.bounds().y;
        assert_eq!(support_below(&w, 800.0, 500.0), top);
        // Мяч рельефом не становится.
        let ball = Prop::new(8, PropKind::Ball, Vec2::new(400.0, 1080.0), 96.0);
        assert!(ball.platform().is_none());
    }

    /// В руке физика выключена, после отпускания — включается.
    #[test]
    fn held_prop_ignores_gravity() {
        let w = world();
        let c = cfg();
        let mut p = Prop::new(1, PropKind::Ball, Vec2::new(500.0, 300.0), 96.0);
        p.hold(Vec2::new(600.0, 200.0), false);
        for _ in 0..120 {
            p.tick(&w, &c, 1.0 / 60.0);
        }
        assert_eq!(p.pos, Vec2::new(600.0, 200.0), "висит в руке");
        p.release(Vec2::new(0.0, 0.0));
        for _ in 0..600 {
            p.tick(&w, &c, 1.0 / 120.0);
        }
        assert_eq!(p.pos.y, w.ground_y());
    }

    /// Предмет не улетает за экран: у края отскакивает внутрь.
    #[test]
    fn prop_stays_on_screen() {
        let w = world();
        let c = cfg();
        let mut p = Prop::new(1, PropKind::Ball, Vec2::new(1800.0, 400.0), 96.0);
        p.release(Vec2::new(2500.0, 0.0));
        for _ in 0..600 {
            p.tick(&w, &c, 1.0 / 120.0);
            let b = p.bounds();
            assert!(
                b.x >= w.screen.x - 0.01 && b.right() <= w.screen.right() + 0.01,
                "предмет вылез за экран: {b:?}"
            );
        }
    }
}
