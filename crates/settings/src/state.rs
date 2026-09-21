//! Живое состояние: фоновый опрос демона и отправка команд. UI-поток
//! никогда не ходит в IPC сам — только читает готовый снимок под мьютексом.
//!
//! Опрос подписной, а не «раз в секунду всегда»: `PetInfo` на стороне
//! демона сворачивает весь журнал, и держать его на секундном темпе, пока
//! окно свёрнуто и его никто не видит, — это чужой CPU за просто так.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use driftling_core::{Config, PetAttributes, PetStats, Stage};
use driftling_ipc::{call, PropInfo, Request, Response};
use eframe::egui;

use crate::i18n::fl;

/// Карточка питомца от демона.
#[derive(Clone)]
pub struct PetSnapshot {
    pub name: String,
    pub state: Option<String>,
    pub attributes: PetAttributes,
    /// Статы тамагочи (сытость/энергия/настроение + скрытое здоровье).
    pub stats: PetStats,
    /// Стадия роста (локализуется на нашей стороне).
    pub stage: Stage,
    /// Базовый цвет тела (ARGB) — акцент всего окна следует за ним.
    pub color: u32,
    pub uptime_secs: u64,
}

/// Снимок статуса синка от демона (фаза E, страница «Устройства»).
#[derive(Clone)]
pub struct SyncSnapshot {
    /// Машинный режим демона: "off" | "server" | "folder".
    pub mode: String,
    pub last_push_secs: Option<u64>,
    pub last_pull_secs: Option<u64>,
    pub devices: u32,
    pub events: u64,
    pub holding: bool,
    pub holder: Option<String>,
    pub last_error: Option<String>,
}

/// Картина мира: что сейчас на экране (страница «Мир»).
#[derive(Clone, Default)]
pub struct WorldSnapshot {
    pub screen: Option<(f32, f32, f32, f32)>,
    /// Пол мира; окно его пока только получает — пригодится карте сцены.
    #[allow(dead_code)]
    pub ground_y: Option<f32>,
    pub fullscreen_hidden: bool,
    pub props: Vec<PropInfo>,
}

/// Настройки глазами демона.
#[derive(Clone)]
pub struct ConfigSnapshot {
    pub config: Config,
    pub path: String,
    pub token_set: bool,
}

/// Последнее известное состояние демона (пишут фоновые потоки, читает UI).
#[derive(Default)]
pub struct PollState {
    /// Хотя бы один опрос уже завершился.
    pub checked: bool,
    /// Демон ответил на последний запрос.
    pub up: bool,
    pub info: Option<PetSnapshot>,
    /// Статус синка (фаза E); None — демон лежит или не ответил.
    pub sync: Option<SyncSnapshot>,
    /// Картина мира (фаза H); None — не спрашивали или демон лежит.
    pub world: Option<WorldSnapshot>,
    /// Настройки от демона; None — читаем файл сами.
    pub config: Option<ConfigSnapshot>,
    /// Сырой PetInfo (pretty JSON) для страницы «Продвинутые».
    pub raw: Option<String>,
    /// Демон новее или старше окна (несовпадение версии протокола).
    pub protocol_mismatch: bool,
}

/// Что именно опрашивать: страница «Мир» просит мир, «Устройства» — синк.
/// Биты, а не enum, потому что флаг делят UI-поток и поток опроса.
pub const WANT_WORLD: u8 = 1 << 0;
pub const WANT_SYNC: u8 = 1 << 1;
pub const WANT_CONFIG: u8 = 1 << 2;

/// Один цикл опроса демона в общий слот. Вызывается из фоновых потоков.
pub fn poll_once(slot: &Arc<Mutex<PollState>>, want: u8) {
    let mut next = PollState {
        checked: true,
        ..Default::default()
    };
    match call(&Request::PetInfo) {
        Ok(Response::PetInfo {
            name,
            state,
            attributes,
            stats,
            stage,
            color,
            uptime_secs,
        }) => {
            next.up = true;
            next.raw = Some(
                serde_json::to_string_pretty(&serde_json::json!({
                    "name": name,
                    "state": state,
                    "attributes": serde_json::to_value(attributes).unwrap_or_default(),
                    "stats": serde_json::to_value(stats).unwrap_or_default(),
                    "stage": stage.as_str(),
                    "color": format!("#{:06x}", color & 0x00ff_ffff),
                    "uptime_secs": uptime_secs,
                }))
                .unwrap_or_default(),
            );
            next.info = Some(PetSnapshot {
                name,
                state,
                attributes,
                stats,
                stage,
                color,
                uptime_secs,
            });
        }
        // Демон другой версии: сокет отвечает, но договориться нельзя —
        // это не «демон лежит», и сказать об этом надо иначе.
        Err(e) => {
            let text = format!("{e:#}");
            next.protocol_mismatch = text.contains("protocol") || text.contains("протокол");
        }
        _ => {}
    }
    if next.up {
        if want & WANT_SYNC != 0 {
            if let Ok(Response::SyncStatus {
                mode,
                last_push_secs,
                last_pull_secs,
                devices,
                events,
                holding,
                holder,
                last_error,
                ..
            }) = call(&Request::SyncStatus)
            {
                next.sync = Some(SyncSnapshot {
                    mode,
                    last_push_secs,
                    last_pull_secs,
                    devices,
                    events,
                    holding,
                    holder,
                    last_error,
                });
            }
        }
        if want & WANT_WORLD != 0 {
            if let Ok(Response::World {
                screen,
                ground_y,
                fullscreen_hidden,
                props,
            }) = call(&Request::World)
            {
                next.world = Some(WorldSnapshot {
                    screen,
                    ground_y,
                    fullscreen_hidden,
                    props,
                });
            }
        }
        if want & WANT_CONFIG != 0 {
            if let Ok(Response::Config {
                config,
                path,
                token_set,
            }) = call(&Request::GetConfig)
            {
                next.config = Some(ConfigSnapshot {
                    config,
                    path,
                    token_set,
                });
            }
        }
    }
    // Прошлый конфиг и мир не выбрасываем, если в этот раз их не просили:
    // страница не должна мигать пустотой при переключении вкладок.
    let mut slot = slot.lock().unwrap();
    if next.config.is_none() {
        next.config = slot.config.take();
    }
    if next.world.is_none() {
        next.world = slot.world.take();
    }
    if next.sync.is_none() {
        next.sync = slot.sync.take();
    }
    *slot = next;
}

/// Что опрашивать сейчас — общий флаг между UI и потоком опроса.
pub type Want = Arc<AtomicU8>;

/// Вечный поток опроса: раз в секунду при видимом окне и раз в пять секунд,
/// когда на окно не смотрят.
pub fn spawn_poller(slot: Arc<Mutex<PollState>>, want: Want, ctx: egui::Context) {
    std::thread::spawn(move || loop {
        let bits = want.load(Ordering::Relaxed);
        poll_once(&slot, bits & !FOCUS_BIT);
        ctx.request_repaint();
        let calm = bits & FOCUS_BIT == 0;
        std::thread::sleep(if calm {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        });
    });
}

/// Бит «на окно сейчас смотрят» в том же флаге, что и запросы.
pub const FOCUS_BIT: u8 = 1 << 7;

/// Итог команды для уведомления: текст плюс тон.
pub struct ActionOutcome {
    pub text: String,
    pub good: bool,
    /// Демон сказал, что применилось не всё (нужен перезапуск).
    pub partial: bool,
}

/// Разовая IPC-команда в короткоживущем потоке. Результат кладётся в слот,
/// сразу после команды дёргается свежий опрос — UI не ждёт секунду.
#[allow(clippy::too_many_arguments)]
pub fn spawn_action(
    req: Request,
    ok_text: String,
    result: Arc<Mutex<Option<ActionOutcome>>>,
    poll: Arc<Mutex<PollState>>,
    want: Want,
    // Счётчик летящих команд снимается ЗДЕСЬ же, в потоке команды: ждать
    // появления результата в слоте нельзя — UI-поток забирает его первым,
    // и ожидающий висел бы вечно, оставив кнопки выключенными навсегда.
    inflight: Arc<Mutex<usize>>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let outcome = match call(&req) {
            Ok(Response::Error(e)) => ActionOutcome {
                text: fl!("daemon-error", error = e),
                good: false,
                partial: false,
            },
            // Честный ответ на настройки: что применилось, а что ждёт
            // перезапуска. Раньше здесь было безусловное «готово».
            Ok(Response::Reloaded {
                applied,
                needs_restart,
                warnings,
            }) => {
                let mut text = ok_text;
                if !needs_restart.is_empty() {
                    text = fl!("msg-needs-restart", items = needs_restart.join(", "));
                } else if let Some(w) = warnings.first() {
                    text = w.clone();
                } else if applied.is_empty() {
                    text = fl!("msg-nothing-changed");
                }
                ActionOutcome {
                    text,
                    good: needs_restart.is_empty() && warnings.is_empty(),
                    partial: !needs_restart.is_empty() || !warnings.is_empty(),
                }
            }
            Ok(_) => ActionOutcome {
                text: ok_text,
                good: true,
                partial: false,
            },
            Err(e) => ActionOutcome {
                text: fl!("generic-error", error = format!("{e:#}")),
                good: false,
                partial: false,
            },
        };
        *result.lock().unwrap() = Some(outcome);
        poll_once(&poll, want.load(Ordering::Relaxed) & !FOCUS_BIT);
        *inflight.lock().unwrap() -= 1;
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Счётчик летящих команд обязан вернуться к нулю ДАЖЕ когда результат
    /// уже забрал UI-поток. Раньше ожидающий поток крутился до появления
    /// результата в слоте, UI забирал его первым — и кнопки оставались
    /// выключенными навсегда.
    #[test]
    fn inflight_returns_to_zero_even_if_ui_took_the_result() {
        let inflight = Arc::new(Mutex::new(1usize));
        let result = Arc::new(Mutex::new(None));
        let poll = Arc::new(Mutex::new(PollState::default()));
        let want: Want = Arc::new(AtomicU8::new(0));
        // Демона в тестах нет: команда честно вернётся ошибкой связи.
        spawn_action(
            Request::Status,
            "ок".into(),
            Arc::clone(&result),
            poll,
            want,
            Arc::clone(&inflight),
            egui::Context::default(),
        );
        // UI забирает результат сразу, как только он появился.
        let mut taken = None;
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(10));
            if let Some(o) = result.lock().unwrap().take() {
                taken = Some(o);
                break;
            }
        }
        assert!(taken.is_some(), "команда должна завершиться");
        for _ in 0..200 {
            if *inflight.lock().unwrap() == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("счётчик команд завис — кнопки остались бы выключенными");
    }
}
