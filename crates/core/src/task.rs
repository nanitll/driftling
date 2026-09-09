//! Дела питомца (фаза H1, ТЗ — `docs/WORLD.md`).
//!
//! Дело — это «дойти до места и поработать там»: убрать лужу шваброй,
//! поесть у миски, дойти до лежанки и лечь, догнать мяч. Всё, что питомец
//! делает сам по своей воле, выражается этой парой шагов; демон только
//! решает, какое дело начать, и что случится в конце.
//!
//! Почему так:
//!
//! - **Один код ходьбы на все занятия.** Первое дело (уборка, фаза H0)
//!   было написано прямо в демоне и уже начало обрастать копиями; здесь
//!   логика живёт один раз и покрыта тестами без Wayland.
//! - **Дело всегда прерываемо.** Человек трогает питомца — дело бросается
//!   на любом шаге, и это ответственность вызывающего: [`Errand::step`]
//!   ничего не знает про мышь.
//! - **Дела не пишут в журнал.** История ухода — про человека; то, что
//!   питомец сходил к миске, в ней не нужно.

use crate::behavior::PetState;
use crate::pet::{Direction, Pet};

/// На сколько пикселей можно промахнуться мимо цели — дальше питомец
/// топтался бы на месте, дёргаясь влево-вправо на дробных шагах.
pub const ARRIVE_EPS: f32 = 6.0;

/// Чем именно занят питомец.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrandKind {
    /// Вытереть лужу шваброй (H0).
    Mop,
    /// Поесть у миски.
    Eat,
    /// Дойти до лежанки и лечь спать.
    Nap,
    /// Догнать мяч.
    Fetch,
    /// Отнести предмет на место.
    Carry,
    /// Зайти внутрь (домик).
    Enter,
}

/// Ход дела на текущем тике.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    /// Идёт к месту.
    Going,
    /// Работает; `done` — доля выполненного, 0..1.
    Working { done: f32 },
    /// Только что закончил (возвращается один раз).
    Finished,
}

/// Дело: куда идти, сколько там работать и над каким предметом.
#[derive(Debug, Clone, PartialEq)]
pub struct Errand {
    pub kind: ErrandKind,
    /// Предмет, ради которого всё затевалось (миска, лужа, мяч).
    pub prop: Option<u64>,
    /// Куда идти по горизонтали.
    pub target_x: f32,
    /// Сколько работать по приходе, сек (0 — закончить сразу).
    pub work_secs: f32,
    /// Момент начала работы (время приложения); None — ещё в пути.
    pub work_since: Option<f64>,
}

impl Errand {
    pub fn new(kind: ErrandKind, target_x: f32, work_secs: f32) -> Self {
        Self {
            kind,
            prop: None,
            target_x,
            work_secs,
            work_since: None,
        }
    }

    /// Привязать дело к предмету.
    pub fn about(mut self, prop: u64) -> Self {
        self.prop = Some(prop);
        self
    }

    /// Перенацелить дело (вторая нога маршрута: догнал мяч — понёс назад).
    pub fn retarget(&mut self, kind: ErrandKind, target_x: f32, work_secs: f32) {
        self.kind = kind;
        self.target_x = target_x;
        self.work_secs = work_secs;
        self.work_since = None;
    }

    /// Идёт ли питомец к месту прямо сейчас.
    pub fn walking(&self) -> bool {
        self.work_since.is_none()
    }

    /// Шаг дела. Питомца ведём сами (как в пробежках присутствия): его
    /// собственный автомат покоя на время дела не у руля, иначе он бы
    /// разворачивался по своим таймерам посреди дороги.
    pub fn step(&mut self, pet: &mut Pet, speed: f32, now: f64, dt: f32) -> Progress {
        match self.work_since {
            None => {
                let dx = self.target_x - pet.pos.x;
                if dx.abs() <= ARRIVE_EPS {
                    pet.pos.x = self.target_x;
                    pet.state = PetState::Idle;
                    pet.state_time = 0.0;
                    pet.state_left = self.work_secs.max(0.0);
                    self.work_since = Some(now);
                    if self.work_secs <= 0.0 {
                        return Progress::Finished;
                    }
                    return Progress::Working { done: 0.0 };
                }
                let dir = dx.signum();
                pet.facing = if dir < 0.0 {
                    Direction::Left
                } else {
                    Direction::Right
                };
                pet.state = PetState::Walk;
                pet.state_time += dt;
                pet.state_left = f32::INFINITY;
                // Не перешагиваем цель: иначе на редких тиках (Drowsy) шаг
                // в 90 px перелетал бы миску и питомец ходил бы вокруг неё.
                pet.pos.x += (dir * speed * dt).clamp(-dx.abs(), dx.abs());
                Progress::Going
            }
            Some(since) => {
                pet.state = PetState::Idle;
                pet.state_left = self.work_secs;
                let done = if self.work_secs > 0.0 {
                    ((now - since) as f32 / self.work_secs).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                if done >= 1.0 {
                    Progress::Finished
                } else {
                    Progress::Working { done }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::BehaviorConfig;
    use crate::geometry::Rect;
    use crate::pet::World;

    fn pet_at(x: f32) -> Pet {
        let world = World::new(Rect::new(0.0, 0.0, 800.0, 600.0));
        let mut pet = Pet::new(
            crate::geometry::Vec2::new(x, world.ground_y()),
            48.0,
            BehaviorConfig::default(),
            7,
        );
        pet.pos.x = x;
        pet.pos.y = world.ground_y();
        pet
    }

    #[test]
    fn errand_walks_then_works_then_finishes() {
        let mut pet = pet_at(100.0);
        let mut errand = Errand::new(ErrandKind::Mop, 300.0, 1.0);
        let mut now = 0.0;
        let mut progress = Progress::Going;
        for _ in 0..200 {
            now += 0.05;
            progress = errand.step(&mut pet, 90.0, now, 0.05);
            if progress == Progress::Finished {
                break;
            }
        }
        assert_eq!(progress, Progress::Finished);
        assert!((pet.pos.x - 300.0).abs() <= ARRIVE_EPS);
    }

    #[test]
    fn errand_faces_the_way_it_walks() {
        let mut pet = pet_at(400.0);
        let mut errand = Errand::new(ErrandKind::Eat, 100.0, 0.5);
        errand.step(&mut pet, 90.0, 0.1, 0.1);
        assert_eq!(pet.facing, Direction::Left);
        assert_eq!(pet.state, PetState::Walk);
    }

    #[test]
    fn errand_does_not_overshoot_on_a_long_tick() {
        let mut pet = pet_at(0.0);
        let mut errand = Errand::new(ErrandKind::Fetch, 50.0, 0.0);
        // Один «сонный» тик в секунду при скорости 90 px/с перелетел бы цель.
        let p = errand.step(&mut pet, 90.0, 1.0, 1.0);
        assert_eq!(p, Progress::Going);
        assert!(pet.pos.x <= 50.0, "перелетел цель: {}", pet.pos.x);
    }

    #[test]
    fn zero_work_errand_finishes_on_arrival() {
        let mut pet = pet_at(60.0);
        let mut errand = Errand::new(ErrandKind::Fetch, 62.0, 0.0);
        assert_eq!(errand.step(&mut pet, 90.0, 0.1, 0.1), Progress::Finished);
    }

    #[test]
    fn retarget_starts_the_second_leg() {
        let mut pet = pet_at(60.0);
        let mut errand = Errand::new(ErrandKind::Fetch, 60.0, 0.0);
        assert_eq!(errand.step(&mut pet, 90.0, 0.1, 0.1), Progress::Finished);
        errand.retarget(ErrandKind::Carry, 200.0, 0.4);
        assert!(errand.walking());
        assert_eq!(errand.step(&mut pet, 90.0, 0.2, 0.1), Progress::Going);
    }
}
