//! Стейт-машина поведения. M0: Idle/Walk/Sleep/Falling/Dragged;
//! фаза G добавляет лазание (Climb) и удар о потолок (Bonk), а также
//! мелкие занятия внутри Idle ([`IdleAction`]).
//! Переходы — по таймерам с весами (все константы в BehaviorConfig, ТЗ §3.2).

use crate::body::Body;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PetState {
    Idle,
    Walk,
    Sleep,
    Falling,
    Dragged,
    /// Короткое оглушение после приземления с большой высоты.
    Landing,
    /// Движение по стене или потолку (фаза G).
    Climb,
    /// Удар головой о потолок: короткое оглушение в воздухе, дальше падение.
    Bonk,
    /// Катится по полу после броска: инерция гасится трением, тело
    /// кувыркается (фаза G3).
    Roll,
}

/// Мелкое занятие в состоянии Idle (фаза G): питомец не «стоит столбом»,
/// а моргает, садится, потягивается и шевелит антенной. На физику не влияет —
/// только на выбор кадра ([`crate::sprite::SpriteSet::frame_look`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdleAction {
    /// Обычная стойка: базовое семейство idle.
    #[default]
    Stand,
    /// Моргнул (доли секунды).
    Blink,
    /// Сел, поджав лапки.
    Sit,
    /// Потянулся (с зевком).
    Stretch,
    /// Повёл антенной / потоптался.
    Wiggle,
}

#[derive(Debug, Clone)]
pub struct BehaviorConfig {
    /// Скорость ходьбы, px/s.
    pub walk_speed: f32,
    /// Физика тела (фаза G3): масштаб мира, масса, сопротивление, отскок.
    /// Гравитация, предельная скорость падения и предел броска НЕ задаются
    /// числами — они выводятся отсюда (см. [`crate::body::Body`]).
    pub body: Body,
    /// Диапазон длительности Idle до следующего решения, сек.
    pub idle_range: (f32, f32),
    /// Диапазон длительности Walk, сек.
    pub walk_range: (f32, f32),
    /// Диапазон длительности Sleep, сек.
    pub sleep_range: (f32, f32),
    /// Длительность Landing, сек.
    pub landing_time: f32,
    /// Вес перехода Idle -> Walk (из 100).
    pub w_idle_to_walk: u32,
    /// Вес перехода Idle -> Sleep (из 100).
    pub w_idle_to_sleep: u32,
    /// Скорость падения, с которой приземление считается «жёстким», м/с.
    /// В пиксели переводится масштабом тела — порог физический, а не
    /// экранный: на большом мониторе он не должен меняться.
    pub hard_landing_mps: f32,

    // ---- Фаза G: лазание, воздух, мелкая жизнь ----
    /// Доля скорости ходьбы при лазании по стене/потолку.
    pub climb_speed_scale: f32,
    /// Шанс (%) полезть на стену вместо разворота у края экрана.
    pub w_wall_climb: u32,
    /// Шанс (%), что очередная прогулка окажется «походом к стене»:
    /// питомец идёт до края экрана не сворачивая и лезет наверх.
    /// Без этого лазание видно только по случайности — до края экрана
    /// спокойный питомец добирается редко.
    pub w_wall_trip: u32,
    /// Вес продолжить лазание после паузы на стене/потолке (из 100).
    pub w_cling_to_climb: u32,
    /// Вес спрыгнуть со стены/потолка по своей воле (из 100).
    pub w_cling_to_drop: u32,
    /// Диапазон длительности одного «пролёта» по стене/потолку, сек.
    pub climb_range: (f32, f32),
    /// Частота перехвата лапами при лазании, Гц: скорость пульсирует в
    /// такт хвату, а не тянется ровной линейкой.
    pub climb_pull_hz: f32,
    /// Глубина пульсации (0 — равномерно, 1 — до полной остановки между
    /// перехватами). Средняя скорость не меняется.
    pub climb_pull_depth: f32,
    /// Скорость удара о потолок, выше которой питомец не цепляется, а
    /// набивает шишку (Bonk), м/с.
    pub ceiling_grab_mps: f32,
    /// Горизонтальная скорость влёта в стену, с которой питомец за неё
    /// цепляется в падении, м/с.
    pub wall_grab_mps: f32,
    /// Длительность Bonk, сек.
    pub bonk_time: f32,

    /// Диапазон паузы между мелкими занятиями в Idle, сек.
    pub fidget_range: (f32, f32),
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            walk_speed: 38.0,
            body: Body::default(),
            // Фаза G: питомец должен быть спокойным соседом, а не бегать
            // без остановки — паузы длиннее прогулок.
            idle_range: (4.0, 12.0),
            walk_range: (1.5, 4.5),
            sleep_range: (12.0, 30.0),
            landing_time: 0.8,
            w_idle_to_walk: 32,
            w_idle_to_sleep: 14,
            hard_landing_mps: 2.6,

            climb_speed_scale: 0.65,
            // Баланс замерен прогоном часа жизни (см. `g_stats` в pet.rs):
            // ~20 вылазок на стены в час, 15% времени вне пола.
            w_wall_climb: 30,
            w_wall_trip: 8,
            w_cling_to_climb: 60,
            w_cling_to_drop: 12,
            climb_range: (2.0, 7.0),
            // Четыре кадра лазания при 5 fps — цикл 0.8 с, два перехвата
            // за цикл: 2.5 Гц.
            climb_pull_hz: 2.5,
            climb_pull_depth: 0.55,
            // Бросок вверх — это приглашение повисеть: цепляется почти
            // всегда, шишка достаётся только совсем зверскому запуску.
            ceiling_grab_mps: 5.0,
            wall_grab_mps: 0.3,
            bonk_time: 0.5,
            fidget_range: (1.5, 5.0),
        }
    }
}

impl BehaviorConfig {
    /// Скорость движения по стене/потолку, px/s.
    pub fn climb_speed(&self) -> f32 {
        self.walk_speed * self.climb_speed_scale
    }

    /// Гравитация в пикселях (из физики тела).
    pub fn gravity(&self) -> f32 {
        self.body.gravity_px()
    }

    /// Скорость удара о пол, выше которой приземление «жёсткое», px/s.
    pub fn hard_landing_speed(&self) -> f32 {
        self.body.mps_to_px(self.hard_landing_mps)
    }

    /// Порог цепляния за потолок, px/s.
    pub fn ceiling_grab_speed(&self) -> f32 {
        self.body.mps_to_px(self.ceiling_grab_mps)
    }

    /// Порог цепляния за стену в полёте, px/s.
    pub fn wall_grab_speed(&self) -> f32 {
        self.body.mps_to_px(self.wall_grab_mps)
    }
}

/// Решение, принимаемое машиной по истечении таймера состояния.
/// Выделено в чистую функцию для тестируемости: RNG передаётся снаружи.
pub fn next_state_after(
    state: PetState,
    cfg: &BehaviorConfig,
    rng: &mut fastrand::Rng,
) -> (PetState, f32) {
    match state {
        PetState::Idle => {
            let roll = rng.u32(0..100);
            if roll < cfg.w_idle_to_walk {
                (PetState::Walk, rng_range(rng, cfg.walk_range))
            } else if roll < cfg.w_idle_to_walk + cfg.w_idle_to_sleep {
                (PetState::Sleep, rng_range(rng, cfg.sleep_range))
            } else {
                (PetState::Idle, rng_range(rng, cfg.idle_range))
            }
        }
        PetState::Walk => (PetState::Idle, rng_range(rng, cfg.idle_range)),
        PetState::Sleep => (PetState::Idle, rng_range(rng, cfg.idle_range)),
        PetState::Landing => (PetState::Idle, rng_range(rng, cfg.idle_range)),
        PetState::Climb => (PetState::Idle, rng_range(rng, cfg.idle_range)),
        // Falling, Dragged, Bonk и Roll завершаются событиями физики.
        s => (s, f32::INFINITY),
    }
}

/// Решение на стене или потолке (фаза G): спать там нельзя, зато можно
/// ползти дальше, повисеть или отцепиться и упасть.
pub fn next_state_off_floor(cfg: &BehaviorConfig, rng: &mut fastrand::Rng) -> (PetState, f32) {
    let roll = rng.u32(0..100);
    if roll < cfg.w_cling_to_climb {
        (PetState::Climb, rng_range(rng, cfg.climb_range))
    } else if roll < cfg.w_cling_to_climb + cfg.w_cling_to_drop {
        (PetState::Falling, f32::INFINITY)
    } else {
        (PetState::Idle, rng_range(rng, cfg.idle_range))
    }
}

/// Следующее мелкое занятие в Idle и его длительность (фаза G).
/// Веса заданы «на глаз»: чаще всего короткое моргание, реже — посидеть.
pub fn next_idle_action(rng: &mut fastrand::Rng) -> (IdleAction, f32) {
    match rng.u32(0..100) {
        0..=39 => (IdleAction::Blink, 0.16),
        40..=64 => (IdleAction::Wiggle, 0.9),
        65..=84 => (IdleAction::Stretch, 1.4),
        _ => (IdleAction::Sit, 3.0 + 5.0 * rng.f32()),
    }
}

fn rng_range(rng: &mut fastrand::Rng, (lo, hi): (f32, f32)) -> f32 {
    lo + (hi - lo) * rng.f32()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_eventually_walks_and_sleeps() {
        let cfg = BehaviorConfig::default();
        let mut rng = fastrand::Rng::with_seed(7);
        let mut seen_walk = false;
        let mut seen_sleep = false;
        for _ in 0..200 {
            let (s, t) = next_state_after(PetState::Idle, &cfg, &mut rng);
            assert!(t > 0.0);
            seen_walk |= s == PetState::Walk;
            seen_sleep |= s == PetState::Sleep;
        }
        assert!(seen_walk && seen_sleep);
    }

    /// Фаза G: на стене решения свои — ползти, повисеть или отцепиться,
    /// но никогда не спать.
    #[test]
    fn off_floor_never_sleeps_and_can_let_go() {
        let cfg = BehaviorConfig::default();
        let mut rng = fastrand::Rng::with_seed(3);
        let (mut climb, mut idle, mut drop) = (false, false, false);
        for _ in 0..500 {
            let (s, t) = next_state_off_floor(&cfg, &mut rng);
            assert_ne!(s, PetState::Sleep, "на стене не спят");
            assert!(t > 0.0);
            climb |= s == PetState::Climb;
            idle |= s == PetState::Idle;
            drop |= s == PetState::Falling;
        }
        assert!(climb && idle && drop, "нужны все три исхода");
    }

    /// Мелкие занятия в покое конечны и разнообразны.
    #[test]
    fn idle_actions_are_varied_and_short_enough() {
        let mut rng = fastrand::Rng::with_seed(11);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..400 {
            let (action, dur) = next_idle_action(&mut rng);
            assert!(dur > 0.0 && dur <= 10.0, "{action:?}: {dur}");
            assert_ne!(action, IdleAction::Stand, "занятие — не «стоять»");
            seen.insert(format!("{action:?}"));
        }
        assert_eq!(seen.len(), 4, "все четыре занятия: {seen:?}");
    }

    /// Скорость лазания выводится из скорости ходьбы и заметно ниже её.
    #[test]
    fn climb_is_slower_than_walking() {
        let cfg = BehaviorConfig::default();
        assert!(cfg.climb_speed() < cfg.walk_speed);
        assert!(cfg.climb_speed() > 0.0);
    }

    #[test]
    fn walk_and_sleep_return_to_idle() {
        let cfg = BehaviorConfig::default();
        let mut rng = fastrand::Rng::with_seed(1);
        assert_eq!(
            next_state_after(PetState::Walk, &cfg, &mut rng).0,
            PetState::Idle
        );
        assert_eq!(
            next_state_after(PetState::Sleep, &cfg, &mut rng).0,
            PetState::Idle
        );
    }
}
