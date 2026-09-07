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
//!
//! X11-бэкенд (D3): override-redirect ARGB-окно по границам сцены,
//! XShape INPUT-регион, RandR-геометрия выхода — см. [`x11`].

use anyhow::Result;
use driftling_core::sprite::Frame;
use driftling_core::{Orient, Rect, Vec2};

mod render;
mod supervise;
pub mod wayland;
pub mod x11;

/// Запустить бэкенд под текущее окружение: `WAYLAND_DISPLAY` → Wayland,
/// иначе `DISPLAY` → X11, иначе ошибка (графической сессии нет).
/// Wayland первичен: на Wayland-сессии `DISPLAY` — это XWayland, и рисовать
/// поверх экрана через него нельзя.
pub fn run_auto(app: impl App + 'static) -> Result<()> {
    if env_nonempty("WAYLAND_DISPLAY") {
        wayland::run(app)
    } else if env_nonempty("DISPLAY") {
        x11::run(app)
    } else {
        anyhow::bail!(
            "не найдено ни WAYLAND_DISPLAY, ни DISPLAY — запускать надо из графической сессии"
        )
    }
}

/// Переменная окружения есть и непуста.
fn env_nonempty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// Что нарисовать в этом кадре.
pub struct Scene<'a> {
    /// Спрайты в логических координатах выхода (пока один питомец — один спрайт).
    pub sprites: Vec<SpriteInstance<'a>>,
    /// Прямоугольники, которые должны ловить мышь (input region).
    pub input_rects: Vec<Rect>,
}

pub struct SpriteInstance<'a> {
    pub frame: &'a Frame,
    /// Левый верхний угол спрайта (уже с учётом поворота: для четвертей
    /// 90°/270° ширина и высота кадра меняются местами).
    pub origin: Vec2,
    /// Ориентация кадра: зеркала + поворот под поверхность (фаза G).
    pub orient: Orient,
}

impl SpriteInstance<'_> {
    /// Размер кадра на экране с учётом поворота.
    pub fn size(&self) -> (u32, u32) {
        self.orient.output_size(self.frame.w, self.frame.h)
    }
}

/// События, которые бэкенд отдаёт приложению.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Размер/появление выхода: логическая ширина и высота, плюс положение
    /// выхода в глобальном логическом пространстве композитора (xdg-output).
    /// `origin` нужен worldsense (фаза D): KWin отдаёт геометрию окон в
    /// глобальных координатах, а мир питомца локален своему выходу.
    /// Неизвестно (нет xdg-output) — (0, 0): верно для единственного выхода.
    OutputGeometry {
        width: f32,
        height: f32,
        origin: Vec2,
    },
    PointerPress(Vec2),
    PointerMotion(Vec2),
    PointerRelease(Vec2),
    /// Захват потерян не по воле пользователя (композитор увёл фокус, ушёл
    /// курсор при зажатой кнопке): перетаскивание надо ЗАВЕРШИТЬ, но не
    /// швырять питомца — он просто выпадает из руки на месте.
    PointerCancel(Vec2),
    /// ПКМ по питомцу — заготовка под контекстное меню (фаза B).
    PointerMenu(Vec2),
    /// Wayland-соединение потеряно/слой закрыт; бэкенд попробует
    /// пересоздаться сам — приложению только знать (пауза симуляции).
    OutputLost,
    /// Запрошено завершение (например, IPC Quit).
    Shutdown,
}

/// Желаемый темп симуляции — энергобюджет ТЗ §7: спящий питомец не должен
/// будить CPU 30 раз в секунду.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// Движение/падение/drag: ~30 Гц.
    Active,
    /// Спокойный idle: ~5 Гц.
    Calm,
    /// Сон или питомец убран: ~1 Гц.
    Drowsy,
}

pub trait App {
    /// Шаг симуляции; `now` — монотонные секунды от старта бэкенда.
    /// Здесь же приложение разбирает свои внешние очереди (IPC-канал).
    fn tick(&mut self, now: f64) -> Scene<'_>;
    /// Событие от платформы. Возвращает false, если приложение хочет выйти.
    fn event(&mut self, ev: Event, now: f64) -> bool;
    /// Бэкенд проверяет это каждый кадр и завершает цикл при true
    /// (например, после IPC Quit, принятого внутри tick).
    fn wants_exit(&self) -> bool {
        false
    }

    /// Желаемый темп следующего тика; бэкенд адаптирует таймер.
    fn pace(&self) -> Pace {
        Pace::Active
    }
}
