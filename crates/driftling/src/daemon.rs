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
//!
//! Живучесть (ТД-18, 19, 20): SIGTERM/SIGINT — graceful-выход с сохранением
//! pet.json; паника — запись в журнал + сохранение записи + abort (рестарт
//! отдаётся systemd, Restart=on-failure); логи — journald под systemd.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use driftling_core::sprite::{placeholder, SpriteSet};
use driftling_core::{
    Config, Direction, Pet, PetAttributes, PetRecord, PointerEvent, Rect, SimPace, Vec2, World,
};
use driftling_ipc::{Request, Response, Server};
use driftling_platform::{App, Event, Pace, Scene, SpriteInstance};

use crate::i18n::fl;

/// Сколько IPC-поток ждёт ответа от цикла приложения, прежде чем сдаться.
const IPC_REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Сообщение из IPC-потока: запрос + канал для ровно одного ответа.
type IpcMessage = (Request, Sender<Response>);

/// Инициализация логирования демона (ТД-19): под systemd/journald пишем
/// прямо в журнал с идентификатором "driftling", иначе — env_logger в
/// stderr. Терпима к уже установленному логгеру (повторный вызов — no-op).
pub fn init_logging() {
    if systemd_journal_logger::connected_to_journal() {
        if let Ok(journal) = systemd_journal_logger::JournalLog::new() {
            if journal
                .with_syslog_identifier("driftling".to_string())
                .install()
                .is_ok()
            {
                log::set_max_level(log::LevelFilter::Info);
                return;
            }
        }
    }
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}

/// Паника не должна молча терять питомца (ТД-18): пишем причину в журнал,
/// сохраняем последнюю известную запись и завершаемся abort'ом — рестарт
/// делает systemd (Restart=on-failure), а не полуживой процесс.
fn install_panic_hook(mirror: Arc<Mutex<PetRecord>>, data_dir: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("паника демона: {info}");
        let record = mirror
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match record.save_in(&data_dir) {
            Ok(()) => log::error!("pet.json сохранён перед аварийным выходом"),
            Err(e) => log::error!("pet.json не сохранён при панике: {e}"),
        }
        default_hook(info);
        std::process::abort();
    }));
}

/// Запустить демон: IPC-сервер в своём потоке, симуляция — в цикле бэкенда.
pub fn run() -> Result<()> {
    init_logging();

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
                return Response::Error(fl!("daemon-shutting-down"));
            }
            reply_rx
                .recv_timeout(IPC_REPLY_TIMEOUT)
                .unwrap_or_else(|_| Response::Error(fl!("daemon-reply-timeout")))
        });
        if let Err(e) = result {
            log::error!("IPC-сервер завершился с ошибкой: {e:#}");
        }
    });

    let data_dir = driftling_core::attributes::data_dir();
    let record = load_pet_record(&data_dir);
    let size = record.attributes.clamped().size;
    let mut app = DaemonApp::new(placeholder(size), rx, data_dir.clone());
    app.set_record(record);

    // SIGTERM/SIGINT (ТД-20): флаг проверяется в tick -> graceful-выход
    // с финальной записью pet.json.
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&app.sig_exit))?;
    }
    install_panic_hook(Arc::clone(&app.record_mirror), data_dir);

    driftling_platform::wayland::run(app)
}

/// Загрузить запись питомца из `data_dir`. Нет файла — мигрируем
/// характеристики из legacy-секций config.toml (до переезда в pet.json)
/// либо создаём дефолт; в обоих случаях сразу сохраняем. Битый файл
/// load_in уже переименовал в pet.json.corrupt-<ts> (улики целы, ТД-15).
///
/// Имя питомца локализуется ровно один раз — при первом создании записи
/// (ТД-30): в pet.json оно хранится как данные и смену локали переживает.
fn load_pet_record(data_dir: &Path) -> PetRecord {
    match PetRecord::load_in(data_dir) {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            let mut rec = PetRecord {
                name: fl!("default-pet-name"),
                ..PetRecord::default()
            };
            if let Some(attrs) = driftling_core::config::raw_text()
                .and_then(|t| driftling_core::config::legacy_attributes(&t))
            {
                log::info!("миграция характеристик из legacy config.toml");
                rec.attributes = attrs;
            }
            if let Err(e) = rec.save_in(data_dir) {
                log::warn!("pet.json не сохранён: {e}");
            }
            rec
        }
        Err(e) => {
            // Файл новее нашей схемы или бэкап не удался: НЕ сохраняем
            // дефолт поверх — работаем на дефолте только в памяти.
            log::warn!("pet.json не прочитан ({e}), работаем на дефолте без записи");
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
    /// Каталог pet.json — DI вместо env-переменных (ТД-26).
    data_dir: PathBuf,
    /// Взводится обработчиком SIGTERM/SIGINT; tick превращает в exit.
    sig_exit: Arc<AtomicBool>,
    /// Зеркало record для panic hook (живёт в замыкании хука).
    record_mirror: Arc<Mutex<PetRecord>>,
}

impl DaemonApp {
    fn new(sprites: SpriteSet, rx: Receiver<IpcMessage>, data_dir: PathBuf) -> Self {
        Self {
            sprites,
            record: PetRecord::default(),
            pet: None,
            world: None,
            exit: false,
            started: Instant::now(),
            last_now: None,
            rx,
            data_dir,
            sig_exit: Arc::new(AtomicBool::new(false)),
            record_mirror: Arc::new(Mutex::new(PetRecord::default())),
        }
    }

    /// Установить запись и синхронизировать зеркало panic hook'а.
    fn set_record(&mut self, record: PetRecord) {
        *self
            .record_mirror
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = record.clone();
        self.record = record;
    }

    /// Сохранить запись на диск и обновить зеркало для panic hook.
    fn persist_record(&self) -> Result<(), String> {
        *self
            .record_mirror
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.record.clone();
        self.record.save_in(&self.data_dir)
    }

    /// Призвать питомца (идемпотентно). Требует известной геометрии выхода.
    fn summon(&mut self, now: f64) -> Response {
        let Some(world) = &self.world else {
            return Response::Error(fl!("daemon-output-not-ready"));
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
            Request::Summon => {
                let resp = self.summon(now);
                // Призванность переживает рестарт (ТД-17).
                if resp == Response::Ok && !self.record.summoned {
                    self.record.summoned = true;
                    if let Err(e) = self.persist_record() {
                        return Response::Error(fl!("daemon-summoned-not-saved", error = e));
                    }
                }
                resp
            }
            Request::Dismiss => {
                if self.pet.take().is_some() {
                    log::info!("dismiss: питомец убран с экрана");
                }
                // Убранность переживает рестарт (ТД-17).
                if self.record.summoned {
                    self.record.summoned = false;
                    if let Err(e) = self.persist_record() {
                        return Response::Error(fl!("daemon-dismissed-not-saved", error = e));
                    }
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
        if let Err(e) = self.persist_record() {
            return Response::Error(fl!("daemon-applied-not-saved", error = e));
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
            Err(e) => Response::Error(fl!("daemon-config-unreadable", error = e)),
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
        // SIGTERM/SIGINT: graceful-выход с финальной записью (ТД-20).
        if self.sig_exit.load(Ordering::Relaxed) && !self.exit {
            log::info!("получен SIGTERM/SIGINT: сохраняемся и выходим");
            if let Err(e) = self.persist_record() {
                log::warn!("pet.json не сохранён при выходе: {e}");
            }
            self.exit = true;
        }

        // Разбираем очередь IPC без блокировки. Клиент мог отвалиться по
        // таймауту — неудачная отправка ответа не считается ошибкой.
        while let Ok((req, reply)) = self.rx.try_recv() {
            let resp = self.handle(req, now);
            let _ = reply.send(resp);
        }

        // dt с прошлого тика; кламп согласован с Pet::tick (ТД-3: при
        // адаптивном темпе Drowsy тики приходят ~раз в секунду).
        let dt = (now - self.last_now.unwrap_or(now)).clamp(0.0, 1.5) as f32;
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
                // Демон стартует с питомцем на экране — но только если его
                // не убирали до рестарта (ТД-17: dismissed персистентен).
                if first && self.record.summoned {
                    self.summon(now);
                }
                true
            }
            Event::PointerPress(p) => self.pointer(PointerEvent::Press(p), now),
            Event::PointerMotion(p) => self.pointer(PointerEvent::Motion(p), now),
            Event::PointerRelease(p) => self.pointer(PointerEvent::Release(p), now),
            Event::PointerMenu(p) => {
                // Заглушка: контекстное меню питомца приходит в фазе B.
                log::info!(
                    "ПКМ по питомцу в ({:.0}, {:.0}) — меню будет в фазе B",
                    p.x,
                    p.y
                );
                true
            }
            Event::OutputLost => {
                // Бэкенд пересоздаст слой сам; состояние питомца целиком в
                // памяти + pet.json — терять нечего, просто ждём.
                log::warn!(
                    "выход потерян (рестарт композитора?) — состояние сохранено, ждём пересоздания"
                );
                true
            }
            Event::Shutdown => false,
        }
    }

    fn wants_exit(&self) -> bool {
        self.exit
    }

    /// Темп для адаптивного таймера бэкенда (ТД-3): спрашиваем у питомца;
    /// нет питомца или выхода — рисовать нечего, спим (~1 Гц).
    fn pace(&self) -> Pace {
        match (&self.pet, &self.world) {
            (Some(pet), Some(_)) => match pet.pace() {
                SimPace::Active => Pace::Active,
                SimPace::Calm => Pace::Calm,
                SimPace::Drowsy => Pace::Drowsy,
            },
            _ => Pace::Drowsy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use driftling_core::behavior::PetState;

    /// Уникальный каталог данных на тест — DI вместо env-мутаций (ТД-26).
    fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("driftling-daemon-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// DaemonApp без Wayland: маленький спрайт и ручной канал запросов.
    fn app_in(dir: &Path) -> (DaemonApp, Sender<IpcMessage>) {
        let (tx, rx) = mpsc::channel();
        (DaemonApp::new(placeholder(32), rx, dir.to_path_buf()), tx)
    }

    fn app(tag: &str) -> (DaemonApp, Sender<IpcMessage>, PathBuf) {
        let dir = tmp_dir(tag);
        let (app, tx) = app_in(&dir);
        (app, tx, dir)
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
        let (mut app, _tx, dir) = app("empty");
        let scene = app.tick(0.0);
        assert!(scene.sprites.is_empty());
        assert!(scene.input_rects.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_geometry_auto_summons() {
        let (mut app, _tx, dir) = app("autosummon");
        geometry(&mut app);
        let scene = app.tick(0.0);
        assert_eq!(scene.sprites.len(), 1);
        assert_eq!(scene.input_rects.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dismiss_and_summon_toggle_scene() {
        let (mut app, tx, dir) = app("toggle");
        geometry(&mut app);

        let reply = send(&tx, Request::Dismiss);
        assert!(app.tick(0.1).sprites.is_empty());
        assert_eq!(reply.recv().unwrap(), Response::Ok);

        let reply = send(&tx, Request::Summon);
        assert_eq!(app.tick(0.2).sprites.len(), 1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ТД-17: dismiss/summon персистят summoned, и «рестартовавший» демон
    /// с summoned=false НЕ призывает питомца на первой геометрии.
    #[test]
    fn dismissed_survives_restart() {
        let (mut app, tx, dir) = app("dismissed");
        geometry(&mut app);

        let reply = send(&tx, Request::Dismiss);
        app.tick(0.1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);

        let saved = PetRecord::load_in(&dir).unwrap().expect("запись сохранена");
        assert!(!saved.summoned, "dismiss персистит summoned=false");

        // «Рестарт»: новое приложение поднимает запись с диска.
        let (mut app2, tx2) = app_in(&dir);
        app2.set_record(load_pet_record(&dir));
        geometry(&mut app2);
        assert!(
            app2.tick(0.0).sprites.is_empty(),
            "убранный питомец не возвращается сам после рестарта"
        );

        // Явный Summon возвращает питомца и персистит summoned=true.
        let reply = send(&tx2, Request::Summon);
        assert_eq!(app2.tick(0.1).sprites.len(), 1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        let saved = PetRecord::load_in(&dir).unwrap().unwrap();
        assert!(saved.summoned);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summon_before_geometry_is_error() {
        let (mut app, tx, dir) = app("nogeom");
        let reply = send(&tx, Request::Summon);
        app.tick(0.0);
        assert!(matches!(reply.recv().unwrap(), Response::Error(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_reports_pet_then_dismissed() {
        let (mut app, tx, dir) = app("status");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_attributes_applies_and_petinfo_reports() {
        // pet.json изолирован через DI-каталог, env не трогаем (ТД-26).
        let (mut app, tx, dir) = app("attrs");
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

        // Характеристики реально доехали до диска.
        let saved = PetRecord::load_in(&dir).unwrap().unwrap();
        assert_eq!(saved.attributes.size, 128);

        let reply = send(&tx, Request::PetInfo);
        app.tick(0.1);
        match reply.recv().unwrap() {
            Response::PetInfo {
                name,
                state,
                attributes,
                ..
            } => {
                assert_eq!(name, PetRecord::default().name);
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
        let (mut app, tx, dir) = app("quit");
        geometry(&mut app);
        assert!(!app.wants_exit());

        let reply = send(&tx, Request::Quit);
        app.tick(0.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(app.wants_exit());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ТД-20: сигнал завершения превращается в graceful-выход с записью.
    #[test]
    fn sigterm_flag_saves_and_exits() {
        let (mut app, _tx, dir) = app("sigterm");
        geometry(&mut app);
        assert!(!app.wants_exit());

        app.sig_exit.store(true, Ordering::Relaxed);
        app.tick(0.1);
        assert!(app.wants_exit());
        assert!(
            PetRecord::load_in(&dir).unwrap().is_some(),
            "запись сохранена перед выходом"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shutdown_event_requests_exit() {
        let (mut app, _tx, dir) = app("shutdown");
        assert!(!app.event(Event::Shutdown, 0.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Новые события контракта: ПКМ и потеря выхода не роняют цикл.
    #[test]
    fn menu_and_output_lost_are_consumed() {
        let (mut app, _tx, dir) = app("events");
        geometry(&mut app);
        assert!(app.event(Event::PointerMenu(Vec2::new(10.0, 10.0)), 0.0));
        assert!(app.event(Event::OutputLost, 0.1));
        // Питомец и мир пережили потерю выхода.
        assert!(app.pet.is_some());
        assert!(app.world.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ТД-3: демон транслирует SimPace питомца в Pace бэкенда;
    /// без питомца/мира — Drowsy.
    #[test]
    fn pace_follows_pet_state() {
        let (mut app, _tx, dir) = app("pace");
        assert_eq!(app.pace(), Pace::Drowsy, "до геометрии спим");

        geometry(&mut app);
        assert_eq!(app.pace(), Pace::Active, "питомец падает — активный темп");

        let pet = app.pet.as_mut().unwrap();
        pet.state = PetState::Sleep;
        assert_eq!(app.pace(), Pace::Drowsy);
        let pet = app.pet.as_mut().unwrap();
        pet.state = PetState::Idle;
        assert_eq!(app.pace(), Pace::Calm);

        app.pet = None;
        assert_eq!(app.pace(), Pace::Drowsy, "убранный питомец — спим");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pet_falls_to_ground_via_ticks() {
        let (mut app, _tx, dir) = app("falls");
        geometry(&mut app);
        // Питомец стартует в падении и должен осесть на нижнюю кромку.
        let mut now = 0.0;
        for _ in 0..1200 {
            now += 1.0 / 60.0;
            app.tick(now);
        }
        let pet = app.pet.as_ref().unwrap();
        assert_eq!(pet.pos.y, 1080.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scene_mirrors_when_facing_left() {
        let (mut app, _tx, dir) = app("mirror");
        geometry(&mut app);
        app.pet.as_mut().unwrap().facing = Direction::Left;
        let scene = app.tick(0.0);
        assert!(scene.sprites[0].mirror);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
