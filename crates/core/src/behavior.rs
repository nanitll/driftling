//! Стейт-машина поведения. M0: Idle/Walk/Sleep/Falling/Dragged;
//! фаза G добавляет лазание (Climb) и удар о потолок (Bonk), а также
//! мелкие занятия внутри Idle ([`IdleAction`]).
//! Переходы — по таймерам с весами (все константы в BehaviorConfig, ТЗ §3.2).

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
    /// Гравитация, px/s^2.
    pub gravity: f32,
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
    /// Скорость падения, с которой приземление считается «жёстким», px/s.
    pub hard_landing_speed: f32,

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
    /// Скорость удара о потолок, выше которой питомец не цепляется, а
    /// набивает шишку (Bonk), px/s.
    pub ceiling_grab_speed: f32,
    /// Горизонтальная скорость влёта в стену, с которой питомец за неё
    /// цепляется в падении, px/s.
    pub wall_grab_speed: f32,
    /// Длительность Bonk, сек.
    pub bonk_time: f32,
    /// Коэффициент сопротивления воздуха для горизонтальной скорости, 1/с:
    /// бросок затухает, вместо того чтобы лететь равномерно до стены.
    pub air_drag: f32,
    /// Диапазон паузы между мелкими занятиями в Idle, сек.
    pub fidget_range: (f32, f32),
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            walk_speed: 38.0,
            gravity: 1800.0,
            // Фаза G: питомец должен быть спокойным соседом, а не бегать
            // без остановки — паузы длиннее прогулок.
            idle_range: (4.0, 12.0),
            walk_range: (1.5, 4.5),
            sleep_range: (12.0, 30.0),
            landing_time: 0.8,
            w_idle_to_walk: 32,
            w_idle_to_sleep: 14,
            hard_landing_speed: 700.0,

            climb_speed_scale: 0.65,
            // Баланс замерен прогоном часа жизни (см. `g_stats` в pet.rs):
            // ~20 вылазок на стены в час, 15% времени вне пола.
            w_wall_climb: 30,
            w_wall_trip: 8,
            w_cling_to_climb: 60,
            w_cling_to_drop: 12,
            climb_range: (2.0, 7.0),
            // Бросок вверх — это приглашение повисеть: цепляется почти
            // всегда, шишка достаётся только совсем зверскому запуску.
            ceiling_grab_speed: 1500.0,
            wall_grab_speed: 90.0,
            bonk_time: 0.5,
            air_drag: 0.9,
            fidget_range: (1.5, 5.0),
        }
    }
}

impl BehaviorConfig {
    /// Скорость движения по стене/потолку, px/s.
    pub fn climb_speed(&self) -> f32 {
        self.walk_speed * self.climb_speed_scale
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
        // Falling, Dragged и Bonk завершаются событиями физики/указателя.
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
