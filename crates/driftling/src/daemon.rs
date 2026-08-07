//! Демон: симуляция + платформа + IPC.
//!
//! Связывает:
//! - `driftling_core::Pet` (симуляция, tick + pointer);
//! - `driftling_core::sprite::placeholder` (кадры);
//! - `driftling_platform::wayland::run` (оверлей: App::tick -> Scene);
//! - `driftling_ipc::Server` в отдельном потоке -> канал -> DaemonApp::tick.
//!
//! Контракт App::tick: собрать Scene из текущего кадра питомца
//! (`SpriteSet::frame(state, state_time, facing)`, origin = bounds().{x,y},
//! mirror = facing==Left) и input_rects = [bounds()], когда питомец призван.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use driftling_core::sprite::{placeholder, SpriteSet};
use driftling_core::{
    Config, Direction, Pet, PetAttributes, PetRecord, PointerEvent, Rect, Vec2, World,
};
use driftling_ipc::{Request, Response, Server};
use driftling_platform::{App, Event, Scene, SpriteInstance};

/// Сколько IPC-поток ждёт ответа от цикла приложения, прежде чем сдаться.
const IPC_REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Сообщение из IPC-потока: запрос + канал для ровно одного ответа.
type IpcMessage = (Request, Sender<Response>);

/// Запустить демон: IPC-сервер в своём потоке, симуляция — в цикле бэкенда.
pub fn run() -> Result<()> {
    let (tx, rx) = mpsc::channel::<IpcMessage>();

    // IPC-поток: каждый запрос пробрасывается в цикл приложения, ответ
    // ждём с таймаутом (цикл мог зависнуть — клиент не должен висеть вечно).
    // После Quit serve() возвращается сам (контракт driftling_ipc) — поток
    // завершается штатно.
    let server = Server::bind()?;
    std::thread::spawn(move || {
        let result = server.serve(|req| {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send((req, reply_tx)).is_err() {
                // Приложение уже уронило приёмник — отвечаем за него.
                return Response::Error("демон завершается".into());
            }
            reply_rx
                .recv_timeout(IPC_REPLY_TIMEOUT)
                .unwrap_or_else(|_| Response::Error("демон не ответил вовремя".into()))
        });
        if let Err(e) = result {
            log::error!("IPC-сервер завершился с ошибкой: {e:#}");
        }
    });

    let record = load_pet_record();
    let size = record.attributes.clamped().size;
    let mut app = DaemonApp::new(placeholder(size), rx);
    app.record = record;
    driftling_platform::wayland::run(app)
}

/// Загрузить запись питомца. Нет файла — мигрируем характеристики из
/// legacy-секций config.toml (до переезда в pet.json) либо создаём дефолт;
/// в обоих случаях сразу сохраняем.
fn load_pet_record() -> PetRecord {
    match PetRecord::load() {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            let mut rec = PetRecord::default();
            if let Some(attrs) = driftling_core::config::raw_text()
                .and_then(|t| driftling_core::config::legacy_attributes(&t))
            {
                log::info!("миграция характеристик из legacy config.toml");
                rec.attributes = attrs;
            }
            if let Err(e) = rec.save() {
                log::warn!("pet.json не сохранён: {e}");
            }
            rec
        }
        Err(e) => {
            log::warn!("pet.json не прочитан ({e}), используем дефолт");
            PetRecord::default()
        }
    }
}

/// Состояние демона между кадрами. Владеет SpriteSet: Scene заимствует
/// кадры из него, поэтому tick возвращает Scene<'_> с временем жизни self.
struct DaemonApp {
    sprites: SpriteSet,
    /// Персистентная запись питомца: имя + характеристики (pet.json).
    record: PetRecord,
    /// None = питомец убран (dismiss).
    pet: Option<Pet>,
    /// Появляется с первым Event::OutputGeometry; до него сцена пустая.
    world: Option<World>,
    /// Взводится по IPC Quit; бэкенд проверяет через wants_exit.
    exit: bool,
    /// Старт демона — для uptime в Status.
    started: Instant,
    /// `now` прошлого тика для вычисления dt.
    last_now: Option<f64>,
    rx: Receiver<IpcMessage>,
}

impl DaemonApp {
    fn new(sprites: SpriteSet, rx: Receiver<IpcMessage>) -> Self {
        Self {
            sprites,
            record: PetRecord::default(),
            pet: None,
            world: None,
            exit: false,
            started: Instant::now(),
            last_now: None,
            rx,
        }
    }

    /// Призвать питомца (идемпотентно). Требует известной геометрии выхода.
    fn summon(&mut self, now: f64) -> Response {
        let Some(world) = &self.world else {
            return Response::Error("выход ещё не готов (нет геометрии)".into());
        };
        if self.pet.is_none() {
            let pos = Vec2::new(world.screen.w / 2.0, world.screen.h * 0.3);
            self.pet = Some(Pet::new(
                pos,
                self.sprites.size as f32,
                self.record.attributes.behavior_config(),
                // Сид из битов монотонного времени: дёшево и достаточно.
                now.to_bits(),
            ));
            log::info!("summon: питомец появился в ({:.0}, {:.0})", pos.x, pos.y);
        }
        Response::Ok
    }

    /// Обработать один IPC-запрос. Каждый запрос получает ровно один ответ.
    fn handle(&mut self, req: Request, now: f64) -> Response {
        match req {
            Request::Summon => self.summon(now),
            Request::Dismiss => {
                if self.pet.take().is_some() {
                    log::info!("dismiss: питомец убран с экрана");
                }
                Response::Ok
            }
            Request::Status => Response::Status {
                pets: u32::from(self.pet.is_some()),
                state: match &self.pet {
                    Some(pet) => format!("{:?}", pet.state),
                    None => "dismissed".into(),
                },
                uptime_secs: self.started.elapsed().as_secs(),
            },
            Request::PetInfo => Response::PetInfo {
                name: self.record.name.clone(),
                state: self.pet.as_ref().map(|p| format!("{:?}", p.state)),
                attributes: self.record.attributes,
                uptime_secs: self.started.elapsed().as_secs(),
            },
            Request::SetAttributes(attrs) => self.set_attributes(attrs),
            Request::Reload => self.reload(),
            Request::Quit => {
                log::info!("quit: завершаем демон по IPC");
                self.exit = true;
                Response::Ok
            }
        }
    }

    /// Дебаг-панель: задать характеристики напрямую. Клампим, применяем к
    /// живому питомцу, персистим в pet.json.
    fn set_attributes(&mut self, attrs: PetAttributes) -> Response {
        let a = attrs.clamped();
        if a.size != self.sprites.size {
            self.sprites = placeholder(a.size);
        }
        if let Some(pet) = &mut self.pet {
            pet.apply_config(a.behavior_config(), a.size as f32);
        }
        self.record.attributes = a;
        log::info!("set_attributes: применены {a:?}");
        if let Err(e) = self.record.save() {
            return Response::Error(format!("применено, но pet.json не сохранён: {e}"));
        }
        Response::Ok
    }

    /// Перечитать config.toml (настройки приложения). Характеристики питомца
    /// сюда больше не входят — они меняются только через SetAttributes.
    fn reload(&mut self) -> Response {
        match Config::load() {
            Ok(_cfg) => {
                log::info!("reload: настройки приложения перечитаны");
                Response::Ok
            }
            Err(e) => Response::Error(format!("config.toml не прочитан: {e}")),
        }
    }

    /// Пробросить событие указателя в питомца. Для платформы событие всегда
    /// «обработано» — выходить из цикла оно не просит.
    fn pointer(&mut self, ev: PointerEvent, now: f64) -> bool {
        if let (Some(pet), Some(world)) = (&mut self.pet, &self.world) {
            pet.pointer(world, ev, now as f32);
        }
        true
    }
}

impl App for DaemonApp {
    fn tick(&mut self, now: f64) -> Scene<'_> {
        // Разбираем очередь IPC без блокировки. Клиент мог отвалиться по
        // таймауту — неудачная отправка ответа не считается ошибкой.
        while let Ok((req, reply)) = self.rx.try_recv() {
            let resp = self.handle(req, now);
            let _ = reply.send(resp);
        }

        // dt с прошлого тика; кламп на случай скачков монотонных часов
        // (первый кадр, долгий сон компоситора).
        let dt = (now - self.last_now.unwrap_or(now)).clamp(0.0, 0.1) as f32;
        self.last_now = Some(now);

        if let (Some(pet), Some(world)) = (&mut self.pet, &self.world) {
            pet.tick(world, dt);
        }

        match (&self.pet, &self.world) {
            (Some(pet), Some(_)) => {
                let bounds = pet.bounds();
                Scene {
                    sprites: vec![SpriteInstance {
                        frame: self.sprites.frame(pet.state, pet.state_time, pet.facing),
                        origin: Vec2::new(bounds.x, bounds.y),
                        mirror: pet.facing == Direction::Left,
                    }],
                    input_rects: vec![bounds],
                }
            }
            // До геометрии или после dismiss рисовать нечего и мышь не ловим.
            _ => Scene {
                sprites: Vec::new(),
                input_rects: Vec::new(),
            },
        }
    }

    fn event(&mut self, ev: Event, now: f64) -> bool {
        match ev {
            Event::OutputGeometry { width, height } => {
                log::info!("выход: {width:.0}x{height:.0}");
                let first = self.world.is_none();
                self.world = Some(World {
                    screen: Rect::new(0.0, 0.0, width, height),
                });
                // Демон стартует с питомцем на экране: первая геометрия
                // равносильна призыву.
                if first {
                    self.summon(now);
                }
                true
            }
            Event::PointerPress(p) => self.pointer(PointerEvent::Press(p), now),
            Event::PointerMotion(p) => self.pointer(PointerEvent::Motion(p), now),
            Event::PointerRelease(p) => self.pointer(PointerEvent::Release(p), now),
            Event::Shutdown => false,
        }
    }

    fn wants_exit(&self) -> bool {
        self.exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DaemonApp без Wayland: маленький спрайт и ручной канал запросов.
    fn app() -> (DaemonApp, Sender<IpcMessage>) {
        let (tx, rx) = mpsc::channel();
        (DaemonApp::new(placeholder(32), rx), tx)
    }

    /// Положить запрос в очередь приложения; ответ забирается после tick.
    fn send(tx: &Sender<IpcMessage>, req: Request) -> Receiver<Response> {
        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send((req, reply_tx)).unwrap();
        reply_rx
    }

    fn geometry(app: &mut DaemonApp) {
        assert!(app.event(
            Event::OutputGeometry {
                width: 1920.0,
                height: 1080.0,
            },
            0.0,
        ));
    }

    #[test]
    fn empty_scene_before_geometry() {
        let (mut app, _tx) = app();
        let scene = app.tick(0.0);
        assert!(scene.sprites.is_empty());
        assert!(scene.input_rects.is_empty());
    }

    #[test]
    fn first_geometry_auto_summons() {
        let (mut app, _tx) = app();
        geometry(&mut app);
        let scene = app.tick(0.0);
        assert_eq!(scene.sprites.len(), 1);
        assert_eq!(scene.input_rects.len(), 1);
    }

    #[test]
    fn dismiss_and_summon_toggle_scene() {
        let (mut app, tx) = app();
        geometry(&mut app);

        let reply = send(&tx, Request::Dismiss);
        assert!(app.tick(0.1).sprites.is_empty());
        assert_eq!(reply.recv().unwrap(), Response::Ok);

        let reply = send(&tx, Request::Summon);
        assert_eq!(app.tick(0.2).sprites.len(), 1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
    }

    #[test]
    fn summon_before_geometry_is_error() {
        let (mut app, tx) = app();
        let reply = send(&tx, Request::Summon);
        app.tick(0.0);
        assert!(matches!(reply.recv().unwrap(), Response::Error(_)));
    }

    #[test]
    fn status_reports_pet_then_dismissed() {
        let (mut app, tx) = app();
        geometry(&mut app);

        let reply = send(&tx, Request::Status);
        app.tick(0.0);
        match reply.recv().unwrap() {
            Response::Status { pets, state, .. } => {
                assert_eq!(pets, 1);
                assert_ne!(state, "dismissed");
            }
            other => panic!("неожиданный ответ: {other:?}"),
        }

        let reply = send(&tx, Request::Dismiss);
        app.tick(0.1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);

        let reply = send(&tx, Request::Status);
        app.tick(0.2);
        match reply.recv().unwrap() {
            Response::Status { pets, state, .. } => {
                assert_eq!(pets, 0);
                assert_eq!(state, "dismissed");
            }
            other => panic!("неожиданный ответ: {other:?}"),
        }
    }

    #[test]
    fn set_attributes_applies_and_petinfo_reports() {
        // Изолируем pet.json от реального: save() внутри set_attributes.
        let dir = std::env::temp_dir().join(format!("driftling-attrs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_DATA_HOME", &dir);

        let (mut app, tx) = app();
        geometry(&mut app);

        let attrs = PetAttributes {
            size: 128,
            walk_speed: 200.0,
            ..PetAttributes::default()
        };
        let reply = send(&tx, Request::SetAttributes(attrs));
        app.tick(0.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert_eq!(app.sprites.size, 128);
        assert_eq!(app.pet.as_ref().unwrap().config().walk_speed, 200.0);

        let reply = send(&tx, Request::PetInfo);
        app.tick(0.1);
        match reply.recv().unwrap() {
            Response::PetInfo {
                name,
                state,
                attributes,
                ..
            } => {
                assert_eq!(name, "Дрифтлинг");
                assert!(state.is_some());
                assert_eq!(attributes.size, 128);
                assert_eq!(attributes.walk_speed, 200.0);
            }
            other => panic!("неожиданный ответ: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quit_sets_exit_flag() {
        let (mut app, tx) = app();
        geometry(&mut app);
        assert!(!app.wants_exit());

        let reply = send(&tx, Request::Quit);
        app.tick(0.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(app.wants_exit());
    }

    #[test]
    fn shutdown_event_requests_exit() {
        let (mut app, _tx) = app();
        assert!(!app.event(Event::Shutdown, 0.0));
    }

    #[test]
    fn pet_falls_to_ground_via_ticks() {
        let (mut app, _tx) = app();
        geometry(&mut app);
        // Питомец стартует в падении и должен осесть на нижнюю кромку.
        let mut now = 0.0;
        for _ in 0..1200 {
            now += 1.0 / 60.0;
            app.tick(now);
        }
        let pet = app.pet.as_ref().unwrap();
        assert_eq!(pet.pos.y, 1080.0);
    }

    #[test]
    fn scene_mirrors_when_facing_left() {
        let (mut app, _tx) = app();
        geometry(&mut app);
        app.pet.as_mut().unwrap().facing = Direction::Left;
        let scene = app.tick(0.0);
        assert!(scene.sprites[0].mirror);
    }
}
