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
pub mod journal;
pub mod pet;
pub mod physics;
pub mod sprite;

pub use attributes::{PetAttributes, PetRecord};
pub use behavior::{BehaviorConfig, PetState};
pub use config::Config;
pub use geometry::{Rect, Vec2};
pub use pet::{Direction, Pet, PointerEvent, SimPace, World};
