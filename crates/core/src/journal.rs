//! Журнал событий ухода — источник истины состояния тамагочи (ТЗ §3.5).
//!
//! M0: только типы-заготовки, чтобы формат закладывался с самого начала.
//! Реализация (HLC-метки, G-Set-слияние, снапшоты/компакция, fold статов) — M2.
//! Требование: детерминированный fold, никакого чтения часов внутри crate.

use serde::{Deserialize, Serialize};

/// Метка гибридных логических часов (модель jlongster/Actual Budget):
/// лексикографическая сортировка строки даёт полный порядок.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hlc {
    /// Unix-время в миллисекундах (физическая компонента).
    pub wall_ms: u64,
    /// Логический счётчик (tie-break внутри одной миллисекунды).
    pub counter: u16,
    /// Идентификатор устройства.
    pub device: String,
}

/// Событие ухода. Слияние журналов = объединение множеств по `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CareEvent {
    pub id: Hlc,
    pub kind: CareKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CareKind {
    Summoned,
    Fed { food: String },
    Petted,
    PutToSleep,
    Renamed { name: String },
}
