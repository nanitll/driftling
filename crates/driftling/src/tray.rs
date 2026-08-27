//! Иконка демона в системном трее (B7): StatusNotifierItem через ksni.
//!
//! Работает в собственном потоке (blocking-API ksni) и разговаривает с
//! циклом приложения тем же каналом, что и IPC-сервер: на каждое действие
//! заводится одноразовый reply-канал, ответ логируется. Состояние
//! «призван/убран» и цвет питомца держим опросом Request::PetInfo раз в
//! POLL_INTERVAL (плюс оптимистично сразу после успешного действия) — без
//! общих атомиков с DaemonApp. Иконка пересоздаётся при смене цвета:
//! ksni диффит свойства при update() и сам сигналит хосту NewIcon.
//!
//! Нет SNI-вотчера (GNOME без расширения) — `run()` возвращает ошибку,
//! демон продолжает работать без трея: трей не единственный вход (B7).

use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use driftling_core::{sprite, Stage, DEFAULT_PET_COLOR};
use driftling_ipc::{Request, Response};
use ksni::blocking::TrayMethods;

use crate::daemon::IpcMessage;
use crate::i18n::fl;

/// Сторона иконки в px: SNI-хост масштабирует сам, 64 хватает и на HiDPI.
const ICON_SIZE: u32 = 64;

/// Период опроса состояния демона.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Сколько ждать ответа цикла приложения (согласовано с IPC_REPLY_TIMEOUT
/// демона: дольше цикл всё равно не отвечает).
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Поднять трей и крутить опрос состояния до завершения демона.
/// Блокирует текущий поток — вызывается из выделенного потока daemon::run().
pub fn run(tx: Sender<IpcMessage>) -> Result<()> {
    let (summoned, color) = match query_info(&tx) {
        Ok(Some(pair)) => pair,
        _ => (false, DEFAULT_PET_COLOR),
    };
    let tray = DriftlingTray {
        tx: tx.clone(),
        summoned,
        color,
        icon: tray_icon(color),
    };
    let handle = tray.spawn().context("SNI-сервис трея не поднялся")?;
    log::info!("трей: SNI-иконка зарегистрирована");

    loop {
        std::thread::sleep(POLL_INTERVAL);
        if handle.is_closed() {
            log::info!("трей: SNI-сервис остановлен");
            return Ok(());
        }
        match query_info(&tx) {
            // update() дёшев: ksni сигналит хосту только при реальной
            // смене свойств/меню (диффит по хэшам); перекраска питомца
            // пересоздаёт иконку — хост получает NewIcon.
            Ok(Some((summoned, color))) => {
                let updated = handle.update(|t| {
                    t.summoned = summoned;
                    if t.color != color {
                        t.color = color;
                        t.icon = tray_icon(color);
                    }
                });
                if updated.is_none() {
                    return Ok(());
                }
            }
            // Цикл занят/не ответил — не фатально, живём с прошлым знанием.
            Ok(None) => {}
            // Канал закрыт: демон завершается — гасим иконку и уходим.
            Err(_) => {
                handle.shutdown().wait();
                log::info!("трей: демон завершился, иконка снята");
                return Ok(());
            }
        }
    }
}

/// Состояние иконки. `summoned` — последнее известное «питомец на экране»,
/// от него зависят подпись тоггла в меню и действие левого клика.
struct DriftlingTray {
    tx: Sender<IpcMessage>,
    summoned: bool,
    /// Последний известный цвет питомца — детектор перекраски иконки.
    color: u32,
    /// Готовый ARGB32-кадр: idle-кадр плейсхолдера в цвете питомца.
    icon: ksni::Icon,
}

impl DriftlingTray {
    /// Призвать/убрать питомца по текущему знанию о состоянии; при успехе
    /// состояние обновляется оптимистично (опрос затем подтвердит).
    fn toggle_summon(&mut self) {
        let (req, target) = if self.summoned {
            (Request::Dismiss, false)
        } else {
            (Request::Summon, true)
        };
        match call(&self.tx, req) {
            Ok(Some(Response::Ok)) => {
                self.summoned = target;
                log::info!(
                    "трей: {}",
                    if target {
                        "питомец призван"
                    } else {
                        "питомец убран"
                    }
                );
            }
            Ok(Some(resp)) => log::warn!("трей: ответ на призыв/убирание: {resp:?}"),
            Ok(None) => log::warn!("трей: демон не ответил на призыв/убирание"),
            Err(e) => log::warn!("трей: {e}"),
        }
    }

    /// «Остановить демона»: демон сам снимет иконку — поток опроса увидит
    /// закрытый канал и погасит SNI-сервис.
    fn quit_daemon(&mut self) {
        match call(&self.tx, Request::Quit) {
            Ok(reply) => log::info!("трей: запрошено завершение демона, ответ {reply:?}"),
            Err(e) => log::warn!("трей: {e}"),
        }
    }
}

impl ksni::Tray for DriftlingTray {
    fn id(&self) -> String {
        "driftling".into()
    }

    fn title(&self) -> String {
        fl!("tray-tooltip")
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: fl!("tray-tooltip"),
            ..Default::default()
        }
    }

    /// Левый клик по иконке — тот же тоггл, что и первый пункт меню.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.toggle_summon();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        vec![
            StandardItem {
                label: if self.summoned {
                    fl!("tray-dismiss")
                } else {
                    fl!("tray-summon")
                },
                activate: Box::new(Self::toggle_summon),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: fl!("tray-settings"),
                activate: Box::new(|_: &mut Self| spawn_settings()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: fl!("tray-quit"),
                activate: Box::new(Self::quit_daemon),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Отправить запрос в цикл приложения. `Err` — канал закрыт (демон
/// завершается), `Ok(None)` — цикл не ответил за REPLY_TIMEOUT.
fn call(tx: &Sender<IpcMessage>, req: Request) -> Result<Option<Response>> {
    let (reply_tx, reply_rx) = mpsc::channel();
    tx.send((req, reply_tx))
        .map_err(|_| anyhow!("цикл приложения недоступен"))?;
    Ok(reply_rx.recv_timeout(REPLY_TIMEOUT).ok())
}

/// Спросить демона о питомце: (призван ли, цвет тела).
/// Семантика ошибок как у `call`.
fn query_info(tx: &Sender<IpcMessage>) -> Result<Option<(bool, u32)>> {
    match call(tx, Request::PetInfo)? {
        // state = None означает «убран с экрана» (контракт PetInfo).
        Some(Response::PetInfo { state, color, .. }) => Ok(Some((state.is_some(), color))),
        Some(other) => {
            log::warn!("трей: неожиданный ответ на PetInfo: {other:?}");
            Ok(None)
        }
        None => Ok(None),
    }
}

/// idle-кадр плейсхолдера в цвете питомца -> ksni::Icon: ARGB32 в network
/// byte order, то есть big-endian побайтово (кадр ядра — те же
/// ARGB8888-слова).
fn tray_icon(color: u32) -> ksni::Icon {
    let set = sprite::placeholder_colored(ICON_SIZE, Stage::Adult, color);
    let frame = &set.idle[0];
    let mut data = Vec::with_capacity(frame.argb.len() * 4);
    for px in &frame.argb {
        data.extend_from_slice(&px.to_be_bytes());
    }
    ksni::Icon {
        width: frame.w as i32,
        height: frame.h as i32,
        data,
    }
}

/// Запустить окно настроек тем же способом, что `driftling settings`
/// (main.rs): бинарь driftling-settings рядом с собой, затем в PATH.
/// Завершения не ждём — колбэк меню не должен блокироваться на GUI;
/// чтобы не плодить зомби, ребёнка дожидается отдельный поток.
fn spawn_settings() {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("driftling-settings")))
        .filter(|p| p.exists());
    let program = sibling.unwrap_or_else(|| "driftling-settings".into());
    match std::process::Command::new(&program).spawn() {
        Ok(mut child) => {
            log::info!("трей: настройки запущены ({program:?}, pid {})", child.id());
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("трей: не удалось запустить {program:?}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    /// Ответить на очередной запрос из канала как «демон».
    fn answer(rx: &Receiver<IpcMessage>, resp: Response) -> Request {
        let (req, reply) = rx.recv().unwrap();
        reply.send(resp).unwrap();
        req
    }

    fn tray(tx: Sender<IpcMessage>, summoned: bool) -> DriftlingTray {
        DriftlingTray {
            tx,
            summoned,
            color: DEFAULT_PET_COLOR,
            icon: tray_icon(DEFAULT_PET_COLOR),
        }
    }

    #[test]
    fn icon_is_argb32_of_expected_size() {
        let icon = tray_icon(DEFAULT_PET_COLOR);
        assert_eq!(icon.width, ICON_SIZE as i32);
        assert_eq!(icon.height, ICON_SIZE as i32);
        assert_eq!(icon.data.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        // Хоть один непрозрачный пиксель: альфа (первый байт ARGB) = 255.
        assert!(icon.data.chunks_exact(4).any(|px| px[0] == 0xff));
    }

    /// Иконка следует за цветом питомца: разные цвета — разные пиксели.
    #[test]
    fn icon_follows_pet_color() {
        let default = tray_icon(DEFAULT_PET_COLOR);
        let amber = tray_icon(0xff_e8_94_4a);
        assert_eq!(default.data.len(), amber.data.len());
        assert_ne!(default.data, amber.data);
    }

    #[test]
    fn toggle_sends_summon_then_dismiss() {
        let (tx, rx) = mpsc::channel();
        let mut t = tray(tx, false);

        let daemon = std::thread::spawn(move || {
            let first = answer(&rx, Response::Ok);
            let second = answer(&rx, Response::Ok);
            (first, second)
        });
        t.toggle_summon();
        assert!(t.summoned, "успешный Summon переключает состояние");
        t.toggle_summon();
        assert!(!t.summoned, "успешный Dismiss переключает обратно");

        let (first, second) = daemon.join().unwrap();
        assert!(matches!(first, Request::Summon));
        assert!(matches!(second, Request::Dismiss));
    }

    #[test]
    fn toggle_keeps_state_on_error() {
        let (tx, rx) = mpsc::channel();
        let mut t = tray(tx, false);
        let daemon = std::thread::spawn(move || {
            answer(&rx, Response::Error("нет геометрии".into()));
        });
        t.toggle_summon();
        assert!(!t.summoned, "ошибка демона не переключает состояние");
        daemon.join().unwrap();
    }

    #[test]
    fn call_reports_closed_channel() {
        let (tx, rx) = mpsc::channel::<IpcMessage>();
        drop(rx);
        assert!(call(&tx, Request::Status).is_err());
        assert!(query_info(&tx).is_err());
    }

    /// Картинка PetInfo для ответов «демона» в тестах.
    fn petinfo(state: Option<&str>, color: u32) -> Response {
        Response::PetInfo {
            name: "Тестик".into(),
            state: state.map(str::to_string),
            attributes: driftling_core::PetAttributes::default(),
            stats: driftling_core::PetStats::default(),
            stage: Stage::Adult,
            color,
            uptime_secs: 1,
        }
    }

    #[test]
    fn query_info_maps_petinfo() {
        let (tx, rx) = mpsc::channel();
        let daemon = std::thread::spawn(move || {
            answer(&rx, petinfo(Some("Idle"), 0xff_e8_94_4a));
            answer(&rx, petinfo(None, DEFAULT_PET_COLOR));
            // Неожиданный ответ -> None (не фатально).
            answer(&rx, Response::Ok);
        });
        assert_eq!(query_info(&tx).unwrap(), Some((true, 0xff_e8_94_4a)));
        assert_eq!(query_info(&tx).unwrap(), Some((false, DEFAULT_PET_COLOR)));
        assert_eq!(query_info(&tx).unwrap(), None);
        daemon.join().unwrap();
    }

    /// Меню: тоггл отражает состояние, всего 4 пункта (тоггл, настройки,
    /// разделитель, стоп). Подписи не проверяем дословно — они из fl!.
    #[test]
    fn menu_shape_follows_summoned() {
        use ksni::menu::MenuItem;
        use ksni::Tray as _;
        let (tx, _rx) = mpsc::channel();
        let mut t = tray(tx, false);

        let menu = t.menu();
        assert_eq!(menu.len(), 4);
        assert!(matches!(menu[2], MenuItem::Separator));
        let label_dismissed = match &menu[0] {
            MenuItem::Standard(item) => item.label.clone(),
            _ => panic!("первый пункт меню не Standard"),
        };

        t.summoned = true;
        let menu = t.menu();
        let label_summoned = match &menu[0] {
            MenuItem::Standard(item) => item.label.clone(),
            _ => panic!("первый пункт меню не Standard"),
        };
        assert_ne!(
            label_dismissed, label_summoned,
            "подпись тоггла меняется вместе с summoned"
        );
    }
}
