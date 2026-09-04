//! Driftling core: платформонезависимая симуляция питомца.
//!
//! Правила crate (ТЗ §4, §3.7):
//! - собирается под `wasm32-unknown-unknown` (проверяется в CI);
//! - время не берётся из ОС — оно приходит параметром `dt`/`now`;
//! - никакого рендеринга и никаких протоколов — только чистая логика.

/// Идентификатор приложения (SHIPPING.md, фаза F1): reverse-DNS-имя,
/// зафиксированное до релиза. Совпадает с базовым именем .desktop-файла и
/// именем иконок (`dist/`, `assets/icons/`); им подписываются ВСЕ окна и
/// слои — app_id/namespace Wayland-оверлея, WM_CLASS X11-оверлея, окно
/// настроек (eframe) и SNI-трей, — иначе KDE/GNOME не сопоставят окно с
/// иконкой приложения.
pub const APP_ID: &str = "io.github.nanitll.driftling";

pub mod attributes;
pub mod behavior;
pub mod config;
pub mod geometry;
pub mod growth;
pub mod journal;
pub mod pack;
pub mod palette;
pub mod pet;
pub mod physics;
pub mod sprite;
pub mod stats;
pub mod sync;
pub mod text;

pub use attributes::{PetAttributes, PetRecord};
pub use behavior::{BehaviorConfig, IdleAction, PetState};
pub use config::{Config, SyncConfig, SyncMode};
pub use geometry::{Rect, Vec2};
pub use growth::Stage;
#[cfg(not(target_arch = "wasm32"))]
pub use journal::{device_journal_path_in, journal_path_in, Journal};
pub use journal::{
    fold, merge, random_device_id, DerivedPet, Event, EventKind, FoldCfg, Hlc, HlcClock,
};
pub use palette::{DEFAULT_PET_COLOR, PET_PRESETS};
pub use pet::{Direction, Pet, PointerEvent, SimPace, World};
pub use physics::{Orient, Surface};
pub use stats::PetStats;
pub use sync::{apply_remote, cursors_of, events_after, Cursors};
