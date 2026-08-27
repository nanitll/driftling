//! Driftling core: платформонезависимая симуляция питомца.
//!
//! Правила crate (ТЗ §4, §3.7):
//! - собирается под `wasm32-unknown-unknown` (проверяется в CI);
//! - время не берётся из ОС — оно приходит параметром `dt`/`now`;
//! - никакого рендеринга и никаких протоколов — только чистая логика.

pub mod attributes;
pub mod behavior;
pub mod config;
pub mod geometry;
pub mod growth;
pub mod journal;
pub mod palette;
pub mod pet;
pub mod physics;
pub mod sprite;
pub mod stats;
pub mod text;

pub use attributes::{PetAttributes, PetRecord};
pub use behavior::{BehaviorConfig, PetState};
pub use config::Config;
pub use geometry::{Rect, Vec2};
pub use growth::Stage;
pub use journal::{
    fold, merge, random_device_id, DerivedPet, Event, EventKind, FoldCfg, Hlc, HlcClock,
};
#[cfg(not(target_arch = "wasm32"))]
pub use journal::{journal_path_in, Journal};
pub use palette::{DEFAULT_PET_COLOR, PET_PRESETS};
pub use pet::{Direction, Pet, PointerEvent, SimPace, World};
pub use stats::PetStats;
