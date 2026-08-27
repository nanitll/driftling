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
//! - Null: всегда `None` (все прочие окружения до D3).

mod kwin;
mod null;
mod parse;

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

/// Снимок «рельефа» рабочего стола.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorldSnapshot {
    /// Видимые, несвёрнутые, обычные окна текущего рабочего стола;
    /// порядок — сверху вниз по стекингу (первое — самое верхнее).
    pub platforms: Vec<WindowPlatform>,
    /// Верх нижней панели = пол. `None` — данных нет, пол = низ экрана.
    pub workspace_bottom: Option<f32>,
    /// Есть полноэкранное окно: питомцу пора прятаться (вежливость, D5).
    pub fullscreen_active: bool,
}

/// Источник снапшотов мира. Реализации не блокируют надолго и не паникуют.
pub trait WorldSense: Send {
    /// Последний известный снапшот; `None` — провайдер мёртв или данных нет.
    fn latest(&mut self) -> Option<WorldSnapshot>;
}

/// Выбирает провайдера под текущее окружение.
///
/// KDE (в `XDG_CURRENT_DESKTOP` есть `KDE` и `org.kde.KWin` присутствует на
/// сессионной шине) — KWin-провайдер; всё остальное — Null-провайдер
/// (штатная деградация: пол = низ экрана).
pub fn detect() -> Box<dyn WorldSense> {
    if !desktop_is_kde() {
        log::info!("worldsense: не-KDE окружение — null-провайдер (пол = низ экрана)");
        return Box::new(null::NullSense);
    }
    match kwin::KWinSense::new() {
        Ok(sense) => {
            log::info!("worldsense: KWin-провайдер активен");
            Box::new(sense)
        }
        Err(e) => {
            log::warn!("worldsense: KWin-провайдер не поднялся ({e}) — null-провайдер");
            Box::new(null::NullSense)
        }
    }
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
