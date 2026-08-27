//! Wayland-бэкенд: прозрачный слой-якорь + wl_subsurface под питомца,
//! wl_shm рендер.
//!
//! Архитектура после переработки энергобюджета (аудит ТД-3, ТД-4; модель
//! доказана wl_shimeji, см. docs/RESEARCH.md; реализация clean-room):
//!
//! - **Якорь**: прозрачный `Layer::Overlay`-surface с ПУСТЫМ input region,
//!   РАСТЯЖИМЫЙ (фаза D): пока питомец на экране — на весь выход (как у
//!   wl_shimeji; полноэкранный слой держит композицию включённой — KWin при
//!   прямом сканауте накрытого окном выхода не рисовал субповерхности
//!   1x1-якоря и не слал им frame callback'и), при пустой сцене — 1x1
//!   (сканаут возвращается композитору, D5). Пиксели якоря перерисовываются
//!   только по configure; в остальном — пустые коммиты, применяющие позицию
//!   субповерхности.
//! - **Питомец**: `wl_subsurface` якоря в режиме desync со своим маленьким
//!   буфером (границы спрайта * масштаб). Перемещение — дешёвая пара
//!   `wl_subsurface.set_position(x, y)` + коммит якоря, без configure-циклов.
//!   Субповерхность легально выходит за границы 1x1-родителя в скрытом
//!   режиме. Известный нюанс: Hyprland исторически клипует субповерхности
//!   по родителю; при полноэкранном якоре это не мешает, а сжатый якорь
//!   показывают без питомца — лечится в D3, не здесь.
//! - **Dirty-check**: буфер перерисовывается только когда сменилось
//!   содержимое кадра (указатель пикселей/размер/зеркало/масштаб — см.
//!   [`ContentKey`]); чистое перемещение не рисует ни пикселя, а неподвижный
//!   спящий питомец не стоит вообще ничего.
//! - **Адаптивный таймер** (ТД-3): после каждого тика таймер перевзводится по
//!   `App::pace()` — Active 33 мс / Calm 200 мс / Drowsy 1000 мс. События
//!   указателя будят таймер немедленно (сон → drag без секундной задержки).
//!   Обратная сторона: IPC-очередь приложения дренируется в tick, поэтому в
//!   Drowsy команда `ctl summon` может ждать до ~1 с — осознанный размен.
//! - **Размер выхода**: configure слоя о выходе ничего не говорит в
//!   1x1-режиме, поэтому геометрия всегда берётся из [`OutputState`] того
//!   выхода, куда композитор посадил якорь (`surface_enter`), с фолбэком
//!   на текущий видеорежим / масштаб.
//! - **Ввод** (ТД-5, ТД-9, ТД-24): события приходят на поверхность питомца в
//!   её локальных координатах и переводятся в логические экранные прибавлением
//!   позиции субповерхности. Пока кнопка зажата, Motion доставляется даже вне
//!   спрайта (implicit grab); `Leave` при зажатой кнопке = PointerRelease в
//!   последней известной точке (фикс вечного Dragged). ПКМ → PointerMenu.
//! - **Живучесть** (ТД-2): `run()` — supervision-цикл. Потеря соединения или
//!   закрытие слоя доставляет приложению `Event::OutputLost` и пересоздаёт
//!   сессию с бэкоффом 1с→2с→5с→10с→30с (сброс после 60 с стабильной работы).
//!   `App` живёт в `run()` и переезжает между попытками.
//!
//! TODO(D6): дробный масштаб (wp_fractional_scale_v1 + wp_viewporter) — пока
//! только целочисленный `wl_surface.set_buffer_scale`, на 125–150% KDE спрайт
//! рисуется с округлением масштаба.
//!
//! Каркас (registry/output/seat/shm-хендлеры и delegate-макросы) следует
//! examples/simple_layer.rs из smithay-client-toolkit v0.19.2.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use driftling_core::sprite::Frame;
use driftling_core::{Rect, Vec2};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_subcompositor,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle, LoopSignal, RegistrationToken,
        },
        calloop_wayland_source::WaylandSource,
        client::{
            globals::registry_queue_init,
            protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_subsurface, wl_surface},
            Connection, QueueHandle,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT, BTN_RIGHT},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
    subcompositor::SubcompositorState,
};

use crate::{App, Event, Pace, Scene};

/// Периоды адаптивного таймера симуляции (энергобюджет ТЗ §7).
/// Страховка от голодания frame callback'ов: дросселирование перерисовок
/// по callback'у экономит энергию, но KWin 6.3 на выходе, целиком накрытом
/// максимизированным окном (режим прямого сканаута), может вообще не слать
/// callback'и нашей overlay-субповерхности — содержимое замерзало бы на
/// первом кадре навсегда (диагностировано живьём в фазе D: ровно один attach
/// за минуты жизни). Если callback молчит дольше этого срока, рисуем без
/// него: собственный damage заставляет композитор вернуться к композиции.
const FRAME_CB_FALLBACK: Duration = Duration::from_millis(250);

const ACTIVE_TICK: Duration = Duration::from_millis(33);
const CALM_TICK: Duration = Duration::from_millis(200);
const DROWSY_TICK: Duration = Duration::from_millis(1000);

/// Лестница пауз перед переподключением (ТД-2).
const BACKOFF_STEPS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
/// Сессия, прожившая дольше этого, считается стабильной — лестница заново.
const BACKOFF_STABLE: Duration = Duration::from_secs(60);
/// Шаг дренажа очередей приложения во время паузы переподключения:
/// `ctl status`/`ctl quit` остаются отзывчивыми, пока композитора нет.
const LOST_TICK: Duration = Duration::from_millis(250);

/// Период тика для темпа приложения.
fn pace_delay(pace: Pace) -> Duration {
    match pace {
        Pace::Active => ACTIVE_TICK,
        Pace::Calm => CALM_TICK,
        Pace::Drowsy => DROWSY_TICK,
    }
}

/// Запустить бэкенд; возвращается после запроса выхода приложением.
///
/// Supervision-цикл: одна «попытка» = соединение + слой + event loop. Потеря
/// сессии (ошибка Wayland, закрытие слоя композитором) не убивает демон:
/// приложению доставляется [`Event::OutputLost`], всё платформенное состояние
/// сносится и пересоздаётся после паузы по лестнице бэкоффа. Если же ПЕРВАЯ
/// попытка не смогла даже подняться (нет Wayland, нет layer-shell — GNOME),
/// возвращается ошибка: ретраи тут бессмысленны, а внешний рестарт при гонке
/// старта сессии — забота systemd-юнита (Restart=on-failure).
///
/// Отклонение от заголовка в lib.rs: требуется `'static`, потому что
/// wayland-client хранит состояние диспатча в `Arc<dyn ObjectData>`.
pub fn run(app: impl App + 'static) -> Result<()> {
    let mut app: Box<dyn App> = Box::new(app);
    // Одни монотонные часы на все попытки: `now` приложения не прыгает
    // и не обнуляется при переподключении.
    let clock = Instant::now();
    let mut backoff = Backoff::new();
    let mut attempt: u64 = 0;

    loop {
        attempt += 1;
        let attempt_start = Instant::now();
        let (returned, outcome) = run_attempt(app, clock);
        app = returned;

        let reason = match outcome {
            Outcome::Exit => return Ok(()),
            Outcome::Lost(e) => {
                // Живая сессия оборвалась — приложение ставит симуляцию на
                // паузу (и само решает, что делать с миром).
                let now = clock.elapsed().as_secs_f64();
                if !app.event(Event::OutputLost, now) || app.wants_exit() {
                    return Ok(());
                }
                e
            }
            Outcome::SetupFailed(e) if attempt == 1 => return Err(e),
            // Повторный запуск не поднялся (композитор ещё стартует) —
            // мы всё ещё в потерянном состоянии, OutputLost уже доставлен.
            Outcome::SetupFailed(e) => e,
        };

        let delay = backoff.after_attempt(attempt_start.elapsed());
        log::warn!(
            "Wayland-сессия потеряна (попытка {attempt}): {reason:#}; переподключение через {delay:?}"
        );
        if !wait_lost(app.as_mut(), clock, delay) {
            return Ok(());
        }
    }
}

/// Пауза между попытками. Спим ломтиками и продолжаем тикать приложение,
/// чтобы его внешние очереди (IPC) не зависали на всё время бэкоффа.
/// `false` — приложение попросило выход.
fn wait_lost(app: &mut dyn App, clock: Instant, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if app.wants_exit() {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(LOST_TICK));
        // Сцена не нужна — тик здесь только ради дренажа очередей.
        let _ = app.tick(clock.elapsed().as_secs_f64());
    }
}

/// Итог одной попытки.
enum Outcome {
    /// Приложение попросило выход — завершаемся по-настоящему.
    Exit,
    /// Живая сессия умерла (ошибка соединения / слой закрыт) — переподключаться.
    Lost(anyhow::Error),
    /// До event loop не дошли (нет соединения/глобалов).
    SetupFailed(anyhow::Error),
}

/// Лестница бэкоффа: 1с → 2с → 5с → 10с → 30с (потолок); попытка, прожившая
/// дольше [`BACKOFF_STABLE`], возвращает лестницу к началу.
struct Backoff {
    step: usize,
}

impl Backoff {
    fn new() -> Self {
        Self { step: 0 }
    }

    /// Пауза после попытки, длившейся `ran`.
    fn after_attempt(&mut self, ran: Duration) -> Duration {
        if ran >= BACKOFF_STABLE {
            self.step = 0;
        }
        let delay = BACKOFF_STEPS[self.step];
        self.step = (self.step + 1).min(BACKOFF_STEPS.len() - 1);
        delay
    }
}

/// Одна попытка: соединение + якорь + субповерхность + event loop.
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

    let conn = setup!(
        Connection::connect_to_env(),
        "нет соединения с Wayland (WAYLAND_DISPLAY)"
    );
    let (globals, event_queue) =
        setup!(registry_queue_init::<Backend>(&conn), "registry_queue_init");
    let qh = event_queue.handle();

    let compositor = setup!(
        CompositorState::bind(&globals, &qh),
        "wl_compositor недоступен"
    );
    let subcompositor = setup!(
        SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh),
        "wl_subcompositor недоступен"
    );
    let layer_shell = setup!(
        LayerShell::bind(&globals, &qh),
        "zwlr_layer_shell_v1 недоступен (композитор без wlr-layer-shell?)"
    );
    let shm = setup!(Shm::bind(&globals, &qh), "wl_shm недоступен");

    // Якорь: прозрачная поверхность НА ВЕСЬ выход (модель wl_shimeji, см.
    // RESEARCH.md). Полноэкранный якорь держит композицию выхода включённой,
    // пока питомец на экране: KWin 6.3 при прямом сканауте целиком накрытого
    // окном выхода не рисует субповерхности «точечного» 1x1-якоря и не шлёт
    // им frame callback'и (питомец замерзал — диагностировано живьём в фазе
    // D). Пустая сцена сжимает якорь до 1x1 (Renderer::set_anchor_full) —
    // сканаут и его энергосбережение возвращаются композитору (D5).
    // exclusive_zone(-1) растягивает слой по самому ВЫХОДУ (не рабочей
    // области), чтобы позиция субповерхности совпадала с логическими
    // экранными координатами сцены.
    // TODO(M3): мультивыход — по одному якорю на каждый wl_output.
    let surface = compositor.create_surface(&qh);
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("driftling"), None);
    layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::BOTTOM | Anchor::RIGHT);
    layer.set_size(0, 0);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    // Якорь никогда не ловит мышь: пустой (не None!) input region.
    let region = setup!(Region::new(&compositor), "wl_region якоря");
    layer
        .wl_surface()
        .set_input_region(Some(region.wl_region()));
    drop(region);
    // Первый commit без буфера — маппинг слоя; композитор ответит configure.
    layer.commit();

    // Питомец: desync-субповерхность якоря. Позиция — состояние якоря
    // (применяется его коммитом), содержимое — своё (применяется сразу).
    let (pet_subsurface, pet_surface) =
        subcompositor.create_subsurface(layer.wl_surface().clone(), &qh);
    pet_subsurface.set_desync();

    // Пул вырастет сам при первом create_buffer нужного размера.
    let pool = setup!(SlotPool::new(64 * 1024, &shm), "SlotPool");

    let mut event_loop: EventLoop<'static, Backend> = setup!(EventLoop::try_new(), "calloop");
    let loop_handle = event_loop.handle();
    setup!(
        WaylandSource::new(conn, event_queue)
            .insert(loop_handle.clone())
            .map_err(|e| anyhow!("{e}")),
        "wayland source"
    );

    let mut backend = Backend {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        _subcompositor: subcompositor,
        pointer: None,
        loop_handle,
        loop_signal: event_loop.get_signal(),
        timer_token: None,
        armed_delay: Duration::ZERO,
        app: app_slot.take().expect("App передан в попытку"),
        clock,
        current_output: None,
        last_geometry: None,
        pressed: false,
        last_pointer: Vec2::default(),
        exit: false,
        lost: None,
        renderer: Renderer {
            pool,
            compositor,
            qh,
            layer,
            pet_surface,
            pet_subsurface,
            scale: 1,
            parent_mapped: false,
            anchor_full: true,
            visible: false,
            frame_done: true,
            last_draw: None,
            last_content: None,
            last_pos: None,
            last_input: Vec::new(),
        },
    };

    log::info!("Wayland: соединение установлено, слой-якорь создан");
    backend.arm_timer(Duration::ZERO);

    let run_result = event_loop.run(None, &mut backend, |_| {});

    let outcome = if backend.exit {
        Outcome::Exit
    } else if let Some(reason) = backend.lost.take() {
        Outcome::Lost(reason)
    } else if let Err(e) = run_result {
        Outcome::Lost(anyhow!(e).context("event loop"))
    } else {
        // loop_signal.stop() зовём только мы — сюда попадать не должны.
        Outcome::Lost(anyhow!("event loop остановился без причины"))
    };
    *app_slot = Some(backend.app);
    outcome
}

/// Состояние бэкенда: SCTK-обвязка + приложение + рендер.
struct Backend {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    /// Понадобится живым для новых субповерхностей (мультипитомцы M4).
    _subcompositor: SubcompositorState,
    pointer: Option<wl_pointer::WlPointer>,
    loop_handle: LoopHandle<'static, Backend>,
    loop_signal: LoopSignal,
    /// Взведённый таймер симуляции и период, на который он взведён.
    timer_token: Option<RegistrationToken>,
    armed_delay: Duration,
    app: Box<dyn App>,
    /// Общие для всех попыток монотонные часы (`now` приложения).
    clock: Instant,

    /// Выход, на который композитор посадил якорь (`surface_enter`).
    current_output: Option<wl_output::WlOutput>,
    /// Последняя доставленная геометрия (размер + глобальная позиция
    /// выхода) — дедупликация update_output.
    last_geometry: Option<(f32, f32, i32, i32)>,

    /// Кнопка (BTN_LEFT) зажата — идёт implicit grab.
    pressed: bool,
    /// Последняя позиция указателя в логических экранных координатах —
    /// точка синтетического Release при Leave во время grab.
    last_pointer: Vec2,

    exit: bool,
    /// Причина потери сессии; взведение останавливает цикл с ретраем.
    lost: Option<anyhow::Error>,

    renderer: Renderer,
}

/// Всё, что нужно для отрисовки сцены. Отдельная структура, чтобы занимать
/// поля self раздельно с `app` (Scene заимствует app на время sync).
struct Renderer {
    pool: SlotPool,
    compositor: CompositorState,
    qh: QueueHandle<Backend>,
    layer: LayerSurface,
    pet_surface: wl_surface::WlSurface,
    pet_subsurface: wl_subsurface::WlSubsurface,
    /// Целочисленный масштаб буфера (HiDPI). TODO(D6): дробный масштаб.
    scale: u32,
    /// Якорь замаплен (configure получен, прозрачный буфер прикреплён).
    parent_mapped: bool,
    /// Режим якоря: true — на весь выход (питомец на экране, композиция
    /// принудительно включена), false — 1x1 (сцена пуста, сканаут отдан
    /// композитору). См. комментарий при создании слоя.
    anchor_full: bool,
    /// У поверхности питомца есть буфер (сцена непустая).
    visible: bool,
    /// Прошлый кадр показан композитором — можно рисовать следующий.
    frame_done: bool,
    /// Когда последний раз рисовали содержимое — страховка от голодания
    /// frame callback'ов (см. FRAME_CB_FALLBACK).
    last_draw: Option<Instant>,
    /// Содержимое последнего нарисованного буфера (dirty-check).
    last_content: Option<ContentKey>,
    /// Последняя выставленная позиция субповерхности (логические координаты).
    last_pos: Option<(i32, i32)>,
    /// Последний выставленный input region в локальных координатах питомца.
    last_input: Vec<(i32, i32, i32, i32)>,
}

impl Renderer {
    /// Ответ на configure якоря: прозрачный буфер назначенного размера
    /// (полный выход или 1x1 — по текущему режиму). Каждый configure
    /// (первый маппинг, смена режима, ресайз выхода) перепривязывает буфер.
    fn anchor_configured(&mut self, (w, h): (u32, u32)) -> Result<()> {
        let (w, h) = (w.max(1) as i32, h.max(1) as i32);
        let (buffer, canvas) = self
            .pool
            .create_buffer(w, h, w * 4, wl_shm::Format::Argb8888)
            .context("shm-буфер якоря")?;
        canvas.fill(0);
        let surface = self.layer.wl_surface();
        buffer.attach_to(surface).context("attach буфера якоря")?;
        surface.damage_buffer(0, 0, w, h);
        self.layer.commit();
        self.parent_mapped = true;
        Ok(())
    }

    /// Переключить режим якоря (весь выход <-> 1x1). Новый буфер придёт
    /// со следующим configure (anchor_configured); вызов идемпотентен.
    fn set_anchor_full(&mut self, full: bool) {
        if self.anchor_full == full {
            return;
        }
        self.anchor_full = full;
        if full {
            self.layer
                .set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::BOTTOM | Anchor::RIGHT);
            self.layer.set_size(0, 0);
        } else {
            self.layer.set_anchor(Anchor::TOP | Anchor::LEFT);
            self.layer.set_size(1, 1);
        }
        self.layer.commit();
    }

    /// Привести поверхности в соответствие сцене: перерисовать содержимое
    /// (только если оно изменилось), передвинуть субповерхность (только если
    /// сдвинулась), обновить input region (только если изменился).
    fn sync(&mut self, scene: &Scene<'_>) {
        if !self.parent_mapped {
            return;
        }
        let Some((bx, by, bw, bh)) = scene_bounds(scene) else {
            self.hide();
            return;
        };
        // Питомец на экране — якорь на весь выход (композиция включена).
        self.set_anchor_full(true);
        let scale = self.scale.max(1);
        let key = content_key(scene, (bx, by), scale);
        let content_dirty = !self.visible || self.last_content.as_ref() != Some(&key);
        let mut commit_pet = false;

        let first_show = !self.visible;
        // Обычный такт задаёт callback композитора; его голодание (KWin
        // при прямом сканауте) не должно замораживать питомца навсегда.
        let may_draw = self.frame_done
            || self
                .last_draw
                .is_none_or(|t| t.elapsed() >= FRAME_CB_FALLBACK);
        if content_dirty && may_draw {
            let (pw, ph) = (bw * scale, bh * scale);
            match self.pool.create_buffer(
                pw as i32,
                ph as i32,
                pw as i32 * 4,
                wl_shm::Format::Argb8888,
            ) {
                Ok((buffer, canvas)) => {
                    compose(canvas, (pw, ph), scene, (bx, by), scale);
                    self.pet_surface.set_buffer_scale(scale as i32);
                    self.pet_surface.damage_buffer(0, 0, pw as i32, ph as i32);
                    // Дросселирование: следующее содержимое — после показа
                    // этого (frame callback). Запрашиваем на ЯКОРЕ: KWin не
                    // шлёт callback'и субповерхностям (см. Backend::frame);
                    // коммит якоря идёт следом в этом же sync. Пропущенная
                    // перерисовка не теряется: dirty-check повторит её.
                    let parent = self.layer.wl_surface();
                    parent.frame(&self.qh, parent.clone());
                    if let Err(e) = buffer.attach_to(&self.pet_surface) {
                        log::error!("attach буфера питомца: {e}");
                    }
                    self.frame_done = false;
                    self.last_draw = Some(Instant::now());
                    self.visible = true;
                    self.last_content = Some(key);
                    commit_pet = true;
                }
                Err(e) => log::error!("shm-буфер {pw}x{ph}: {e}"),
            }
        }
        if !self.visible {
            // Первый кадр ещё не нарисован (нет буфера) — позиция и input
            // region подождут его.
            return;
        }

        // Первый показ: позиция применяется ДО коммита содержимого, иначе
        // питомец на один кадр композитора замапится в (0,0) якоря.
        if first_show && self.last_pos != Some((bx, by)) {
            self.pet_subsurface.set_position(bx, by);
            self.layer.commit();
            self.last_pos = Some((bx, by));
        }

        // Input region поверхности питомца — в ЕЁ локальных координатах:
        // регион едет вместе с субповерхностью и меняется только при смене
        // размеров спрайта. Пустой список прямоугольников недостижим здесь
        // (пустая сцена ушла в hide), но и он дал бы пустой регион, не None.
        let input = local_input_rects(&scene.input_rects, (bx, by));
        if input != self.last_input {
            match Region::new(&self.compositor) {
                Ok(region) => {
                    for &(x, y, w, h) in &input {
                        region.add(x, y, w, h);
                    }
                    self.pet_surface.set_input_region(Some(region.wl_region()));
                    self.last_input = input;
                    commit_pet = true;
                }
                Err(e) => log::error!("wl_region: {e}"),
            }
        }
        if commit_pet {
            self.pet_surface.commit();
        }

        // Позиция субповерхности — состояние РОДИТЕЛЯ: set_position +
        // коммит якоря. Никаких пикселей и configure — самое частое действие
        // (ходьба) стоит два крошечных запроса. Коммит якоря нужен и без
        // движения, когда рисовалось содержимое: он применяет frame-запрос
        // на якоре (см. блок отрисовки выше).
        let moved = self.last_pos != Some((bx, by));
        if moved {
            self.pet_subsurface.set_position(bx, by);
            self.last_pos = Some((bx, by));
        }
        if moved || commit_pet {
            self.layer.commit();
        }
    }

    /// Спрятать питомца: null-буфер демапит поверхность, размаппленная
    /// поверхность не ловит ввод — сцена «ничего нет» стоит ноль. Якорь
    /// сжимается до 1x1: выход возвращается к прямому сканауту (D5).
    fn hide(&mut self) {
        self.set_anchor_full(false);
        if !self.visible {
            return;
        }
        self.pet_surface.attach(None, 0, 0);
        self.pet_surface.commit();
        self.visible = false;
        self.last_content = None;
        // frame callback скрытой поверхности может не прийти никогда —
        // не дать ему заблокировать первый кадр после следующего summon.
        self.frame_done = true;
    }
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
        self.loop_signal.stop();
    }

    /// Отдать событие приложению; false от него — сигнал завершения.
    fn deliver(&mut self, ev: Event) {
        let now = self.now();
        if !self.app.event(ev, now) || self.app.wants_exit() {
            self.exit = true;
            self.loop_signal.stop();
            return;
        }
        // Событие могло разбудить приложение (сон → drag) — ускоряемся.
        self.maybe_escalate();
    }

    /// (Пере)взвести таймер симуляции. Каждый тик сам перевзводится на период
    /// из `App::pace()` — это и есть адаптивный темп ТД-3.
    ///
    /// Пересоздание источника (remove+insert) дёргаем редко: при старте
    /// попытки и при ускорении темпа (maybe_escalate). Снятие таймера, чьё
    /// срабатывание уже в очереди диспатча, шумит в логе calloop стейл-токеном
    /// — потому обычное перевзведение живёт внутри callback'а (ToDuration).
    fn arm_timer(&mut self, delay: Duration) {
        if let Some(token) = self.timer_token.take() {
            self.loop_handle.remove(token);
        }
        self.armed_delay = delay;
        let timer = Timer::from_duration(delay);
        match self
            .loop_handle
            .insert_source(timer, |_deadline, _, state: &mut Backend| {
                state.tick_and_draw();
                let delay = pace_delay(state.app.pace());
                state.armed_delay = delay;
                TimeoutAction::ToDuration(delay)
            }) {
            Ok(token) => self.timer_token = Some(token),
            Err(e) => self.fail(anyhow!("таймер симуляции: {e}")),
        }
    }

    /// Если приложение захотело тикать чаще, чем взведено, — перевзвести
    /// немедленно (пробуждение при событии, без ожидания медленного таймера).
    fn maybe_escalate(&mut self) {
        if self.exit || self.lost.is_some() {
            return;
        }
        if pace_delay(self.app.pace()) < self.armed_delay {
            self.arm_timer(Duration::ZERO);
        }
    }

    /// Шаг симуляции + синхронизация поверхностей со сценой.
    fn tick_and_draw(&mut self) {
        if self.exit || self.lost.is_some() {
            self.loop_signal.stop();
            return;
        }
        let now = self.now();
        // Scene заимствует self.app; renderer — отдельное поле, конфликта нет.
        let scene = self.app.tick(now);
        self.renderer.sync(&scene);
        drop(scene);

        if self.app.wants_exit() {
            self.exit = true;
            self.loop_signal.stop();
        }
    }

    /// Отдать приложению логический размер текущего выхода. 1x1-якорь не
    /// узнаёт размер экрана из configure — берём его из OutputState (ТД-4).
    fn push_geometry(&mut self) {
        let Some(output) = self.current_output.clone() else {
            return;
        };
        let Some(info) = self.output_state.info(&output) else {
            return;
        };
        let mode = info.modes.iter().find(|m| m.current).map(|m| m.dimensions);
        let swapped = matches!(
            info.transform,
            wl_output::Transform::_90
                | wl_output::Transform::_270
                | wl_output::Transform::Flipped90
                | wl_output::Transform::Flipped270
        );
        let Some((w, h)) = output_logical_size(info.logical_size, mode, info.scale_factor, swapped)
        else {
            log::warn!("выход без размера (ни logical_size, ни текущего режима)");
            return;
        };
        // Глобальное положение выхода (xdg-output): worldsense переводит
        // глобальные координаты окон в локальные координаты этого выхода.
        let (ox, oy) = info.logical_position.unwrap_or((0, 0));
        if self.last_geometry == Some((w, h, ox, oy)) {
            return;
        }
        self.last_geometry = Some((w, h, ox, oy));
        log::info!("выход: {w:.0}x{h:.0} (логических) в глобальной точке ({ox}, {oy})");
        // Если приложение в ответ призовёт питомца (ускорит темп), таймер
        // перевзведёт maybe_escalate внутри deliver — первый кадр не ждёт.
        self.deliver(Event::OutputGeometry {
            width: w,
            height: h,
            origin: Vec2::new(ox as f32, oy as f32),
        });
    }
}

/// Логический размер выхода: приоритет — xdg-output (`logical_size`), фолбэк —
/// текущий видеорежим, поделённый на целый масштаб (с поворотом для 90/270).
fn output_logical_size(
    logical: Option<(i32, i32)>,
    current_mode: Option<(i32, i32)>,
    scale_factor: i32,
    swapped: bool,
) -> Option<(f32, f32)> {
    if let Some((w, h)) = logical {
        if w > 0 && h > 0 {
            return Some((w as f32, h as f32));
        }
    }
    let (mw, mh) = current_mode?;
    if mw <= 0 || mh <= 0 {
        return None;
    }
    let (w, h) = if swapped { (mh, mw) } else { (mw, mh) };
    let s = scale_factor.max(1) as f32;
    Some((w as f32 / s, h as f32 / s))
}

/// Локальные координаты поверхности питомца → логические экранные.
/// Субповерхность стоит в `pos` относительно якоря, якорь — в (0,0) выхода.
/// Позиция берётся последней ВЫСТАВЛЕННОЙ (композитор может отставать на
/// кадр — во время drag это даёт погрешность в один тик, самокорректируется).
fn to_screen(local: (f64, f64), pos: (i32, i32)) -> Vec2 {
    Vec2::new(local.0 as f32 + pos.0 as f32, local.1 as f32 + pos.1 as f32)
}

/// Объединённый прямоугольник всех непустых спрайтов сцены в логических
/// пикселях: (x, y, w, h). None — рисовать нечего.
fn scene_bounds(scene: &Scene<'_>) -> Option<(i32, i32, u32, u32)> {
    let mut acc: Option<(i32, i32, i32, i32)> = None;
    for s in &scene.sprites {
        if s.frame.w == 0 || s.frame.h == 0 {
            continue;
        }
        let x0 = s.origin.x.round() as i32;
        let y0 = s.origin.y.round() as i32;
        let x1 = x0 + s.frame.w as i32;
        let y1 = y0 + s.frame.h as i32;
        acc = Some(match acc {
            None => (x0, y0, x1, y1),
            Some((ax0, ay0, ax1, ay1)) => (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1)),
        });
    }
    acc.map(|(x0, y0, x1, y1)| (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Ключ содержимого буфера для dirty-check. Позиция сцены на экране в ключ
/// НЕ входит: чистое перемещение не требует перерисовки, только set_position.
#[derive(Clone, PartialEq, Eq, Debug)]
struct ContentKey {
    scale: u32,
    sprites: Vec<SpriteKey>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct SpriteKey {
    /// Идентичность пикселей — адрес массива кадра. Кадры живут в SpriteSet
    /// приложения; новый кадр = другой Vec = другой адрес. Теоретическая
    /// коллизия (новая аллокация по старому адресу при том же размере кадра)
    /// дала бы один устаревший кадр; в текущем коде набор пересоздаётся только
    /// со сменой размера — там меняются и w/h.
    argb: usize,
    w: u32,
    h: u32,
    /// Положение спрайта внутри буфера (относительно объединённых границ).
    rel: (i32, i32),
    mirror: bool,
}

fn content_key(scene: &Scene<'_>, origin: (i32, i32), scale: u32) -> ContentKey {
    ContentKey {
        scale,
        sprites: scene
            .sprites
            .iter()
            .filter(|s| s.frame.w > 0 && s.frame.h > 0)
            .map(|s| SpriteKey {
                argb: s.frame.argb.as_ptr() as usize,
                w: s.frame.w,
                h: s.frame.h,
                rel: (
                    s.origin.x.round() as i32 - origin.0,
                    s.origin.y.round() as i32 - origin.1,
                ),
                mirror: s.mirror,
            })
            .collect(),
    }
}

/// Input-прямоугольники сцены (логические экранные) → локальные координаты
/// поверхности питомца, с округлением наружу. Пустые/вырожденные выбрасываются.
fn local_input_rects(rects: &[Rect], origin: (i32, i32)) -> Vec<(i32, i32, i32, i32)> {
    rects
        .iter()
        .filter(|r| r.w > 0.0 && r.h > 0.0)
        .map(|r| {
            let x0 = r.x.floor() as i32 - origin.0;
            let y0 = r.y.floor() as i32 - origin.1;
            let x1 = (r.x + r.w).ceil() as i32 - origin.0;
            let y1 = (r.y + r.h).ceil() as i32 - origin.1;
            (x0, y0, x1 - x0, y1 - y0)
        })
        .collect()
}

/// Собрать буфер питомца: прозрачный фон + спрайты относительно `origin`
/// (левый верх объединённых границ). `size` — физический размер (уже * scale).
fn compose(canvas: &mut [u8], size: (u32, u32), scene: &Scene<'_>, origin: (i32, i32), scale: u32) {
    canvas.fill(0); // 0x00000000 — полностью прозрачно
    for s in &scene.sprites {
        let rel = (
            s.origin.x.round() as i32 - origin.0,
            s.origin.y.round() as i32 - origin.1,
        );
        blit(canvas, size, s.frame, rel, s.mirror, scale);
    }
}

/// Блит спрайта на холст: nearest-neighbour масштаб, опциональное
/// горизонтальное зеркало, пропуск пикселей с альфой 0, клип по краям.
/// `origin` — логические координаты левого верхнего угла спрайта на холсте.
fn blit(
    canvas: &mut [u8],
    (cw, ch): (u32, u32),
    frame: &Frame,
    (ox, oy): (i32, i32),
    mirror: bool,
    scale: u32,
) {
    if frame.w == 0 || frame.h == 0 {
        return;
    }
    let base_x = ox * scale as i32;
    let base_y = oy * scale as i32;
    for dy in 0..frame.h * scale {
        let cy = base_y + dy as i32;
        if cy < 0 || cy >= ch as i32 {
            continue;
        }
        let sy = dy / scale;
        for dx in 0..frame.w * scale {
            let cx = base_x + dx as i32;
            if cx < 0 || cx >= cw as i32 {
                continue;
            }
            let sx = if mirror {
                frame.w - 1 - dx / scale
            } else {
                dx / scale
            };
            let px = frame.argb[(sy * frame.w + sx) as usize];
            if px >> 24 == 0 {
                continue; // прозрачный пиксель спрайта
            }
            let off = ((cy as u32 * cw + cx as u32) * 4) as usize;
            // ARGB8888 little-endian: байты B, G, R, A.
            canvas[off..off + 4].copy_from_slice(&px.to_le_bytes());
        }
    }
}

impl CompositorHandler for Backend {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // Якорь и питомец живут на одном выходе — масштаб общий. Новый
        // масштаб подхватит ближайший тик: он входит в ContentKey, значит
        // содержимое перерисуется с новым set_buffer_scale.
        self.renderer.scale = new_factor.max(1) as u32;
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // Композитор показал кадр — можно рисовать следующий. Callback
        // запрашивается на ЯКОРЕ: KWin 6.3 не шлёт frame callback'и
        // субповерхностям layer-суфейсов вовсе (проверено живьём в фазе D —
        // ноль done за минуты анимации), а слою — шлёт. Принимаем оба на
        // случай других композиторов.
        if *surface == self.renderer.pet_surface || surface == self.renderer.layer.wl_surface() {
            self.renderer.frame_done = true;
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        // Композитор посадил якорь на выход — теперь известно, ЧЕЙ логический
        // размер отдавать приложению.
        if surface == self.renderer.layer.wl_surface() {
            self.current_output = Some(output.clone());
            self.last_geometry = None;
            self.push_geometry();
        }
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        // Переезд на другой выход завершится surface_enter — там и обновимся.
    }
}

impl OutputHandler for Backend {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    // TODO(M3): мультивыход — реагировать на new_output, создавая якорь на
    // каждом выходе. Пока якорь один и живёт, где посадил композитор.
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // Смена разрешения/масштаба нашего выхода.
        if self.current_output.as_ref() == Some(&output) {
            self.push_geometry();
        }
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self.current_output.as_ref() == Some(&output) {
            // Дальше композитор либо закроет слой (наш путь переподключения),
            // либо переселит якорь на другой выход (придёт surface_enter).
            self.current_output = None;
            self.last_geometry = None;
        }
    }
}

impl LayerShellHandler for Backend {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        // Выход исчез / композитор снял слой. Не смерть, а повод
        // пересоздаться: supervision-цикл в run() поднимет новую сессию.
        self.fail(anyhow!("композитор закрыл слой"));
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // Каждый configure (маппинг, смена режима якоря, ресайз выхода)
        // получает прозрачный буфер назначенного размера. Размер ЭКРАНА
        // по-прежнему приходит из OutputState (см. push_geometry): в режиме
        // 1x1 configure о выходе ничего не говорит.
        // Первый кадр питомца придёт по цепочке surface_enter → геометрия →
        // maybe_escalate; отдельного пинка таймеру здесь не нужно (лишний
        // remove+insert только шумит стейл-токенами в логе calloop).
        if let Err(e) = self.renderer.anchor_configured(configure.new_size) {
            self.fail(e);
        }
    }
}

impl SeatHandler for Backend {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(e) => log::error!("wl_pointer: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for Backend {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            // Мышь ловит только поверхность питомца (у якоря пустой регион).
            if event.surface != self.renderer.pet_surface {
                continue;
            }
            let pos = to_screen(event.position, self.renderer.last_pos.unwrap_or((0, 0)));
            let ev = match event.kind {
                // Enter несёт позицию — отдаём как движение.
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.last_pointer = pos;
                    Event::PointerMotion(pos)
                }
                PointerEventKind::Press {
                    button: BTN_LEFT, ..
                } => {
                    self.pressed = true;
                    self.last_pointer = pos;
                    Event::PointerPress(pos)
                }
                PointerEventKind::Release {
                    button: BTN_LEFT, ..
                } => {
                    self.pressed = false;
                    self.last_pointer = pos;
                    Event::PointerRelease(pos)
                }
                // ПКМ — контекстное меню (ТД-9; само меню — фаза B).
                PointerEventKind::Press {
                    button: BTN_RIGHT, ..
                } => {
                    self.last_pointer = pos;
                    Event::PointerMenu(pos)
                }
                // Уход курсора при зажатой кнопке = отпускание в последней
                // точке: если композитор разорвал implicit grab (input region
                // уехал из-под курсора между кадрами), Motion/Release уже не
                // придут — без этого питомец вечно висел бы в Dragged (ТД-5).
                PointerEventKind::Leave { .. } if self.pressed => {
                    self.pressed = false;
                    Event::PointerRelease(self.last_pointer)
                }
                _ => continue,
            };
            self.deliver(ev);
            if self.exit || self.lost.is_some() {
                return;
            }
        }
    }
}

impl ShmHandler for Backend {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(Backend);
delegate_subcompositor!(Backend);
delegate_output!(Backend);
delegate_shm!(Backend);
delegate_seat!(Backend);
delegate_pointer!(Backend);
delegate_layer!(Backend);
delegate_registry!(Backend);

impl ProvidesRegistryState for Backend {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpriteInstance;

    /// Холст w*h, читаем пиксель обратно как ARGB u32.
    fn pixel(canvas: &[u8], w: u32, x: u32, y: u32) -> u32 {
        let off = ((y * w + x) * 4) as usize;
        u32::from_le_bytes(canvas[off..off + 4].try_into().unwrap())
    }

    fn frame_2x2() -> Frame {
        // A B
        // C .   (правый нижний прозрачный)
        Frame {
            w: 2,
            h: 2,
            argb: vec![0xff_11_00_00, 0xff_00_22_00, 0xff_00_00_33, 0x00_00_00_00],
        }
    }

    fn scene_one(frame: &Frame, origin: Vec2, mirror: bool) -> Scene<'_> {
        Scene {
            sprites: vec![SpriteInstance {
                frame,
                origin,
                mirror,
            }],
            input_rects: vec![Rect::new(
                origin.x,
                origin.y,
                frame.w as f32,
                frame.h as f32,
            )],
        }
    }

    // --- blit -------------------------------------------------------------

    #[test]
    fn blit_scale1_pixel_perfect() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 4 * 4 * 4];
        blit(&mut canvas, (4, 4), &f, (1, 1), false, 1);
        assert_eq!(pixel(&canvas, 4, 1, 1), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, 4, 2, 1), 0xff_00_22_00);
        assert_eq!(pixel(&canvas, 4, 1, 2), 0xff_00_00_33);
        // Прозрачный пиксель спрайта не затирает фон.
        assert_eq!(pixel(&canvas, 4, 2, 2), 0);
        // Вокруг — прозрачно.
        assert_eq!(pixel(&canvas, 4, 0, 0), 0);
        assert_eq!(pixel(&canvas, 4, 3, 3), 0);
    }

    #[test]
    fn blit_mirror_swaps_columns() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 2 * 2 * 4];
        blit(&mut canvas, (2, 2), &f, (0, 0), true, 1);
        assert_eq!(pixel(&canvas, 2, 0, 0), 0xff_00_22_00); // B слева
        assert_eq!(pixel(&canvas, 2, 1, 0), 0xff_11_00_00); // A справа
        assert_eq!(pixel(&canvas, 2, 0, 1), 0); // прозрачный (зеркало C)
        assert_eq!(pixel(&canvas, 2, 1, 1), 0xff_00_00_33);
    }

    #[test]
    fn blit_scale2_nearest_neighbour() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 4 * 4 * 4];
        blit(&mut canvas, (4, 4), &f, (0, 0), false, 2);
        // Каждый исходный пиксель — блок 2x2.
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(pixel(&canvas, 4, x, y), 0xff_11_00_00);
        }
        for (x, y) in [(2, 0), (3, 1)] {
            assert_eq!(pixel(&canvas, 4, x, y), 0xff_00_22_00);
        }
        for (x, y) in [(2, 2), (3, 3)] {
            assert_eq!(pixel(&canvas, 4, x, y), 0); // прозрачный блок
        }
    }

    #[test]
    fn blit_clips_out_of_bounds() {
        let f = frame_2x2();
        let mut canvas = vec![0u8; 2 * 2 * 4];
        // Наполовину за левым верхним углом: не паникует, видимая часть верна.
        blit(&mut canvas, (2, 2), &f, (-1, -1), false, 1);
        assert_eq!(pixel(&canvas, 2, 0, 0), 0); // прозрачный угол спрайта
                                                // За правым нижним краем — тоже без паники.
        blit(&mut canvas, (2, 2), &f, (1, 1), false, 1);
        assert_eq!(pixel(&canvas, 2, 1, 1), 0xff_11_00_00);
    }

    // --- compose: спрайт рисуется относительно границ буфера ---------------

    #[test]
    fn compose_draws_relative_to_bounds() {
        let f = frame_2x2();
        let scene = scene_one(&f, Vec2::new(100.0, 200.0), false);
        let (bx, by, bw, bh) = scene_bounds(&scene).unwrap();
        assert_eq!((bx, by, bw, bh), (100, 200, 2, 2));
        let mut canvas = vec![0u8; (bw * bh * 4) as usize];
        compose(&mut canvas, (bw, bh), &scene, (bx, by), 1);
        // Спрайт лёг в (0,0) буфера, а не в экранные (100,200).
        assert_eq!(pixel(&canvas, bw, 0, 0), 0xff_11_00_00);
        assert_eq!(pixel(&canvas, bw, 1, 1), 0);
    }

    // --- перевод координат указателя ---------------------------------------

    #[test]
    fn to_screen_adds_subsurface_position() {
        let p = to_screen((3.5, 7.25), (100, 200));
        assert_eq!(p, Vec2::new(103.5, 207.25));
    }

    #[test]
    fn to_screen_handles_out_of_sprite_coords_during_grab() {
        // Во время implicit grab локальные координаты бывают отрицательными.
        let p = to_screen((-10.0, -2.5), (50, 40));
        assert_eq!(p, Vec2::new(40.0, 37.5));
    }

    // --- границы сцены ------------------------------------------------------

    #[test]
    fn scene_bounds_rounds_origin() {
        let f = frame_2x2();
        let scene = scene_one(&f, Vec2::new(10.6, -3.4), false);
        assert_eq!(scene_bounds(&scene), Some((11, -3, 2, 2)));
    }

    #[test]
    fn scene_bounds_unions_sprites() {
        let f = frame_2x2();
        let scene = Scene {
            sprites: vec![
                SpriteInstance {
                    frame: &f,
                    origin: Vec2::new(0.0, 0.0),
                    mirror: false,
                },
                SpriteInstance {
                    frame: &f,
                    origin: Vec2::new(10.0, 4.0),
                    mirror: false,
                },
            ],
            input_rects: vec![],
        };
        assert_eq!(scene_bounds(&scene), Some((0, 0, 12, 6)));
    }

    #[test]
    fn scene_bounds_skips_empty_frames_and_empty_scene() {
        let empty = Frame {
            w: 0,
            h: 0,
            argb: vec![],
        };
        let scene = scene_one(&empty, Vec2::new(5.0, 5.0), false);
        assert_eq!(scene_bounds(&scene), None);
        let none = Scene {
            sprites: vec![],
            input_rects: vec![],
        };
        assert_eq!(scene_bounds(&none), None);
    }

    // --- dirty-check содержимого --------------------------------------------

    #[test]
    fn content_key_ignores_pure_movement() {
        let f = frame_2x2();
        let a = scene_one(&f, Vec2::new(10.0, 20.0), false);
        let b = scene_one(&f, Vec2::new(300.0, 400.0), false);
        let ka = content_key(&a, (10, 20), 1);
        let kb = content_key(&b, (300, 400), 1);
        // Кадр тот же, позиция другая → перерисовка не нужна.
        assert_eq!(ka, kb);
    }

    #[test]
    fn content_key_detects_frame_change() {
        let f1 = frame_2x2();
        let f2 = frame_2x2(); // другой Vec → другой адрес пикселей
        let a = scene_one(&f1, Vec2::new(0.0, 0.0), false);
        let b = scene_one(&f2, Vec2::new(0.0, 0.0), false);
        assert_ne!(content_key(&a, (0, 0), 1), content_key(&b, (0, 0), 1));
    }

    #[test]
    fn content_key_detects_mirror_and_scale() {
        let f = frame_2x2();
        let plain = scene_one(&f, Vec2::new(0.0, 0.0), false);
        let mirrored = scene_one(&f, Vec2::new(0.0, 0.0), true);
        assert_ne!(
            content_key(&plain, (0, 0), 1),
            content_key(&mirrored, (0, 0), 1)
        );
        assert_ne!(
            content_key(&plain, (0, 0), 1),
            content_key(&plain, (0, 0), 2)
        );
    }

    // --- input region в локальных координатах --------------------------------

    #[test]
    fn local_input_rects_translate_and_round_outward() {
        let rects = [Rect::new(10.2, 20.7, 3.5, 1.1)];
        // origin границ буфера = (10, 20)
        let local = local_input_rects(&rects, (10, 20));
        // floor(10.2)-10=0, floor(20.7)-20=0, ceil(13.7)-10-0=4, ceil(21.8)-20-0=2
        assert_eq!(local, vec![(0, 0, 4, 2)]);
    }

    #[test]
    fn local_input_rects_skip_degenerate() {
        let rects = [Rect::new(0.0, 0.0, 0.0, 5.0), Rect::new(1.0, 1.0, 2.0, 2.0)];
        assert_eq!(local_input_rects(&rects, (0, 0)), vec![(1, 1, 2, 2)]);
    }

    // --- размер выхода --------------------------------------------------------

    #[test]
    fn output_size_prefers_logical() {
        assert_eq!(
            output_logical_size(Some((1920, 1080)), Some((3840, 2160)), 2, false),
            Some((1920.0, 1080.0))
        );
    }

    #[test]
    fn output_size_falls_back_to_mode_with_scale_and_transform() {
        // Без xdg-output: режим 3840x2160 при масштабе 2 → 1920x1080.
        assert_eq!(
            output_logical_size(None, Some((3840, 2160)), 2, false),
            Some((1920.0, 1080.0))
        );
        // Повёрнутый выход меняет стороны местами.
        assert_eq!(
            output_logical_size(None, Some((1920, 1080)), 1, true),
            Some((1080.0, 1920.0))
        );
        assert_eq!(output_logical_size(None, None, 1, false), None);
        assert_eq!(output_logical_size(Some((0, 0)), None, 1, false), None);
    }

    // --- темп и бэкофф ---------------------------------------------------------

    #[test]
    fn pace_delay_matches_budget() {
        assert_eq!(pace_delay(Pace::Active), Duration::from_millis(33));
        assert_eq!(pace_delay(Pace::Calm), Duration::from_millis(200));
        assert_eq!(pace_delay(Pace::Drowsy), Duration::from_millis(1000));
    }

    #[test]
    fn backoff_ladder_caps_and_resets() {
        let mut b = Backoff::new();
        let quick = Duration::from_secs(1); // попытка умерла быстро
        let secs: Vec<u64> = (0..7).map(|_| b.after_attempt(quick).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 5, 10, 30, 30, 30]);
        // Стабильная сессия возвращает лестницу к началу.
        assert_eq!(b.after_attempt(Duration::from_secs(61)).as_secs(), 1);
        assert_eq!(b.after_attempt(quick).as_secs(), 2);
    }
}
