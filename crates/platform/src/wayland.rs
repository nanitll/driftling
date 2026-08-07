//! Wayland-бэкенд: wlr-layer-shell overlay + wl_shm рендер.
//!
//! РЕАЛИЗУЕТСЯ В M0 (см. задачу в docs/TZ.md §6). Требования:
//! - smithay-client-toolkit 0.19: `LayerShell::create_layer_surface` c
//!   `Layer::Overlay`, anchor = все четыре кромки, `exclusive_zone(-1)`,
//!   `KeyboardInteractivity::None`;
//! - прозрачность: ARGB8888 буферы из `SlotPool`, фон полностью прозрачный;
//! - клик-сквозь: `wl_surface::set_input_region` = объединение
//!   `Scene::input_rects`, обновлять при каждом изменении, пустая сцена =
//!   пустой регион;
//! - кадры по `wl_surface::frame` callback (не таймер), но не чаще ~60 fps и
//!   не реже 10 fps (таймер-будильник для сонного питомца);
//! - масштаб выхода (HiDPI): логические координаты = буфер / scale;
//! - мультивыход: M0 — только первый выход, помечено TODO(M3);
//! - выход из цикла: `App::event -> false` или Shutdown.

use crate::App;
use anyhow::Result;

/// Запустить event loop бэкенда; возвращается после Shutdown.
pub fn run(_app: impl App) -> Result<()> {
    anyhow::bail!("wayland backend: не реализован (M0 в работе)")
}
