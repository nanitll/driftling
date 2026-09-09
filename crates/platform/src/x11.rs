//! X11-бэкенд (ROADMAP D3): override-redirect ARGB-окно размером с границы
//! сцены, XShape INPUT-регион, физические пиксели.
//!
//! Зеркалит контракт Wayland-бэкенда, но архитектура проще — X11 дёшев в
//! перемещении окон:
//!
//! - **Окно**: одно override-redirect (WM его не трогает: без рамки, без
//!   анимаций, поверх всех) InputOutput-окно с 32-битным TrueColor-визуалом.
//!   Каждый кадр окно двигается/ресайзится под объединённые границы сцены
//!   (`ConfigureWindow` — пара десятков байт), содержимое перерисовывается
//!   только при смене кадра (общий dirty-check из [`crate::render`]).
//!   Прозрачность честная только под композитором (KWin/Xfwm/picom);
//!   без него ARGB-окно рисуется на чёрном — деградация, о ней warn.
//! - **Ввод**: XShape `SK::INPUT` = input_rects сцены в координатах окна;
//!   мышь вне спрайта проваливается сквозь окно. Во время зажатой кнопки
//!   X11 сам даёт implicit grab — Motion приходит и вне фигуры, Release
//!   гарантирован, синтетика «Leave = Release» (как в Wayland) не нужна.
//!   ПКМ → [`Event::PointerMenu`]. Координаты берём из `root_x/root_y` —
//!   они глобальные, переводим в локальные вычитанием origin выхода.
//! - **Поверх всех**: у override-redirect окна нет _NET_WM_STATE-гарантий,
//!   поэтому поднимаем его сами при VisibilityNotify (окно перекрыли) с
//!   троттлингом против raise-войн. EWMH-подсказки (DOCK, ABOVE, skip
//!   taskbar/pager) всё равно проставляем — их читают композиторы (тени)
//!   и панели.
//! - **Выход**: размер и позиция primary-выхода из RandR (фолбэк — корневое
//!   окно целиком); origin уходит в [`Event::OutputGeometry`], окно ставится
//!   в `origin + сцена`. Один выход, как и в Wayland. TODO(D6): мультимонитор.
//!   Смена разрешения — RandR ScreenChangeNotify → передоставка геометрии.
//! - **Темп** (ТД-3): свой цикл poll(2) по fd соединения с таймаутом из
//!   `App::pace()` — Active 33 мс / Calm 200 мс / Drowsy 1000 мс; событие
//!   будит poll немедленно, эскалация темпа тикает без ожидания таймера.
//! - **Живучесть** (ТД-2): supervision из [`crate::supervise`] — потеря
//!   соединения = [`Event::OutputLost`] + пересоздание с бэкоффом; если
//!   ПЕРВАЯ попытка не поднялась (нет X-сервера) — ошибка наружу.
//!
//! HiDPI: X11 не знает логических пикселей — scale всегда 1, размер питомца
//! задаётся настройкой приложения. Дробный масштаб — не про X11.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use driftling_core::Vec2;
use rustix::event::{PollFd, PollFlags, Timespec};
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::randr::{self, ConnectionExt as _, NotifyMask};
use x11rb::protocol::shape::{self, ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, ImageFormat, ImageOrder, PropMode, Rectangle, StackMode,
    Visibility, VisualClass, Window, WindowClass,
};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::render::{compose, content_key, local_input_rects, scene_bounds, ContentKey};
use crate::supervise::{self, pace_delay, Outcome};
use crate::{App, Event, Scene};

x11rb::atom_manager! {
    /// EWMH-атомы, интернируемые одним раунд-трипом на попытку.
    Atoms:
    AtomsCookie {
        UTF8_STRING,
        _NET_WM_NAME,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_STICKY,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
    }
}

/// Между двумя поднятиями окна по VisibilityNotify — не чаще этого:
/// два самоподнимающихся окна иначе устраивают raise-войну на 100% CPU.
const RAISE_BACKOFF: Duration = Duration::from_millis(500);

/// Потолок одного PutImage: страховка на случай сервера без BIG-REQUESTS
/// (лимит классического запроса 256 КБ; наши кадры обычно много меньше).
const PUT_CHUNK_BYTES: usize = 128 * 1024;

/// Запустить X11-бэкенд; возвращается после запроса выхода приложением.
/// Supervision-семантика — как у Wayland (см. [`supervise::run`]).
pub fn run(app: impl App + 'static) -> Result<()> {
    supervise::run(app, "X11", run_attempt)
}

/// Одна попытка: соединение + окно + event loop.
/// `App` всегда возвращается вызывающему — он переживает все попытки.
fn run_attempt(app: Box<dyn App>, clock: Instant) -> (Box<dyn App>, Outcome) {
    let mut slot = Some(app);
    let outcome = attempt_inner(&mut slot, clock);
    let app = slot.take().expect("App всегда возвращается из попытки");
    (app, outcome)
}

fn attempt_inner(app_slot: &mut Option<Box<dyn App>>, clock: Instant) -> Outcome {
    macro_rules! setup {
        ($expr:expr, $ctx:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => return Outcome::SetupFailed(anyhow!(e).context($ctx)),
            }
        };
    }

    let renderer = setup!(Renderer::connect(), "X11-сессия не поднялась");
    let mut backend = Backend {
        app: app_slot.take().expect("App передан в попытку"),
        rend: renderer,
        clock,
        next_tick: Instant::now(),
        exit: false,
        lost: None,
    };
    log::info!(
        "X11: соединение установлено, окно создано (глубина {}, выход {:.0}x{:.0} @ ({}, {}))",
        backend.rend.depth,
        backend.rend.out_size.0,
        backend.rend.out_size.1,
        backend.rend.origin.0,
        backend.rend.origin.1
    );

    backend.push_geometry();
    backend.run_loop();

    let outcome = if backend.exit {
        Outcome::Exit
    } else if let Some(reason) = backend.lost.take() {
        Outcome::Lost(reason)
    } else {
        Outcome::Lost(anyhow!("event loop остановился без причины"))
    };
    *app_slot = Some(backend.app);
    outcome
}

/// Состояние бэкенда: приложение + рендер (владеет соединением).
struct Backend {
    app: Box<dyn App>,
    rend: Renderer,
    /// Общие для всех попыток монотонные часы (`now` приложения).
    clock: Instant,
    /// Дедлайн следующего тика симуляции (адаптивный темп ТД-3).
    next_tick: Instant,
    exit: bool,
    /// Причина потери сессии; взведение останавливает цикл с ретраем.
    lost: Option<anyhow::Error>,
}

impl Backend {
    /// Монотонные секунды от старта демона (не попытки).
    fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// Остановить цикл с последующим переподключением.
    fn fail(&mut self, reason: anyhow::Error) {
        if self.lost.is_none() {
            self.lost = Some(reason);
        }
    }

    fn done(&self) -> bool {
        self.exit || self.lost.is_some()
    }

    /// Отдать событие приложению; false от него — сигнал завершения.
    fn deliver(&mut self, ev: Event) {
        let now = self.now();
        if !self.app.event(ev, now) || self.app.wants_exit() {
            self.exit = true;
            return;
        }
        // Событие могло разбудить приложение (сон → drag) — ускоряемся:
        // если новый темп короче остатка до дедлайна, тикаем немедленно.
        self.maybe_escalate();
    }

    fn maybe_escalate(&mut self) {
        let now = Instant::now();
        if now + pace_delay(self.app.pace()) < self.next_tick {
            self.next_tick = now;
        }
    }

    /// Главный цикл: тик по расписанию + события по готовности fd.
    fn run_loop(&mut self) {
        while !self.done() {
            // 1) Тик симуляции, если пришло время.
            if Instant::now() >= self.next_tick {
                self.tick_and_draw();
                if self.done() {
                    break;
                }
                self.next_tick = Instant::now() + pace_delay(self.app.pace());
            }

            // 2) События, попавшие во внутреннюю очередь x11rb во время
            // реплаев тика: fd их больше не «подсветит» — добираем сейчас,
            // иначе клик мог бы ждать до секунды в Drowsy.
            self.drain_events();
            if self.done() {
                break;
            }

            // 3) Ждать: событие на fd или дедлайн тика.
            let timeout = self.next_tick.saturating_duration_since(Instant::now());
            if !timeout.is_zero() {
                if let Err(e) = self.rend.conn.flush() {
                    self.fail(anyhow!(e).context("flush"));
                    break;
                }
                match wait_readable(&self.rend.conn, timeout) {
                    Ok(()) => {}
                    Err(e) => {
                        self.fail(e.context("poll по fd X-соединения"));
                        break;
                    }
                }
                self.drain_events();
            }
        }
    }

    /// Шаг симуляции + синхронизация окна со сценой.
    fn tick_and_draw(&mut self) {
        let now = self.now();
        // Scene заимствует self.app; rend — отдельное поле, конфликта нет.
        let scene = self.app.tick(now);
        let result = self.rend.sync(&scene);
        drop(scene);
        if let Err(e) = result {
            self.fail(e);
            return;
        }
        if self.app.wants_exit() {
            self.exit = true;
        }
    }

    /// Выгрести все готовые события (без блокировки).
    fn drain_events(&mut self) {
        loop {
            if self.done() {
                return;
            }
            match self.rend.conn.poll_for_event() {
                Ok(Some(ev)) => self.handle(ev),
                Ok(None) => return,
                Err(e) => {
                    self.fail(anyhow!(e).context("соединение с X-сервером потеряно"));
                    return;
                }
            }
        }
    }

    /// Глобальные (root) координаты → локальные логические координаты выхода.
    fn pos(&self, root_x: i16, root_y: i16) -> Vec2 {
        Vec2::new(
            root_x as f32 - self.rend.origin.0 as f32,
            root_y as f32 - self.rend.origin.1 as f32,
        )
    }

    fn handle(&mut self, ev: XEvent) {
        match ev {
            // Сервер просит перерисовать (нет композитора/распакованный
            // регион) — повторяем последний собранный кадр.
            XEvent::Expose(e) if e.window == self.rend.win && e.count == 0 => {
                if let Err(err) = self.rend.repaint() {
                    self.fail(err);
                }
            }
            XEvent::ButtonPress(e) => match e.detail {
                1 => self.deliver(Event::PointerPress(self.pos(e.root_x, e.root_y))),
                // ПКМ — контекстное меню (ТД-9).
                3 => self.deliver(Event::PointerMenu(self.pos(e.root_x, e.root_y))),
                _ => {}
            },
            XEvent::ButtonRelease(e) if e.detail == 1 => {
                self.deliver(Event::PointerRelease(self.pos(e.root_x, e.root_y)));
            }
            XEvent::MotionNotify(e) => {
                self.deliver(Event::PointerMotion(self.pos(e.root_x, e.root_y)));
            }
            // Enter несёт позицию — отдаём как движение (как в Wayland).
            XEvent::EnterNotify(e) => {
                self.deliver(Event::PointerMotion(self.pos(e.root_x, e.root_y)));
            }
            // Нас перекрыли — поднимаемся (с троттлингом от raise-войн).
            XEvent::VisibilityNotify(e)
                if e.window == self.rend.win && e.state != Visibility::UNOBSCURED =>
            {
                if let Err(err) = self.rend.raise_throttled() {
                    self.fail(err);
                }
            }
            // Кто-то убил наше окно (xkill) — пересоздаёмся супервизором.
            XEvent::DestroyNotify(e) if e.window == self.rend.win => {
                self.fail(anyhow!("окно оверлея уничтожено извне"));
            }
            // Смена разрешения/расположения выходов.
            XEvent::RandrScreenChangeNotify(_) => self.push_geometry(),
            // Асинхронная ошибка запроса (например, BadWindow в гонке) —
            // не смертельно, соединение живо.
            XEvent::Error(e) => {
                log::debug!("X11-ошибка запроса: {e:?}");
            }
            _ => {}
        }
    }

    /// Перечитать геометрию выхода и отдать приложению, если изменилась.
    fn push_geometry(&mut self) {
        match self.rend.query_output() {
            Ok(changed) => {
                if changed {
                    log::info!(
                        "выход: {:.0}x{:.0} в глобальной точке ({}, {})",
                        self.rend.out_size.0,
                        self.rend.out_size.1,
                        self.rend.origin.0,
                        self.rend.origin.1
                    );
                }
                // Первая доставка обязательна даже без изменений.
                if changed || !self.rend.geometry_sent {
                    self.rend.geometry_sent = true;
                    self.deliver(Event::OutputGeometry {
                        width: self.rend.out_size.0,
                        height: self.rend.out_size.1,
                        origin: Vec2::new(self.rend.origin.0 as f32, self.rend.origin.1 as f32),
                    });
                }
            }
            Err(e) => self.fail(e.context("геометрия выхода")),
        }
    }
}

/// Подождать данных на fd X-соединения (или таймаут). EINTR — обычное
/// пробуждение (SIGTERM ставит флаг, его проверит ближайший тик).
fn wait_readable(conn: &RustConnection, timeout: Duration) -> Result<()> {
    let timeout = Timespec::try_from(timeout).context("таймаут poll")?;
    let stream = conn.stream();
    let mut fds = [PollFd::new(&stream, PollFlags::IN)];
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(_) => Ok(()),
        Err(rustix::io::Errno::INTR) => Ok(()),
        Err(e) => Err(anyhow!(e)),
    }
}

/// Всё, что нужно для отрисовки сцены; владеет соединением.
struct Renderer {
    conn: RustConnection,
    root: Window,
    win: Window,
    gc: u32,
    /// Глубина окна: 32 (ARGB) или глубина корня (фолбэк без прозрачности).
    depth: u8,
    /// Байт-порядок изображений сервера (LSB почти всегда).
    byte_order: ImageOrder,
    /// XShape доступен; без него мышь ловит весь прямоугольник окна.
    shape_ok: bool,
    /// RandR доступен; без него выход = корневое окно целиком.
    randr_ok: bool,
    /// Позиция выхода в root-координатах (primary CRTC); сцена локальна
    /// выходу, окно ставится в `origin + сцена`.
    origin: (i32, i32),
    /// Размер выхода в пикселях.
    out_size: (f32, f32),
    /// Первый Event::OutputGeometry уже доставлен.
    geometry_sent: bool,

    mapped: bool,
    /// В окне есть содержимое (canvas собран).
    visible: bool,
    /// Содержимое последнего нарисованного кадра (dirty-check).
    last_content: Option<ContentKey>,
    /// Последняя выставленная геометрия окна (root-координаты).
    last_geom: Option<(i32, i32, u32, u32)>,
    /// Последний выставленный input-регион (координаты окна).
    last_input: Option<Vec<(i32, i32, i32, i32)>>,
    /// Последний собранный кадр (LE ARGB) — для Expose-перерисовки.
    canvas: Vec<u8>,
    canvas_size: (u32, u32),
    last_raise: Option<Instant>,
}

impl Renderer {
    /// Соединение + окно + расширения. Ошибка любого шага — попытка не
    /// состоялась (SetupFailed решает supervision-цикл).
    fn connect() -> Result<Self> {
        let (conn, screen_num) =
            x11rb::connect(None).context("нет соединения с X-сервером (DISPLAY)")?;
        let setup = conn.setup();
        let byte_order = setup.image_byte_order;
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let root_depth = screen.root_depth;
        let root_visual = screen.root_visual;

        // 32-битный TrueColor-визуал — честная прозрачность (под
        // композитором). Нет такого — глубина корня, спрайт на чёрном.
        let argb = screen.allowed_depths.iter().find_map(|d| {
            (d.depth == 32)
                .then(|| {
                    d.visuals
                        .iter()
                        .find(|v| v.class == VisualClass::TRUE_COLOR)
                })
                .flatten()
                .map(|v| v.visual_id)
        });
        let (depth, visual) = match argb {
            Some(v) => (32, v),
            None => {
                log::warn!("X11: нет 32-битного визуала — питомец без прозрачности");
                (root_depth, root_visual)
            }
        };

        let atoms = Atoms::new(&conn)
            .context("intern атомов")?
            .reply()
            .context("intern атомов (reply)")?;

        let win = conn.generate_id().context("id окна")?;
        let colormap = conn.generate_id().context("id колормапа")?;
        // Свой колормап обязателен, когда визуал отличается от родительского
        // (иначе BadMatch); border_pixel — по той же причине.
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)
            .context("колормап")?;
        let aux = CreateWindowAux::new()
            // WM окно не управляет: без рамки, decorations и перехвата.
            .override_redirect(1)
            .background_pixel(0)
            .border_pixel(0)
            .colormap(colormap)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::ENTER_WINDOW
                    | EventMask::VISIBILITY_CHANGE
                    | EventMask::STRUCTURE_NOTIFY,
            );
        // Стартуем 1x1 вне экрана и без map: реальную геометрию даст сцена.
        conn.create_window(
            depth,
            win,
            root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &aux,
        )
        .context("окно оверлея")?;

        // Идентичность окна (SHIPPING.md, F1): WM_CLASS = (instance
        // «driftling», class = app-id) — класс совпадает со StartupWMClass
        // из dist/*.desktop; по подстроке «driftling» worldsense отсекает
        // собственные окна питомца.
        let wm_class = format!("driftling\0{}\0", driftling_core::APP_ID);
        conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            wm_class.as_bytes(),
        )
        .context("WM_CLASS")?;
        conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"Driftling",
        )
        .context("WM_NAME")?;
        conn.change_property8(
            PropMode::REPLACE,
            win,
            atoms._NET_WM_NAME,
            atoms.UTF8_STRING,
            b"Driftling",
        )
        .context("_NET_WM_NAME")?;
        // EWMH-подсказки: для override-redirect WM их не применяет, но
        // композиторы (тени у DOCK не рисуют) и панели — читают.
        conn.change_property32(
            PropMode::REPLACE,
            win,
            atoms._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[atoms._NET_WM_WINDOW_TYPE_DOCK],
        )
        .context("_NET_WM_WINDOW_TYPE")?;
        conn.change_property32(
            PropMode::REPLACE,
            win,
            atoms._NET_WM_STATE,
            AtomEnum::ATOM,
            &[
                atoms._NET_WM_STATE_ABOVE,
                atoms._NET_WM_STATE_STICKY,
                atoms._NET_WM_STATE_SKIP_TASKBAR,
                atoms._NET_WM_STATE_SKIP_PAGER,
            ],
        )
        .context("_NET_WM_STATE")?;

        let gc = conn.generate_id().context("id GC")?;
        conn.create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0))
            .context("GC")?;

        let shape_ok = conn
            .extension_information(shape::X11_EXTENSION_NAME)
            .context("запрос расширений")?
            .is_some();
        if !shape_ok {
            log::warn!("X11: нет расширения SHAPE — мышь ловит весь прямоугольник спрайта");
        }
        let randr_ok = conn
            .extension_information(randr::X11_EXTENSION_NAME)
            .context("запрос расширений")?
            .is_some();
        if randr_ok {
            // Смена разрешения/выходов будит push_geometry.
            conn.randr_select_input(root, NotifyMask::SCREEN_CHANGE)
                .context("RandR select_input")?;
        }

        // Прозрачность работает только под композитором — предупредим.
        let cm_sel = format!("_NET_WM_CM_S{screen_num}");
        let cm_atom = conn
            .intern_atom(false, cm_sel.as_bytes())
            .context("intern _NET_WM_CM_Sn")?
            .reply()
            .context("intern _NET_WM_CM_Sn (reply)")?
            .atom;
        let cm_owner = conn
            .get_selection_owner(cm_atom)
            .context("владелец селекции композитора")?
            .reply()
            .context("владелец селекции композитора (reply)")?
            .owner;
        if cm_owner == x11rb::NONE {
            log::warn!(
                "X11: композитор не запущен ({cm_sel} никем не занята) — прозрачности не будет"
            );
        }

        let mut renderer = Self {
            conn,
            root,
            win,
            gc,
            depth,
            byte_order,
            shape_ok,
            randr_ok,
            origin: (0, 0),
            out_size: (0.0, 0.0),
            geometry_sent: false,
            mapped: false,
            visible: false,
            last_content: None,
            last_geom: None,
            last_input: None,
            canvas: Vec::new(),
            canvas_size: (0, 0),
            last_raise: None,
        };
        renderer.query_output().context("геометрия выхода")?;
        Ok(renderer)
    }

    /// Опросить размер/позицию выхода: primary CRTC из RandR, фолбэк —
    /// корневое окно. true — геометрия изменилась. TODO(D6): мультимонитор
    /// (по окну на CRTC, переходы между выходами).
    fn query_output(&mut self) -> Result<bool> {
        let (origin, size) = match self.primary_crtc() {
            Some((pos, (w, h))) if w > 0 && h > 0 => (pos, (w, h)),
            _ => {
                let g = self
                    .conn
                    .get_geometry(self.root)
                    .context("геометрия корня")?
                    .reply()
                    .context("геометрия корня (reply)")?;
                ((0, 0), (u32::from(g.width), u32::from(g.height)))
            }
        };
        let out_size = (size.0 as f32, size.1 as f32);
        let changed = self.origin != origin || self.out_size != out_size;
        self.origin = origin;
        self.out_size = out_size;
        Ok(changed)
    }

    /// Позиция и размер primary-CRTC, если RandR есть и primary назначен.
    /// Ошибки реплаев здесь не фатальны — фолбэк на корень.
    fn primary_crtc(&self) -> Option<((i32, i32), (u32, u32))> {
        if !self.randr_ok {
            return None;
        }
        let output = self
            .conn
            .randr_get_output_primary(self.root)
            .ok()?
            .reply()
            .ok()?
            .output;
        if output == 0 {
            return None;
        }
        let crtc = self
            .conn
            .randr_get_output_info(output, 0)
            .ok()?
            .reply()
            .ok()?
            .crtc;
        if crtc == 0 {
            return None;
        }
        let info = self.conn.randr_get_crtc_info(crtc, 0).ok()?.reply().ok()?;
        Some((
            (i32::from(info.x), i32::from(info.y)),
            (u32::from(info.width), u32::from(info.height)),
        ))
    }

    /// Привести окно в соответствие сцене: подвинуть/отресайзить (каждый
    /// кадр — X11 это дёшево), перерисовать содержимое (только по dirty),
    /// обновить input-фигуру (только при изменении).
    fn sync(&mut self, scene: &Scene<'_>) -> Result<()> {
        let Some((bx, by, bw, bh)) = scene_bounds(scene) else {
            return self.hide();
        };
        // Сцена локальна выходу; окно живёт в root-координатах.
        let geom = (self.origin.0 + bx, self.origin.1 + by, bw, bh);
        if self.last_geom != Some(geom) {
            let aux = ConfigureWindowAux::new()
                .x(clamp_i16(geom.0))
                .y(clamp_i16(geom.1))
                .width(bw.clamp(1, u16::MAX.into()))
                .height(bh.clamp(1, u16::MAX.into()));
            self.conn
                .configure_window(self.win, &aux)
                .context("configure окна")?;
            self.last_geom = Some(geom);
        }

        // X11-бэкенд рисует сцену целиком в одно окно (кластеров нет).
        let all: Vec<usize> = (0..scene.sprites.len()).collect();
        let key = content_key(scene, &all, (bx, by), 1);
        if !self.visible || self.last_content.as_ref() != Some(&key) {
            self.canvas.resize((bw * bh * 4) as usize, 0);
            compose(&mut self.canvas, (bw, bh), scene, &all, (bx, by), 1);
            self.canvas_size = (bw, bh);
            self.repaint()?;
            self.last_content = Some(key);
            self.visible = true;
        }

        let input = local_input_rects(&scene.input_rects, (bx, by));
        if self.last_input.as_ref() != Some(&input) {
            self.set_input_shape(&input)?;
            self.last_input = Some(input);
        }

        if !self.mapped {
            self.conn.map_window(self.win).context("map окна")?;
            self.mapped = true;
            self.raise()?;
        }
        Ok(())
    }

    /// Спрятать питомца: unmap. Размапленное окно не рисуется и не ловит
    /// ввод — пустая сцена не стоит ничего.
    fn hide(&mut self) -> Result<()> {
        if self.mapped {
            self.conn.unmap_window(self.win).context("unmap окна")?;
            self.mapped = false;
        }
        self.visible = false;
        self.last_content = None;
        Ok(())
    }

    /// Отправить текущий canvas в окно. Ломтями по строкам — влезает и в
    /// классический лимит запроса, если сервер без BIG-REQUESTS.
    fn repaint(&mut self) -> Result<()> {
        let (w, h) = self.canvas_size;
        if w == 0 || h == 0 || self.canvas.is_empty() {
            return Ok(());
        }
        let data = to_wire(&self.canvas, self.byte_order);
        let stride = (w * 4) as usize;
        let rows_per_chunk = (PUT_CHUNK_BYTES / stride).max(1) as u32;
        let mut y = 0u32;
        while y < h {
            let rows = rows_per_chunk.min(h - y);
            let start = y as usize * stride;
            let end = start + rows as usize * stride;
            self.conn
                .put_image(
                    ImageFormat::Z_PIXMAP,
                    self.win,
                    self.gc,
                    w as u16,
                    rows as u16,
                    0,
                    y as i16,
                    0,
                    self.depth,
                    &data[start..end],
                )
                .context("put_image")?;
            y += rows;
        }
        Ok(())
    }

    /// Input-фигура окна = прямоугольники сцены в координатах окна; мимо
    /// них клики проваливаются к окнам ниже. Пустой список = окно-призрак.
    fn set_input_shape(&mut self, rects: &[(i32, i32, i32, i32)]) -> Result<()> {
        if !self.shape_ok {
            return Ok(());
        }
        let rects: Vec<Rectangle> = rects
            .iter()
            .filter(|&&(_, _, w, h)| w > 0 && h > 0)
            .map(|&(x, y, w, h)| Rectangle {
                x: clamp_i16(x) as i16,
                y: clamp_i16(y) as i16,
                width: w.min(i32::from(u16::MAX)) as u16,
                height: h.min(i32::from(u16::MAX)) as u16,
            })
            .collect();
        self.conn
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                self.win,
                0,
                0,
                &rects,
            )
            .context("input-фигура")?;
        Ok(())
    }

    fn raise(&mut self) -> Result<()> {
        self.conn
            .configure_window(
                self.win,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .context("raise окна")?;
        self.last_raise = Some(Instant::now());
        Ok(())
    }

    /// Подъём по VisibilityNotify — с бэкоффом против raise-войн.
    fn raise_throttled(&mut self) -> Result<()> {
        if !self.mapped {
            return Ok(());
        }
        if let Some(t) = self.last_raise {
            if t.elapsed() < RAISE_BACKOFF {
                return Ok(());
            }
        }
        self.raise()
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // Вежливость к живому серверу: окно исчезает сразу, а не когда
        // сервер сам заметит закрытый сокет.
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }
}

/// ConfigureWindow принимает INT16 — за пределами клампим (окно у края
/// гигантского виртуального экрана лучше прижать, чем завернуть по модулю).
fn clamp_i16(v: i32) -> i32 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX))
}

/// LE-ARGB-канвас → байты для PutImage: на LSB-first сервере — как есть,
/// на MSB-first каждый пиксель разворачивается.
fn to_wire(canvas: &[u8], order: ImageOrder) -> std::borrow::Cow<'_, [u8]> {
    if order == ImageOrder::LSB_FIRST {
        return std::borrow::Cow::Borrowed(canvas);
    }
    let mut out = canvas.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.reverse();
    }
    std::borrow::Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_wire_lsb_is_borrowed_passthrough() {
        let canvas = [0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55];
        let wire = to_wire(&canvas, ImageOrder::LSB_FIRST);
        assert!(matches!(wire, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*wire, &canvas);
    }

    #[test]
    fn to_wire_msb_reverses_each_pixel() {
        // LE-байты B,G,R,A → на MSB-сервере A,R,G,B.
        let canvas = [0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55];
        let wire = to_wire(&canvas, ImageOrder::MSB_FIRST);
        assert_eq!(&*wire, &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    }

    #[test]
    fn clamp_i16_saturates() {
        assert_eq!(clamp_i16(100), 100);
        assert_eq!(clamp_i16(-40000), i32::from(i16::MIN));
        assert_eq!(clamp_i16(70000), i32::from(i16::MAX));
    }
}
