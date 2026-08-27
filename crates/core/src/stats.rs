//! Статы тамагочи (ТЗ §3.2, фаза B2): сытость/энергия/настроение —
//! видимые, здоровье — скрытое производное. Все значения 0..=100.
//!
//! Статы никогда не хранятся и не мутируются напрямую: их вычисляет
//! `journal::fold` из журнала событий; все правила (деградация,
//! оффлайн-пол 20, кормление в два приёма, болезнь вместо смерти)
//! и их константы живут в `journal::FoldCfg`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetStats {
    /// Сытость 0..=100.
    pub satiety: f32,
    /// Энергия 0..=100.
    pub energy: f32,
    /// Настроение 0..=100.
    pub mood: f32,
    /// Здоровье 0..=100 (скрытое, производное от ухода).
    pub health: f32,
}

impl Default for PetStats {
    fn default() -> Self {
        Self {
            satiety: 100.0,
            energy: 100.0,
            mood: 100.0,
            health: 100.0,
        }
    }
}
