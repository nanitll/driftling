//! Питомец и мир, в котором он живёт. Точка входа симуляции — `Pet::tick`.

use crate::behavior::{next_state_after, BehaviorConfig, PetState};
use crate::geometry::{Rect, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
}

impl Direction {
    pub fn sign(self) -> f32 {
        match self {
            Direction::Left => -1.0,
            Direction::Right => 1.0,
        }
    }

    pub fn flip(self) -> Self {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }
}

/// Мир M0: один экран, земля — нижняя кромка.
/// В M1 сюда добавятся платформы из worldsense (кромки окон).
#[derive(Debug, Clone)]
pub struct World {
    pub screen: Rect,
}

impl World {
    pub fn ground_y(&self) -> f32 {
        self.screen.bottom()
    }
}

/// События указателя, которые платформа передаёт в симуляцию.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEvent {
    /// Нажатие по питомцу (позиция курсора).
    Press(Vec2),
    /// Движение при зажатой кнопке.
    Motion(Vec2),
    /// Отпускание кнопки.
    Release(Vec2),
}

#[derive(Debug, Clone)]
pub struct Pet {
    /// Позиция опорной точки: центр нижней кромки спрайта.
    pub pos: Vec2,
    pub vel: Vec2,
    pub facing: Direction,
    pub state: PetState,
    /// Сколько питомец уже находится в текущем состоянии, сек.
    pub state_time: f32,
    /// Сколько осталось до следующего решения машины поведения, сек.
    pub state_left: f32,
    /// Размер спрайта в логических пикселях (квадрат).
    pub size: f32,
    cfg: BehaviorConfig,
    rng: fastrand::Rng,
    /// Смещение точки хвата при перетаскивании.
    drag_offset: Vec2,
    /// Недавние позиции курсора для скорости броска.
    drag_history: [(f32, Vec2); 4],
}

impl Pet {
    pub fn new(pos: Vec2, size: f32, cfg: BehaviorConfig, seed: u64) -> Self {
        let mut rng = fastrand::Rng::with_seed(seed);
        let state_left = 0.5 + rng.f32();
        Self {
            pos,
            vel: Vec2::default(),
            facing: Direction::Right,
            state: PetState::Falling,
            state_time: 0.0,
            state_left,
            size,
            cfg,
            rng,
            drag_offset: Vec2::default(),
            drag_history: [(0.0, Vec2::default()); 4],
        }
    }

    pub fn config(&self) -> &BehaviorConfig {
        &self.cfg
    }

    /// Применить новые настройки к живому питомцу: поведение и размер
    /// меняются, состояние (позиция, текущее занятие) сохраняется.
    pub fn apply_config(&mut self, cfg: BehaviorConfig, size: f32) {
        self.cfg = cfg;
        self.size = size;
    }

    /// Прямоугольник спрайта (для рендера и input region).
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.pos.x - self.size / 2.0,
            self.pos.y - self.size,
            self.size,
            self.size,
        )
    }

    /// Один шаг симуляции. `dt` — секунды с прошлого шага (клампится).
    pub fn tick(&mut self, world: &World, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        self.state_time += dt;

        match self.state {
            PetState::Dragged => {
                // Позицию ведёт указатель (см. pointer()); физика выключена.
            }
            PetState::Falling => {
                self.vel.y += self.cfg.gravity * dt;
                self.pos = self.pos + self.vel * dt;
                self.clamp_horizontal(world);
                if self.pos.y >= world.ground_y() {
                    self.pos.y = world.ground_y();
                    let impact = self.vel.y;
                    self.vel = Vec2::default();
                    self.enter(if impact > self.cfg.hard_landing_speed {
                        PetState::Landing
                    } else {
                        PetState::Idle
                    });
                    self.state_left = if self.state == PetState::Landing {
                        self.cfg.landing_time
                    } else {
                        1.0 + self.rng.f32()
                    };
                }
            }
            PetState::Walk => {
                self.pos.x += self.cfg.walk_speed * self.facing.sign() * dt;
                let b = self.bounds();
                if b.x <= world.screen.x {
                    self.pos.x = world.screen.x + self.size / 2.0;
                    self.facing = Direction::Right;
                } else if b.right() >= world.screen.right() {
                    self.pos.x = world.screen.right() - self.size / 2.0;
                    self.facing = Direction::Left;
                }
                self.advance_timer();
            }
            PetState::Idle | PetState::Sleep | PetState::Landing => {
                self.advance_timer();
            }
        }
    }

    /// Обработка событий указателя. Возвращает true, если событие потреблено.
    pub fn pointer(&mut self, world: &World, ev: PointerEvent, now: f32) -> bool {
        match ev {
            PointerEvent::Press(p) => {
                if !self.bounds().contains(p) {
                    return false;
                }
                self.drag_offset = Vec2::new(self.pos.x - p.x, self.pos.y - p.y);
                self.drag_history = [(now, p); 4];
                self.enter(PetState::Dragged);
                true
            }
            PointerEvent::Motion(p) => {
                if self.state != PetState::Dragged {
                    return false;
                }
                self.drag_history.rotate_left(1);
                self.drag_history[3] = (now, p);
                self.pos = p + self.drag_offset;
                // Не даём утащить за экран.
                self.pos.x = self.pos.x.clamp(world.screen.x, world.screen.right());
                self.pos.y = self.pos.y.clamp(world.screen.y, world.ground_y());
                true
            }
            PointerEvent::Release(_) => {
                if self.state != PetState::Dragged {
                    return false;
                }
                // Скорость броска — по финальному «флику»: берём самую раннюю
                // точку истории не старше ~120 мс, чтобы медленное таскание
                // до рывка не гасило скорость.
                let (t1, p1) = self.drag_history[3];
                let (t0, p0) = self
                    .drag_history
                    .iter()
                    .copied()
                    .find(|(t, _)| t1 - t <= 0.12)
                    .unwrap_or(self.drag_history[2]);
                let span = (t1 - t0).max(1e-3);
                self.vel = Vec2::new((p1.x - p0.x) / span, (p1.y - p0.y) / span);
                self.enter(PetState::Falling);
                true
            }
        }
    }

    fn advance_timer(&mut self) {
        if self.state_time >= self.state_left {
            let (next, dur) = next_state_after(self.state, &self.cfg, &mut self.rng);
            if next == PetState::Walk && self.rng.bool() {
                self.facing = self.facing.flip();
            }
            self.enter(next);
            self.state_left = dur;
        }
    }

    fn enter(&mut self, next: PetState) {
        if self.state != next {
            self.state = next;
            self.state_time = 0.0;
        }
    }

    fn clamp_horizontal(&mut self, world: &World) {
        self.pos.x = self.pos.x.clamp(
            world.screen.x + self.size / 2.0,
            world.screen.right() - self.size / 2.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        World {
            screen: Rect::new(0.0, 0.0, 1920.0, 1080.0),
        }
    }

    fn pet() -> Pet {
        Pet::new(Vec2::new(960.0, 100.0), 96.0, BehaviorConfig::default(), 42)
    }

    #[test]
    fn falls_and_lands_on_ground() {
        let w = world();
        let mut p = pet();
        assert_eq!(p.state, PetState::Falling);
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, w.ground_y());
        assert!(matches!(
            p.state,
            PetState::Idle | PetState::Walk | PetState::Sleep
        ));
    }

    #[test]
    fn walk_turns_at_screen_edge() {
        let w = world();
        let mut p = pet();
        // Приземлить и заставить идти влево от левого края.
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        p.pos.x = 50.0;
        p.facing = Direction::Left;
        p.state = PetState::Walk;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        for _ in 0..300 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.facing, Direction::Right);
        assert!(p.bounds().x >= 0.0);
    }

    #[test]
    fn drag_and_throw() {
        let w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let grab = Vec2::new(p.pos.x, p.pos.y - 10.0);
        assert!(p.pointer(&w, PointerEvent::Press(grab), 0.0));
        assert_eq!(p.state, PetState::Dragged);
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(500.0, 300.0)), 0.1));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(700.0, 280.0)), 0.2));
        assert!(p.pointer(&w, PointerEvent::Release(Vec2::new(700.0, 280.0)), 0.2));
        assert_eq!(p.state, PetState::Falling);
        assert!(p.vel.x > 0.0, "бросок вправо должен дать положительную vx");
        // Падение после броска завершается на земле.
        for _ in 0..1200 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, w.ground_y());
    }

    #[test]
    fn press_outside_is_ignored() {
        let w = world();
        let mut p = pet();
        assert!(!p.pointer(&w, PointerEvent::Press(Vec2::new(5.0, 5.0)), 0.0));
    }
}
