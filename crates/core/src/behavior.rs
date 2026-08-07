//! Стейт-машина поведения. M0: Idle/Walk/Sleep/Falling/Dragged.
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
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            walk_speed: 42.0,
            gravity: 1800.0,
            idle_range: (1.0, 4.0),
            walk_range: (2.0, 6.0),
            sleep_range: (12.0, 30.0),
            landing_time: 0.8,
            w_idle_to_walk: 55,
            w_idle_to_sleep: 12,
            hard_landing_speed: 700.0,
        }
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
        // Falling и Dragged завершаются событиями физики/указателя, не таймером.
        s => (s, f32::INFINITY),
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
