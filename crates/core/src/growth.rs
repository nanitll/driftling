//! Рост питомца (ТЗ §3.2, фаза B4): яйцо -> малыш -> ребёнок -> подросток ->
//! взрослый. Гейты — реальное время с рождения; счётчик ошибок ухода на
//! переходах определяет ветку взрослой формы (модель Tamagotchi P1).
//! Логика гейтов и ошибок ухода реализуется в fold журнала (journal.rs).

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
