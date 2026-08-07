//! Wayland-бэкенд: wlr-layer-shell overlay + wl_shm рендер.
//!
//! Реализация M0 (см. docs/TZ.md §6):
//! - один прозрачный overlay-surface на весь выход (`Layer::Overlay`,
//!   anchor = все четыре кромки, size (0,0) — размер назначает композитор,
//!   `exclusive_zone(-1)`, `KeyboardInteractivity::None`);
//! - CPU-рендер в ARGB8888 через `SlotPool`; фон полностью прозрачный,
//!   спрайты блитятся поверх (альфа 0 → пиксель пропускается);
//! - клик-сквозь: input region каждый кадр = объединение `Scene::input_rects`;
//!   пустая сцена → пустой (не None!) регион, весь оверлей прозрачен для мыши;
//! - HiDPI: целочисленный масштаб выхода, буфер = логический размер * scale,
//!   спрайты растягиваются nearest-neighbour; scale=1 — попиксельно точный путь;
//! - симуляцию гонит calloop-таймер каждые 33 мс независимо от показа кадров;
//!   отрисовка дополнительно дросселируется `wl_surface::frame` callback'ом
//!   (композитор не показывает — не рисуем, но tick продолжается);
//! - выход из цикла: `App::event -> false`, `App::wants_exit`, закрытие слоя.
//!
//! Каркас (registry/output/seat/shm-хендлеры и delegate-макросы) следует
//! examples/simple_layer.rs из smithay-client-toolkit v0.19.2.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use driftling_core::sprite::Frame;
use driftling_core::Vec2;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopSignal,
        },
        calloop_wayland_source::WaylandSource,
        client::{
            globals::registry_queue_init,
            protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
            Connection, QueueHandle,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT},
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
};

use crate::{App, Event, Scene};

/// Период таймера симуляции (~30 fps).
const TICK: Duration = Duration::from_millis(33);

/// Запустить event loop бэкенда; возвращается после Shutdown.
///
/// Отклонение от заголовка в lib.rs: требуется `'static`, потому что
/// wayland-client хранит состояние диспатча в `Arc<dyn ObjectData>`.
pub fn run(app: impl App + 'static) -> Result<()> {
    let conn =
        Connection::connect_to_env().context("нет соединения с Wayland (WAYLAND_DISPLAY)")?;
    let (globals, event_queue) =
        registry_queue_init::<Backend>(&conn).context("registry_queue_init")?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor недоступен")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("zwlr_layer_shell_v1 недоступен")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm недоступен")?;

    // TODO(M3): мультивыход — по одному surface на каждый wl_output.
    // M0: output = None, композитор сам выбирает (обычно основной) выход.
    let surface = compositor.create_surface(&qh);
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("driftling"), None);
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_exclusive_zone(-1); // игнорировать чужие exclusive-зоны (панели)
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_size(0, 0); // размер назначит композитор в configure
                          // Первый commit без буфера — маппинг слоя; композитор ответит configure.
    layer.commit();

    // Пул вырастет сам при первом create_buffer нужного размера.
    let pool = SlotPool::new(4096, &shm).context("SlotPool")?;

    let mut event_loop: EventLoop<Backend> = EventLoop::try_new().context("calloop")?;
    let loop_handle = event_loop.handle();

    let mut backend = Backend {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor_state: compositor,
        shm,
        pool,
        layer,
        pointer: None,
        qh: qh.clone(),
        loop_signal: event_loop.get_signal(),
        app: Box::new(app),
        start: Instant::now(),
        width: 0,
        height: 0,
        scale: 1,
        configured: false,
        frame_done: true,
        exit: false,
    };

    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| anyhow!("wayland source: {e}"))?;

    // Таймер симуляции: тикает всегда, даже если композитор не показывает кадры.
    loop_handle
        .insert_source(
            Timer::from_duration(TICK),
            |_deadline, _, state: &mut Backend| {
                state.tick_and_draw();
                TimeoutAction::ToDuration(TICK)
            },
        )
        .map_err(|e| anyhow!("таймер симуляции: {e}"))?;

    event_loop
        .run(Duration::from_millis(500), &mut backend, |_| {})
        .context("event loop")?;
    Ok(())
}

/// Состояние бэкенда: SCTK-обвязка + приложение.
struct Backend {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    pointer: Option<wl_pointer::WlPointer>,
    qh: QueueHandle<Self>,
    loop_signal: LoopSignal,
    app: Box<dyn App>,

    start: Instant,
    /// Логический размер surface (из configure).
    width: u32,
    height: u32,
    /// Целочисленный масштаб буфера (HiDPI).
    scale: u32,
    /// Был ли хотя бы один configure (до него не рисуем).
    configured: bool,
    /// Прошлый кадр показан (frame callback вернулся) — можно рисовать снова.
    frame_done: bool,
    exit: bool,
}

impl Backend {
    /// Монотонные секунды от старта бэкенда.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Отдать событие приложению; false от него — сигнал завершения.
    fn deliver(&mut self, ev: Event) {
        let now = self.now();
        if !self.app.event(ev, now) {
            self.exit = true;
            self.loop_signal.stop();
        }
    }

    /// Шаг симуляции + (если композитор готов) отрисовка кадра.
    fn tick_and_draw(&mut self) {
        if self.exit {
            self.loop_signal.stop();
            return;
        }
        if !self.configured {
            return;
        }
        let now = self.now();
        let scale = self.scale.max(1);
        let (pw, ph) = (self.width * scale, self.height * scale);
        let do_draw = self.frame_done && pw > 0 && ph > 0;

        // Scene заимствует self.app; дальше трогаем только другие поля.
        let scene = self.app.tick(now);
        if do_draw {
            match self.pool.create_buffer(
                pw as i32,
                ph as i32,
                pw as i32 * 4,
                wl_shm::Format::Argb8888,
            ) {
                Ok((buffer, canvas)) => {
                    compose(canvas, (pw, ph), &scene, scale);

                    let surface = self.layer.wl_surface();
                    surface.set_buffer_scale(scale as i32);

                    // Клик-сквозь: input region = объединение input_rects.
                    // Пустая сцена → пустой регион (None означал бы «вся
                    // поверхность ловит мышь» — этого не хотим никогда).
                    match Region::new(&self.compositor_state) {
                        Ok(region) => {
                            for r in &scene.input_rects {
                                let x0 = r.x.floor() as i32;
                                let y0 = r.y.floor() as i32;
                                let x1 = (r.x + r.w).ceil() as i32;
                                let y1 = (r.y + r.h).ceil() as i32;
                                region.add(x0, y0, x1 - x0, y1 - y0);
                            }
                            surface.set_input_region(Some(region.wl_region()));
                        }
                        Err(e) => log::error!("wl_region: {e}"),
                    }

                    surface.damage_buffer(0, 0, pw as i32, ph as i32);
                    // Следующий кадр рисуем только после frame callback.
                    surface.frame(&self.qh, surface.clone());
                    if let Err(e) = buffer.attach_to(surface) {
                        log::error!("attach буфера: {e}");
                    }
                    self.layer.commit();
                    self.frame_done = false;
                }
                Err(e) => log::error!("shm-буфер {pw}x{ph}: {e}"),
            }
        }
        drop(scene);

        if self.exit || self.app.wants_exit() {
            self.loop_signal.stop();
        }
    }
}

/// Собрать кадр: прозрачный фон + все спрайты сцены.
/// `size` — физический размер холста (уже * scale).
fn compose(canvas: &mut [u8], size: (u32, u32), scene: &Scene<'_>, scale: u32) {
    canvas.fill(0); // 0x00000000 — полностью прозрачно
    for s in &scene.sprites {
        let origin = (s.origin.x.round() as i32, s.origin.y.round() as i32);
        blit(canvas, size, s.frame, origin, s.mirror, scale);
    }
}

/// Блит спрайта на холст: nearest-neighbour масштаб, опциональное
/// горизонтальное зеркало, пропуск пикселей с альфой 0, клип по краям.
/// `origin` — логические координаты левого верхнего угла спрайта.
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
        // Новый масштаб подхватит ближайший tick (≤33 мс) — там же
        // set_buffer_scale и буфер нового размера.
        self.scale = new_factor.max(1) as u32;
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
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // Композитор показал кадр — разрешаем следующую отрисовку.
        // Саму отрисовку делает таймер симуляции, чтобы не гнать выше 30 fps.
        self.frame_done = true;
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Backend {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    // TODO(M3): мультивыход — реагировать на new/update/destroyed,
    // создавая/убирая overlay-surface на каждом выходе.
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Backend {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        // Композитор закрыл слой (выход исчез и т.п.) — завершаемся.
        self.deliver(Event::Shutdown);
        self.exit = true;
        self.loop_signal.stop();
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        if w == 0 || h == 0 {
            // Композитор не назначил размер (не должен при полном anchor);
            // ждём следующего configure.
            return;
        }
        let changed = w != self.width || h != self.height;
        self.width = w;
        self.height = h;
        let first = !self.configured;
        if first || changed {
            // Геометрия уходит приложению ДО первого tick/draw.
            self.deliver(Event::OutputGeometry {
                width: w as f32,
                height: h as f32,
            });
        }
        self.configured = true;
        if (first || changed) && !self.exit {
            // Первый кадр сразу, не дожидаясь таймера.
            self.tick_and_draw();
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
            // Только наш слой.
            if &event.surface != self.layer.wl_surface() {
                continue;
            }
            let pos = Vec2::new(event.position.0 as f32, event.position.1 as f32);
            let ev = match event.kind {
                // Enter тоже несёт позицию — отдаём как движение.
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    Event::PointerMotion(pos)
                }
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    Event::PointerPress(pos)
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    Event::PointerRelease(pos)
                }
                _ => continue,
            };
            self.deliver(ev);
            if self.exit {
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
        // Наполовину за левым верхним углом: виден только пиксель (1,1) = прозрачный
        // и (0,0)-часть = пиксель A в (-1,-1)... проверяем, что не паникует и
        // видимая часть корректна.
        blit(&mut canvas, (2, 2), &f, (-1, -1), false, 1);
        assert_eq!(pixel(&canvas, 2, 0, 0), 0); // прозрачный угол спрайта
                                                // За правым нижним краем — тоже без паники.
        blit(&mut canvas, (2, 2), &f, (1, 1), false, 1);
        assert_eq!(pixel(&canvas, 2, 1, 1), 0xff_11_00_00);
    }
}
