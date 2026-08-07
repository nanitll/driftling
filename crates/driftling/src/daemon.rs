//! Демон: симуляция + платформа + IPC.
//!
//! РЕАЛИЗУЕТСЯ В M0. Связывает:
//! - `driftling_core::Pet` (симуляция, tick + pointer);
//! - `driftling_core::sprite::placeholder` (кадры);
//! - `driftling_platform::wayland::run` (оверлей: App::tick -> Scene);
//! - `driftling_ipc::Server` в отдельном потоке -> канал -> App::event.
//!
//! Контракт App::tick: собрать Scene из текущего кадра питомца
//! (`SpriteSet::frame(state, state_time, facing)`, origin = bounds().{x,y},
//! mirror = facing==Left) и input_rects = [bounds()], когда питомец призван.

use anyhow::Result;

pub fn run() -> Result<()> {
    anyhow::bail!("daemon: не реализован (M0 в работе)")
}
