//! Платформенные бэкенды оверлея.
//!
//! Контракт (ТЗ §4): бэкенд владеет event loop'ом; приложение отдаёт ему
//! реализацию [`App`]. Бэкенд зовёт `tick` с частотой кадров, рисует
//! возвращённую сцену и транслирует события указателя обратно.
//!
//! Wayland-бэкенд (M0): один прозрачный wlr-layer-shell overlay-surface на
//! выход, `keyboard_interactivity: None`, input region = прямоугольник спрайта
//! (обновляется каждый кадр), рендер CPU в wl_shm через SlotPool.
//! Модель — wl_shimeji (см. docs/RESEARCH.md), реализация clean-room.

use driftling_core::sprite::Frame;
use driftling_core::{Rect, Vec2};

pub mod wayland;

/// Что нарисовать в этом кадре.
pub struct Scene<'a> {
    /// Спрайты в логических координатах выхода (пока один питомец — один спрайт).
    pub sprites: Vec<SpriteInstance<'a>>,
    /// Прямоугольники, которые должны ловить мышь (input region).
    pub input_rects: Vec<Rect>,
}

pub struct SpriteInstance<'a> {
    pub frame: &'a Frame,
    /// Левый верхний угол спрайта.
    pub origin: Vec2,
    /// Зеркалировать по горизонтали (питомец смотрит влево).
    pub mirror: bool,
}

/// События, которые бэкенд отдаёт приложению.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Размер/появление выхода: логическая ширина и высота.
    OutputGeometry { width: f32, height: f32 },
    PointerPress(Vec2),
    PointerMotion(Vec2),
    PointerRelease(Vec2),
    /// Запрошено завершение (например, IPC Quit).
    Shutdown,
}

pub trait App {
    /// Шаг симуляции; `now` — монотонные секунды от старта бэкенда.
    fn tick(&mut self, now: f64) -> Scene<'_>;
    /// Событие от платформы. Возвращает false, если приложение хочет выйти.
    fn event(&mut self, ev: Event, now: f64) -> bool;
}
