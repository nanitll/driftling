//! Рост питомца (ТЗ §3.2, фаза B4): яйцо -> малыш -> ребёнок -> подросток ->
//! взрослый. Гейты — реальное время с рождения; для дебага возраст
//! умножается на `FoldCfg::growth_scale` ещё до вызова [`stage`]
//! (см. `journal::fold`). Стадия никогда не опускается ниже стадии
//! рождения: мигрировавший из pet.json v1/v2 питомец рождается сразу
//! взрослым и заново не вылупляется.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Stage {
    Egg,
    Baby,
    Child,
    Teen,
    Adult,
}

impl Stage {
    /// Машинное имя стадии (для IPC/логов; локализация — на стороне UI).
    pub fn as_str(&self) -> &'static str {
        match self {
            Stage::Egg => "Egg",
            Stage::Baby => "Baby",
            Stage::Child => "Child",
            Stage::Teen => "Teen",
            Stage::Adult => "Adult",
        }
    }
}

/// Гейты роста: возраст (масштабированный), с которого стадия закончилась.
/// Модель Tamagotchi P1: «1 день = 1 год», вылупление почти сразу.
pub const EGG_UNTIL_MS: u64 = 3 * 60 * 1000;
pub const BABY_UNTIL_MS: u64 = 24 * 60 * 60 * 1000;
pub const CHILD_UNTIL_MS: u64 = 3 * BABY_UNTIL_MS;
pub const TEEN_UNTIL_MS: u64 = 6 * BABY_UNTIL_MS;

/// Стадия по масштабированному возрасту, не ниже стадии рождения.
///
/// `age_ms_scaled` — возраст в мс, уже умноженный на `growth_scale`
/// (1.0 = реальное время; больше — дебаг-ускорение).
pub fn stage(age_ms_scaled: u64, born_stage: Stage) -> Stage {
    let by_age = if age_ms_scaled < EGG_UNTIL_MS {
        Stage::Egg
    } else if age_ms_scaled < BABY_UNTIL_MS {
        Stage::Baby
    } else if age_ms_scaled < CHILD_UNTIL_MS {
        Stage::Child
    } else if age_ms_scaled < TEEN_UNTIL_MS {
        Stage::Teen
    } else {
        Stage::Adult
    };
    // TODO(C2): ветвление взрослой формы по счётчику ошибок ухода
    // (care_mistakes из fold) — одна взрослая форма, пока нет арта.
    by_age.max(born_stage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_are_exact_boundaries() {
        assert_eq!(stage(0, Stage::Egg), Stage::Egg);
        assert_eq!(stage(EGG_UNTIL_MS - 1, Stage::Egg), Stage::Egg);
        assert_eq!(stage(EGG_UNTIL_MS, Stage::Egg), Stage::Baby);
        assert_eq!(stage(BABY_UNTIL_MS - 1, Stage::Egg), Stage::Baby);
        assert_eq!(stage(BABY_UNTIL_MS, Stage::Egg), Stage::Child);
        assert_eq!(stage(CHILD_UNTIL_MS - 1, Stage::Egg), Stage::Child);
        assert_eq!(stage(CHILD_UNTIL_MS, Stage::Egg), Stage::Teen);
        assert_eq!(stage(TEEN_UNTIL_MS - 1, Stage::Egg), Stage::Teen);
        assert_eq!(stage(TEEN_UNTIL_MS, Stage::Egg), Stage::Adult);
    }

    #[test]
    fn born_stage_is_a_floor_not_a_pin() {
        // Мигрант рождается взрослым и остаётся им.
        assert_eq!(stage(0, Stage::Adult), Stage::Adult);
        // Промежуточная стадия рождения: не ниже неё, но рост продолжается.
        assert_eq!(stage(0, Stage::Child), Stage::Child);
        assert_eq!(stage(TEEN_UNTIL_MS, Stage::Child), Stage::Adult);
    }
}
