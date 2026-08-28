//! X11-провайдер (D3): рельеф рабочего стола из EWMH-свойств корневого окна.
//!
//! Событийная модель без потоков: подписываемся на `PropertyNotify` корня
//! (`_NET_CLIENT_LIST_STACKING` и компания) и на `StructureNotify` +
//! `PropertyNotify` каждого клиентского окна (перемещение WM-рамки приходит
//! синтетическим ConfigureNotify по ICCCM). [`WorldSense::latest`] неблокирующе
//! выгребает события; если что-то изменилось — перечитывает свойства
//! (с коалесценцией, чтобы драг чужого окна не молотил опрос на 30 Гц).
//!
//! Координаты — глобальные root-координаты (как у KWin-провайдера): демон
//! переводит их в локальные координаты выхода через origin из
//! `Event::OutputGeometry`. Геометрия окна: `TranslateCoordinates` даёт
//! позицию клиентской области в root, `_NET_FRAME_EXTENTS` расширяет её до
//! WM-рамки — питомец стоит на заголовке, а не на «стекле».
//!
//! Фильтрация: окна чужих рабочих столов и типов DOCK/DESKTOP отсекаются
//! здесь; свёрнутые (`_NET_WM_STATE_HIDDEN`), свои окна driftling, мелочь и
//! skip-taskbar — общим [`parse::filter`] (тот же путь, что у KWin).
//! `fullscreen_active` — `_NET_WM_STATE_FULLSCREEN` на активном окне
//! (плюс дубль по видимым окнам внутри filter).
//!
//! Деградация: любая смерть соединения → `None` (пол = низ экрана) и ленивые
//! попытки переподключения не чаще раза в 5 с. Паник нет.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use x11rb::connection::Connection as _;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, Window,
};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;

use crate::parse::{self, RawSnapshot, RawWindow, RawWorkArea};
use crate::{WorldSense, WorldSnapshot};

x11rb::atom_manager! {
    /// EWMH-атомы; интернируются одним раунд-трипом при подключении.
    Atoms:
    AtomsCookie {
        _NET_CLIENT_LIST_STACKING,
        _NET_CURRENT_DESKTOP,
        _NET_WORKAREA,
        _NET_ACTIVE_WINDOW,
        _NET_WM_DESKTOP,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_FRAME_EXTENTS,
    }
}

/// `_NET_WM_DESKTOP` со значением «на всех рабочих столах».
const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;
/// Потолок длины свойства в 32-битных словах (список окон, workarea).
const PROP_LEN: u32 = 4096;
/// Умершее соединение переподключаем не чаще раза в это время.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);
/// Коалесценция перечитывания: драг чужого окна шлёт ConfigureNotify пачками,
/// перечитываем рельеф не чаще (KWin-скрипт коалесцирует так же, 50 мс).
const REBUILD_MIN_INTERVAL: Duration = Duration::from_millis(100);
/// Страховочный опрос при полной тишине: ловит пропущенные события
/// (не-ICCCM WM без синтетических ConfigureNotify).
const HEARTBEAT: Duration = Duration::from_secs(5);

/// Пробовать ли X11-провайдера в этом окружении, и если да — поднять его.
/// Чистый X11-сеанс: `DISPLAY` есть, `WAYLAND_DISPLAY` нет (на Wayland
/// `DISPLAY` — это XWayland, который видит только X11-клиентов — не мир).
/// `None` — окружение не наше или провайдер не поднялся (детали в логе).
pub(crate) fn try_provider() -> Option<Box<dyn WorldSense>> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    if wayland || !x11 {
        return None;
    }
    match Live::connect() {
        Ok(live) => {
            log::info!("worldsense: X11-провайдер (EWMH) активен");
            Some(Box::new(X11Sense {
                live: Some(live),
                last_connect: Some(Instant::now()),
            }))
        }
        Err(e) => {
            log::warn!("worldsense: X11-провайдер не поднялся ({e:#})");
            None
        }
    }
}

/// Провайдер: живое соединение + ленивое переподключение после его смерти.
pub(crate) struct X11Sense {
    live: Option<Live>,
    last_connect: Option<Instant>,
}

impl WorldSense for X11Sense {
    fn latest(&mut self) -> Option<WorldSnapshot> {
        if self.live.is_none() {
            let due = self
                .last_connect
                .is_none_or(|t| t.elapsed() >= RECONNECT_BACKOFF);
            if !due {
                return None;
            }
            self.last_connect = Some(Instant::now());
            match Live::connect() {
                Ok(live) => {
                    log::info!("worldsense: X11-соединение восстановлено");
                    self.live = Some(live);
                }
                Err(e) => {
                    log::debug!("worldsense: X11 недоступен: {e:#}");
                    return None;
                }
            }
        }
        let live = self.live.as_mut()?;
        match live.poll() {
            Ok(snap) => snap,
            Err(e) => {
                // Смерть X-сервера = смерть сессии; но вдруг он вернётся.
                log::warn!("worldsense: X11-соединение потеряно ({e:#})");
                self.live = None;
                None
            }
        }
    }
}

/// Живое соединение и его кэш.
struct Live {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    /// Окна, на события которых мы подписаны.
    tracked: HashSet<Window>,
    snap: Option<WorldSnapshot>,
    built_at: Option<Instant>,
    dirty: bool,
}

impl Live {
    fn connect() -> Result<Self> {
        let (conn, screen_num) =
            x11rb::connect(None).context("нет соединения с X-сервером (DISPLAY)")?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::new(&conn)
            .context("intern атомов")?
            .reply()
            .context("intern атомов (reply)")?;
        // EWMH-совместимый WM обязан вести список окон в порядке стекинга;
        // без него рельефа не будет — честнее не подниматься вовсе.
        let list = conn
            .get_property(
                false,
                root,
                atoms._NET_CLIENT_LIST_STACKING,
                AtomEnum::WINDOW,
                0,
                PROP_LEN,
            )
            .context("_NET_CLIENT_LIST_STACKING")?
            .reply()
            .context("_NET_CLIENT_LIST_STACKING (reply)")?;
        if list.value32().is_none() {
            anyhow::bail!("WM без _NET_CLIENT_LIST_STACKING (не EWMH?)");
        }
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .context("подписка на корень")?
        .check()
        .context("подписка на корень (check)")?;

        Ok(Self {
            conn,
            root,
            atoms,
            tracked: HashSet::new(),
            snap: None,
            built_at: None,
            dirty: true,
        })
    }

    /// Неблокирующий шаг: выгрести события, при изменениях перечитать рельеф.
    /// `Err` — только смерть соединения.
    fn poll(&mut self) -> Result<Option<WorldSnapshot>> {
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(ev)) => self.on_event(ev),
                Ok(None) => break,
                Err(e) => return Err(anyhow!(e).context("poll_for_event")),
            }
        }
        // Тишина дольше HEARTBEAT — перечитываем на всякий случай.
        if self.built_at.is_none_or(|t| t.elapsed() >= HEARTBEAT) {
            self.dirty = true;
        }
        // Коалесценция: свежесобранный рельеф не пересобираем сразу же.
        let recent = self
            .built_at
            .is_some_and(|t| t.elapsed() < REBUILD_MIN_INTERVAL);
        if self.dirty && !recent {
            self.dirty = false;
            self.snap = Some(self.rebuild()?);
            self.built_at = Some(Instant::now());
        }
        Ok(self.snap.clone())
    }

    fn on_event(&mut self, ev: XEvent) {
        match ev {
            XEvent::PropertyNotify(e) if e.window == self.root => {
                if [
                    self.atoms._NET_CLIENT_LIST_STACKING,
                    self.atoms._NET_CURRENT_DESKTOP,
                    self.atoms._NET_WORKAREA,
                    self.atoms._NET_ACTIVE_WINDOW,
                ]
                .contains(&e.atom)
                {
                    self.dirty = true;
                }
            }
            XEvent::PropertyNotify(e) if self.tracked.contains(&e.window) => {
                if [
                    self.atoms._NET_WM_STATE,
                    self.atoms._NET_WM_DESKTOP,
                    self.atoms._NET_FRAME_EXTENTS,
                ]
                .contains(&e.atom)
                {
                    self.dirty = true;
                }
            }
            // Синтетический ConfigureNotify (ICCCM) — окно поехало/выросло.
            XEvent::ConfigureNotify(e) if self.tracked.contains(&e.window) => {
                self.dirty = true;
            }
            XEvent::MapNotify(e) if self.tracked.contains(&e.window) => self.dirty = true,
            XEvent::UnmapNotify(e) if self.tracked.contains(&e.window) => self.dirty = true,
            XEvent::DestroyNotify(e) => {
                if self.tracked.remove(&e.window) {
                    self.dirty = true;
                }
            }
            // Асинхронная BadWindow-гонка (окно умерло между списком и
            // подпиской) — штатно.
            XEvent::Error(e) => log::debug!("worldsense: X11-ошибка запроса: {e:?}"),
            _ => {}
        }
    }

    /// Перечитать рельеф целиком. `Err` — только смерть соединения; окна,
    /// умершие по дороге (BadWindow), просто выпадают из снапшота.
    fn rebuild(&mut self) -> Result<WorldSnapshot> {
        let ids: Vec<Window> = self
            .prop32(
                self.root,
                self.atoms._NET_CLIENT_LIST_STACKING,
                AtomEnum::WINDOW,
            )?
            .unwrap_or_default();

        // Подписка на новые окна (state/геометрия) — до чтения свойств,
        // чтобы не пропустить изменения между чтением и подпиской.
        for &win in &ids {
            if self.tracked.insert(win) {
                // Ошибка отправки = смерть соединения; BadWindow придёт
                // асинхронно и просто зашумит лог на уровне debug.
                self.conn
                    .change_window_attributes(
                        win,
                        &ChangeWindowAttributesAux::new()
                            .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
                    )
                    .context("подписка на окно")?;
            }
        }
        self.tracked.retain(|w| ids.contains(w));

        let current_desktop = self
            .prop32(
                self.root,
                self.atoms._NET_CURRENT_DESKTOP,
                AtomEnum::CARDINAL,
            )?
            .and_then(|v| v.first().copied())
            .unwrap_or(0);
        let active: Option<Window> = self
            .prop32(self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW)?
            .and_then(|v| v.first().copied())
            .filter(|&w| w != 0);
        let work_area = self
            .prop32(self.root, self.atoms._NET_WORKAREA, AtomEnum::CARDINAL)?
            .as_deref()
            .and_then(|v| workarea_for(v, current_desktop));

        let mut windows = Vec::with_capacity(ids.len());
        let mut any_fullscreen = false;
        for &win in &ids {
            let Some(raw) = self.window_raw(win, current_desktop)? else {
                continue;
            };
            // Вежливость D5: прятаться надо от полноэкранного АКТИВНОГО окна
            // (фильм); фоновые дублируются по видимым окнам в parse::filter.
            if active == Some(win) && raw.fullscreen && !raw.minimized {
                any_fullscreen = true;
            }
            windows.push(raw);
        }

        // Порядок в _NET_CLIENT_LIST_STACKING — bottom-to-top, как у
        // KWin-скрипта: parse::filter сам развернёт в top-to-bottom.
        Ok(parse::filter(&RawSnapshot {
            windows,
            work_area,
            any_fullscreen,
        }))
    }

    /// Свойства одного окна → [`RawWindow`]. `Ok(None)` — окно не платформа
    /// (чужой рабочий стол, DOCK/DESKTOP) или умерло по дороге.
    fn window_raw(&self, win: Window, current_desktop: u32) -> Result<Option<RawWindow>> {
        if let Some(desk) = self
            .prop32(win, self.atoms._NET_WM_DESKTOP, AtomEnum::CARDINAL)?
            .and_then(|v| v.first().copied())
        {
            if desk != current_desktop && desk != ALL_DESKTOPS {
                return Ok(None);
            }
        }
        if let Some(types) = self.prop32(win, self.atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM)? {
            if types.contains(&self.atoms._NET_WM_WINDOW_TYPE_DOCK)
                || types.contains(&self.atoms._NET_WM_WINDOW_TYPE_DESKTOP)
            {
                return Ok(None);
            }
        }
        let state = self
            .prop32(win, self.atoms._NET_WM_STATE, AtomEnum::ATOM)?
            .unwrap_or_default();

        // Геометрия: клиентская область в root-координатах + WM-рамка.
        let Some(geo) = reply_ok(self.conn.get_geometry(win).context("get_geometry")?.reply())?
        else {
            return Ok(None); // окно умерло между списком и запросом
        };
        let Some(abs) = reply_ok(
            self.conn
                .translate_coordinates(win, self.root, 0, 0)
                .context("translate_coordinates")?
                .reply(),
        )?
        else {
            return Ok(None);
        };
        let extents = self
            .prop32(win, self.atoms._NET_FRAME_EXTENTS, AtomEnum::CARDINAL)?
            .filter(|v| v.len() >= 4)
            .map(|v| [v[0], v[1], v[2], v[3]]);
        let (x, y, w, h) = frame_rect(
            i32::from(abs.dst_x),
            i32::from(abs.dst_y),
            u32::from(geo.width),
            u32::from(geo.height),
            extents,
        );

        let cls = self.wm_class(win)?;
        Ok(Some(RawWindow {
            iid: format!("0x{win:x}"),
            x,
            y,
            w,
            h,
            minimized: state.contains(&self.atoms._NET_WM_STATE_HIDDEN),
            fullscreen: state.contains(&self.atoms._NET_WM_STATE_FULLSCREEN),
            skip_taskbar: state.contains(&self.atoms._NET_WM_STATE_SKIP_TASKBAR),
            cls,
        }))
    }

    /// 32-битное свойство. `Ok(None)` — свойства нет/не тот формат/окно
    /// умерло; `Err` — соединение мертво.
    fn prop32(&self, win: Window, prop: Atom, ty: AtomEnum) -> Result<Option<Vec<u32>>> {
        let cookie = self
            .conn
            .get_property(false, win, prop, ty, 0, PROP_LEN)
            .context("get_property")?;
        Ok(reply_ok(cookie.reply())?.and_then(|r| r.value32().map(|it| it.collect::<Vec<u32>>())))
    }

    /// `WM_CLASS` (вторая строка — класс). Пусто, если свойства нет.
    fn wm_class(&self, win: Window) -> Result<String> {
        let cookie = self
            .conn
            .get_property(
                false,
                win,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                0,
                PROP_LEN,
            )
            .context("WM_CLASS")?;
        Ok(reply_ok(cookie.reply())?
            .map(|r| wm_class_from_bytes(&r.value))
            .unwrap_or_default())
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // Снятие подписок не требуется: маски событий — per-client,
        // сервер забывает их вместе с соединением.
        let _ = self.conn.flush();
    }
}

/// Ошибка реплая: X11-ошибка (обычно BadWindow в гонке) — не смертельно,
/// `Ok(None)`; всё остальное — соединение мертво.
fn reply_ok<T>(r: std::result::Result<T, ReplyError>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(ReplyError::ConnectionError(e)) => Err(anyhow!(e).context("соединение с X-сервером")),
    }
}

/// `_NET_WORKAREA` — по 4 CARDINAL (x, y, w, h) на рабочий стол; берём свой,
/// фолбэк — первый (WM с одним столом может прислать ровно одну четвёрку).
fn workarea_for(values: &[u32], desktop: u32) -> Option<RawWorkArea> {
    let idx = (desktop as usize).checked_mul(4)?;
    let chunk = values.get(idx..idx + 4).or_else(|| values.get(0..4))?;
    Some(RawWorkArea {
        y: chunk[1] as f32,
        h: chunk[3] as f32,
    })
}

/// Клиентская область + `_NET_FRAME_EXTENTS` ([left, right, top, bottom]) →
/// прямоугольник WM-рамки: питомец стоит на заголовке окна.
fn frame_rect(ax: i32, ay: i32, w: u32, h: u32, extents: Option<[u32; 4]>) -> (f32, f32, f32, f32) {
    let [l, r, t, b] = extents.unwrap_or([0; 4]);
    (
        (ax - l as i32) as f32,
        (ay - t as i32) as f32,
        (w + l + r) as f32,
        (h + t + b) as f32,
    )
}

/// `WM_CLASS` — две NUL-терминированные строки (instance, class); наружу
/// отдаём класс (как resourceClass у KWin).
fn wm_class_from_bytes(value: &[u8]) -> String {
    let mut parts = value.split(|&b| b == 0);
    let instance = parts.next().unwrap_or_default();
    let class = parts.next().filter(|c| !c.is_empty()).unwrap_or(instance);
    String::from_utf8_lossy(class).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workarea_beryot_svoy_stol_s_folbekom() {
        // Два стола: у второго рабочая область ниже (панель сверху).
        let v = [0, 0, 1920, 1056, 0, 24, 1920, 1032];
        let wa = workarea_for(&v, 1).unwrap();
        assert_eq!((wa.y, wa.h), (24.0, 1032.0));
        // Стол за пределами массива — фолбэк на первый.
        let wa = workarea_for(&v, 7).unwrap();
        assert_eq!((wa.y, wa.h), (0.0, 1056.0));
        // Совсем пусто — None.
        assert!(workarea_for(&[], 0).is_none());
    }

    #[test]
    fn frame_rect_rasshiryaet_do_ramki() {
        // Клиент в (10, 30) размером 100x80, рамка: 2 слева/справа,
        // 24 сверху, 2 снизу.
        assert_eq!(
            frame_rect(10, 30, 100, 80, Some([2, 2, 24, 2])),
            (8.0, 6.0, 104.0, 106.0)
        );
        // Без _NET_FRAME_EXTENTS — клиентская область как есть.
        assert_eq!(frame_rect(10, 30, 100, 80, None), (10.0, 30.0, 100.0, 80.0));
    }

    #[test]
    fn wm_class_paren_instance_class() {
        assert_eq!(wm_class_from_bytes(b"xterm\0XTerm\0"), "XTerm");
        // Класса нет — берём instance.
        assert_eq!(wm_class_from_bytes(b"driftling\0"), "driftling");
        assert_eq!(wm_class_from_bytes(b"driftling"), "driftling");
        assert_eq!(wm_class_from_bytes(b""), "");
    }
}
