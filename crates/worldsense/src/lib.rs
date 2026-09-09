//! Восприятие мира: где окна, где пол (ROADMAP фаза D, D1+D2).
//!
//! Демон спрашивает у провайдера снапшот «рельефа» рабочего стола — верхние
//! кромки окон становятся платформами для питомца, верх нижней панели — полом.
//!
//! Контракт деградации: если окружение не поддержано или провайдер умер,
//! [`WorldSense::latest`] возвращает `None`, и физика живёт на нижней кромке
//! экрана. Это штатный режим, а не ошибка — провайдер никогда не паникует.
//!
//! Сейчас реализованы:
//! - KWin (KDE Plasma 6): свой QML-скрипт внутри KWin шлёт снапшоты по D-Bus
//!   (см. `assets/kwin/driftling-sense.qml` и модуль [`kwin`]);
//! - sway: i3-IPC через `$SWAYSOCK` (модуль [`sway`]);
//! - Hyprland: сокеты инстанса из `$HYPRLAND_INSTANCE_SIGNATURE`
//!   (модуль [`hyprland`]);
//! - X11 (чистый X11-сеанс без Wayland): EWMH-опрос корня (модуль [`x11`]);
//! - Null: всегда `None` (все прочие окружения).

mod hyprland;
mod kwin;
mod null;
mod parse;
mod sway;
mod x11;

use driftling_core::Rect;

/// Окно-платформа: полный прямоугольник окна в логических координатах экрана
/// (ось Y вниз, (0,0) — левый верх). Физика ходит по верхней кромке.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPlatform {
    pub rect: Rect,
    /// Стабильный идентификатор окна (хэш KWin `internalId`) — чтобы физика
    /// могла отличить «моё окно уехало» от «появилось другое окно».
    pub id: u64,
}

/// Рабочая область одного выхода: геометрия самого выхода и свободная от
/// панелей область внутри него — обе в глобальных координатах композитора.
///
/// Питомец живёт на своём выходе, а рабочие области у мониторов разные
/// (панель может быть только на одном). Поэтому область приходит вместе с
/// геометрией экрана — демон выбирает свою по совпадению с выходом.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenArea {
    pub screen: Rect,
    pub area: Rect,
}

/// Снимок «рельефа» рабочего стола.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorldSnapshot {
    /// Видимые, несвёрнутые, обычные окна текущего рабочего стола;
    /// порядок — сверху вниз по стекингу (первое — самое верхнее).
    pub platforms: Vec<WindowPlatform>,
    /// Верх нижней панели = пол. `None` — данных нет, пол = низ экрана.
    /// Общий (грубый) вариант для провайдеров без разбивки по экранам.
    pub workspace_bottom: Option<f32>,
    /// Рабочие области по выходам (KWin). Пусто — данных нет.
    pub screen_areas: Vec<ScreenArea>,
    /// Есть полноэкранное окно: питомцу пора прятаться (вежливость, D5).
    pub fullscreen_active: bool,
    /// Класс окна, из-за которого включилась вежливость — только для лога:
    /// без него «питомец опять пропал» невозможно объяснить.
    pub fullscreen_by: Option<String>,
}

impl WorldSnapshot {
    /// Рабочая область выхода, чья геометрия совпала с `output`
    /// (в глобальных координатах). Точного совпадения не требуем: выбираем
    /// область с наибольшим пересечением — масштабирование и округление
    /// логических координат не должны ломать выбор.
    pub fn area_for_output(&self, output: Rect) -> Option<Rect> {
        self.screen_areas
            .iter()
            .map(|sa| (overlap(sa.screen, output), sa.area))
            .filter(|(o, _)| *o > 0.0)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, area)| area)
    }
}

/// Площадь пересечения прямоугольников.
fn overlap(a: Rect, b: Rect) -> f32 {
    let w = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let h = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    w * h
}

/// Источник снапшотов мира. Реализации не блокируют надолго и не паникуют.
pub trait WorldSense: Send {
    /// Последний известный снапшот; `None` — провайдер мёртв или данных нет.
    fn latest(&mut self) -> Option<WorldSnapshot>;
}

/// Выбирает провайдера под текущее окружение. Порядок:
///
/// 1. KDE (в `XDG_CURRENT_DESKTOP` есть `KDE` и `org.kde.KWin` на шине) —
///    KWin-провайдер;
/// 2. задан `$SWAYSOCK` — sway (i3-IPC);
/// 3. задана `$HYPRLAND_INSTANCE_SIGNATURE` — Hyprland;
/// 4. `$DISPLAY` без `$WAYLAND_DISPLAY` (чистый X11-сеанс) — EWMH-провайдер;
/// 5. иначе Null (штатная деградация: пол = низ экрана).
///
/// Ошибка подъёма провайдера не фатальна — пробуем следующего по списку.
pub fn detect() -> Box<dyn WorldSense> {
    if desktop_is_kde() {
        match kwin::KWinSense::new() {
            Ok(sense) => {
                log::info!("worldsense: выбран KWin-провайдер");
                return Box::new(sense);
            }
            Err(e) => log::warn!("worldsense: KWin-провайдер не поднялся ({e})"),
        }
    }
    if env_non_empty("SWAYSOCK") {
        match sway::SwaySense::new() {
            Ok(sense) => {
                log::info!("worldsense: выбран sway-провайдер");
                return Box::new(sense);
            }
            Err(e) => log::warn!("worldsense: sway-провайдер не поднялся ({e})"),
        }
    }
    if env_non_empty("HYPRLAND_INSTANCE_SIGNATURE") {
        match hyprland::HyprlandSense::new() {
            Ok(sense) => {
                log::info!("worldsense: выбран Hyprland-провайдер");
                return Box::new(sense);
            }
            Err(e) => log::warn!("worldsense: Hyprland-провайдер не поднялся ({e})"),
        }
    }
    // --- D3b: X11 (EWMH) — проверка окружения внутри try_provider ---
    if let Some(sense) = x11::try_provider() {
        return sense;
    }
    // --- конец D3b ---
    log::info!("worldsense: подходящего провайдера нет — null (пол = низ экрана)");
    Box::new(null::NullSense)
}

/// Переменная окружения задана и не пуста.
fn env_non_empty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// `XDG_CURRENT_DESKTOP` — список через `:`, регистр не гарантирован.
fn desktop_is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| {
            v.split(':')
                .any(|part| part.trim().eq_ignore_ascii_case("kde"))
        })
        .unwrap_or(false)
}
