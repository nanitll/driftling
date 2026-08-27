//! Демон: симуляция + платформа + IPC.
//!
//! Связывает:
//! - `driftling_core::journal` — журнал событий, единственный источник
//!   истины о питомце (имя, характеристики, статы, стадия, summoned);
//! - `driftling_core::Pet` (поведение на экране, tick + pointer);
//! - `driftling_core::sprite::placeholder` (кадры);
//! - `driftling_platform::wayland::run` (оверлей: App::tick -> Scene);
//! - `driftling_ipc::Server` в отдельном потоке -> канал -> DaemonApp::tick.
//!
//! Контракт App::tick: собрать Scene из текущего кадра питомца
//! (`SpriteSet::frame(state, state_time, facing)`, origin = bounds().{x,y},
//! mirror = facing==Left) и input_rects = [bounds()], когда питомец призван.
//!
//! Персистентность (фаза B): каждое действие ухода — событие журнала,
//! записанное на диск в момент команды (append + fsync в `Journal::append`);
//! состояние всегда выводится свёрткой [`fold`]. Кэш свёртки обновляется
//! после каждого события и раз в [`REFOLD_INTERVAL`] (декей для
//! Status/PetInfo). pet.json (schema v3) хранит только device_id для
//! HLC-меток и после старта демоном не пишется.
//!
//! Живучесть (ТД-18, 19, 20): SIGTERM/SIGINT — graceful-выход (сохранять
//! нечего: журнал уже на диске); паника — причина в лог + abort (рестарт
//! отдаётся systemd, Restart=on-failure); логи — journald под systemd.
//!
//! Волна 2 фазы B — визуальный слой (B3/B5/B6):
//! - **Спрайты по стадии/настроению**: набор кадров пересоздаётся при смене
//!   свёрнутой стадии или размера ([`sprite::placeholder_for_stage`]); кадр
//!   выбирается через [`SpriteSet::frame_look`] по полному виду
//!   ([`Look`]: состояние + стадия + настроение + оверлей). Яйцо не ходит —
//!   [`Pet::set_grounded_only`].
//! - **Меню ПКМ (B3)** — главный интерфейс: покормить/вкусняшка/поиграть/
//!   уложить/настройки/убрать. Кадр меню — [`driftling_core::text::menu_frame`],
//!   перепекается только при смене подсвеченной строки; строки меню зовут те
//!   же обработчики, что IPC.
//! - **Видимый уход (B5)**: еда — оверлей [`ActionLook::Eating`]; игра и
//!   поглаживание (клик) — «радостное» окно (семейство happy в Idle);
//!   «Уложить спать» — [`Pet::force_sleep`]; конец любого периода сна
//!   (естественный, drag, dismiss) пишет [`EventKind::Slept`] с настоящей
//!   длительностью (короче [`SLEPT_MIN_MINUTES`] — шум, не пишем).
//! - **Онбординг (B6)**: яйцо сидит на земле; переход стадии Egg -> Baby
//!   (гейт держит fold; `DRIFTLING_GROWTH_SCALE` ускоряет для дебага)
//!   играет [`ActionLook::Hatching`], затем пузырь «Привет!»
//!   ([`driftling_core::text::bubble_frame`]); существующий питомец
//!   здоровается тем же пузырём при старте демона.
//! - **Бюджет**: меню/пузырь/оверлеи поднимают темп ([`Pace::Active`])
//!   только пока видимы; в покое лишних перерисовок и тиков нет.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use driftling_core::sprite::{self, mood_tier, ActionLook, Frame, Look, MoodTier, SpriteSet};
use driftling_core::{
    fold, growth, text, Config, DerivedPet, Direction, Event as JournalEvent, EventKind, FoldCfg,
    HlcClock, Journal, Pet, PetAttributes, PetRecord, PetState, PointerEvent, Rect, SimPace, Stage,
    Vec2, World,
};
use driftling_ipc::{Request, Response, Server};
use driftling_platform::{App, Event, Pace, Scene, SpriteInstance};

use crate::i18n::fl;

/// Сколько IPC-поток ждёт ответа от цикла приложения, прежде чем сдаться.
const IPC_REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Как часто пересворачивать журнал без новых событий: декей статов в
/// Status/PetInfo между командами. Сам fold дешёвый (файлы малы), но
/// гонять его каждый кадр незачем.
const REFOLD_INTERVAL: Duration = Duration::from_secs(60);

/// Кегль строк меню ПКМ, px (язык дизайна настроек, text::menu_frame).
const MENU_PX: f32 = 15.0;
/// Кегль текста речевого пузыря, px.
const BUBBLE_PX: f32 = 14.0;
/// Зазор между пузырём и макушкой питомца, логические px.
const BUBBLE_GAP: f32 = 6.0;
/// Длительность оверлея еды после кормления (B5), сек.
const EATING_SECS: f64 = 2.5;
/// Длительность «радостного» вида после игры (B5), сек.
const HAPPY_PLAY_SECS: f64 = 2.0;
/// Длительность «радостного» вида после поглаживания-клика (B5), сек.
const HAPPY_PET_SECS: f64 = 1.2;
/// Длительность анимации вылупления (B6), сек.
const HATCH_SECS: f64 = 3.0;
/// Пузырь «Привет!» после вылупления (B6), сек.
const HELLO_HATCH_SECS: f64 = 4.0;
/// Пузырь-приветствие при старте демона с призванным питомцем (B5), сек.
const HELLO_START_SECS: f64 = 3.0;
/// Минимальный засчитываемый сон, мин: короче — шум, Slept не пишем.
const SLEPT_MIN_MINUTES: f32 = 0.5;

/// Сообщение из IPC-потока: запрос + канал для ровно одного ответа.
/// Тем же каналом пользуется поток трея (B7).
pub(crate) type IpcMessage = (Request, Sender<Response>);

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
    // zbus (трей) на info дампит целые D-Bus-сообщения — по умолчанию
    // глушим до warn; RUST_LOG переопределяет фильтр целиком.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,zbus=warn"),
    )
    .try_init();
}

/// Настенное unix-время в мс для HLC-меток и свёртки журнала. Демон — не
/// ядро: часы ОС здесь читать можно (ТЗ §3.7 ограничивает только core).
fn wall_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Паника не должна прятаться (ТД-18): причина — в лог, затем abort —
/// рестарт делает systemd (Restart=on-failure), а не полуживой процесс.
/// Спасать состояние не нужно: журнал событий уже на диске (fsync при
/// каждом append), pet.json после старта не меняется.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("паника демона: {info} (журнал событий цел — он пишется сразу)");
        default_hook(info);
        std::process::abort();
    }));
}

/// Запустить демон: IPC-сервер в своём потоке, симуляция — в цикле бэкенда.
pub fn run() -> Result<()> {
    init_logging();

    let (tx, rx) = mpsc::channel::<IpcMessage>();

    // Трей (B7): свой поток, тот же канал запросов, что и у IPC. Нет
    // SNI-вотчера (GNOME без расширения) — просто работаем без трея.
    let tray_tx = tx.clone();
    std::thread::spawn(move || {
        if let Err(e) = crate::tray::run(tray_tx) {
            log::warn!("трей недоступен (нет SNI-вотчера?), работаем без него: {e:#}");
        }
    });

    // IPC-поток: каждый запрос пробрасывается в цикл приложения, ответ
    // ждём с таймаутом (цикл мог зависнуть — клиент не должен висеть вечно).
    // После Quit serve() возвращается сам (контракт driftling_ipc) — поток
    // завершается штатно.
    let server = Server::bind()?;
    let ipc_thread = std::thread::spawn(move || {
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
    // Характеристики из legacy-секций config.toml — только при первом
    // рождении питомца (см. load_storage); читаются здесь, чтобы тесты
    // DaemonApp не зависели от реального конфига (ТД-26).
    let legacy_attrs = driftling_core::config::raw_text()
        .and_then(|t| driftling_core::config::legacy_attributes(&t));
    // Дебаг-ускорение роста (B6): env читается один раз на старте и уходит
    // в FoldCfg параметром — тесты DaemonApp env не трогают (ТД-26).
    let growth_scale = std::env::var("DRIFTLING_GROWTH_SCALE")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0);
    if let Some(scale) = growth_scale {
        log::warn!("DRIFTLING_GROWTH_SCALE={scale}: рост ускорен (дебаг-режим)");
    }
    let app = DaemonApp::new(rx, data_dir, legacy_attrs, growth_scale);

    // SIGTERM/SIGINT (ТД-20): флаг проверяется в tick -> graceful-выход.
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&app.sig_exit))?;
    }
    install_panic_hook();

    let quit_via_ipc = Arc::clone(&app.quit_via_ipc);
    let result = driftling_platform::wayland::run(app);
    // Выход по ctl quit: дождаться IPC-поток, чтобы ответ «ok» дописался
    // клиенту до конца процесса. serve() после Quit возвращается сам
    // (контракт driftling_ipc), так что join ограничен. При SIGTERM же
    // serve() блокируется в accept — там поток гасится выходом процесса.
    if quit_via_ipc.load(Ordering::Relaxed) {
        let _ = ipc_thread.join();
    }
    result
}

/// Поднятое хранилище питомца: журнал + HLC-часы + признак записи.
struct PetStorage {
    events: Vec<JournalEvent>,
    clock: HlcClock,
    /// false — деградация: pet.json или журнал не читаются (файл новее
    /// нашей версии, бэкап битого файла не удался). Работаем в памяти:
    /// уход действует до рестарта, на диск не пишем ничего.
    writable: bool,
}

/// Загрузить журнал и pet.json (schema v3) из `data_dir`.
///
/// - v1/v2 pet.json `PetRecord::load_in` мигрирует сам: имя/характеристики/
///   summoned уезжают в журнал (Genesis с born_stage = Adult), файл
///   переписывается как v3 (ТД-15);
/// - нет pet.json — первый запуск: создаём v3-запись с device_id;
/// - журнал без Genesis — рождение питомца: пишем Genesis с born_stage =
///   Egg (вылупление и онбординг — B6). Имя локализуется ровно один раз —
///   оно данные журнала и смену локали переживает (ТД-30); характеристики
///   берутся из `legacy_attrs` (миграция старых секций config.toml).
fn load_storage(data_dir: &Path, legacy_attrs: Option<PetAttributes>) -> PetStorage {
    let (record, mut writable) = match PetRecord::load_in(data_dir) {
        Ok(Some(rec)) => (rec, true),
        Ok(None) => {
            let rec = PetRecord::new(driftling_core::random_device_id());
            if let Err(e) = rec.save_in(data_dir) {
                // Не смертельно: device_id доживёт до рестарта, журнал
                // пробуем писать всё равно (append сам скажет, если некуда).
                log::warn!("pet.json не сохранён: {e}");
            }
            (rec, true)
        }
        Err(e) => {
            log::warn!("pet.json не прочитан ({e}) — деградация: работаем в памяти без записи");
            (PetRecord::new(driftling_core::random_device_id()), false)
        }
    };

    let (mut events, warnings) = match Journal::open(data_dir) {
        Ok(pair) => pair,
        Err(e) => {
            log::error!("журнал не прочитан ({e}) — деградация: работаем в памяти без записи");
            writable = false;
            (Vec::new(), 0)
        }
    };
    if warnings > 0 {
        log::warn!("журнал: пропущено битых строк: {warnings}");
    }

    let mut clock = HlcClock::new(record.device_id);
    for ev in &events {
        clock.catch_up(&ev.id);
    }

    // Рождение: журнал без Genesis (первый запуск или журнал утерян).
    if !events
        .iter()
        .any(|e| matches!(e.kind, EventKind::Genesis { .. }))
    {
        let attributes = legacy_attrs
            .inspect(|_| log::info!("миграция характеристик из legacy config.toml"))
            .unwrap_or_default();
        let ev = JournalEvent {
            id: clock.next(wall_now_ms()),
            kind: EventKind::Genesis {
                name: fl!("default-pet-name"),
                attributes,
                born_stage: Stage::Egg,
            },
        };
        if writable {
            if let Err(e) = Journal::append(data_dir, &ev) {
                log::warn!("Genesis не записан в журнал: {e}");
            }
        }
        log::info!("журнал: питомец родился (Genesis)");
        events.push(ev);
    }

    PetStorage {
        events,
        clock,
        writable,
    }
}

/// Кратковременный оверлей-экшен (еда, вылупление) поверх базового вида.
struct Overlay {
    look: ActionLook,
    /// Старт (время приложения, сек) — фаза анимации считается от него.
    from: f64,
    /// Момент окончания; истёкший оверлей снимает expire_effects.
    until: f64,
}

/// Речевой пузырь над питомцем (без хит-области).
struct Bubble {
    frame: Frame,
    /// Показывается с этого момента (вылупление: пузырь ждёт конца анимации).
    from: f64,
    until: f64,
}

/// Открытое меню ПКМ (B3): кадр + геометрия в логических экранных координатах.
struct Menu {
    /// Левый верхний угол меню на экране (уже прижат к краям).
    origin: Vec2,
    /// Локализованные строки (пекутся один раз при открытии).
    rows: Vec<String>,
    /// Текущий кадр (перепекается только при смене hovered).
    frame: Frame,
    /// Хит-области строк в координатах кадра (из text::menu_frame).
    rects: Vec<Rect>,
    hovered: Option<usize>,
}

impl Menu {
    /// Прямоугольник меню в логических экранных координатах.
    fn screen_rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.frame.w as f32,
            self.frame.h as f32,
        )
    }

    /// Перепечь кадр под новую подсвеченную строку (дёшево: маленькая
    /// карточка; вызывается только при реальной смене hovered).
    fn rebake(&mut self) {
        let rows: Vec<&str> = self.rows.iter().map(String::as_str).collect();
        let (frame, rects) = text::menu_frame(&rows, self.hovered, MENU_PX);
        self.frame = frame;
        self.rects = rects;
    }
}

/// Действия строк меню ПКМ — в порядке строк [`menu_rows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Feed,
    Treat,
    Play,
    Sleep,
    Settings,
    Dismiss,
}

/// Порядок действий = порядок строк меню (B3).
const MENU_ACTIONS: [MenuAction; 6] = [
    MenuAction::Feed,
    MenuAction::Treat,
    MenuAction::Play,
    MenuAction::Sleep,
    MenuAction::Settings,
    MenuAction::Dismiss,
];

/// Локализованные подписи строк меню, в порядке [`MENU_ACTIONS`].
fn menu_rows() -> Vec<String> {
    vec![
        fl!("menu-feed"),
        fl!("menu-treat"),
        fl!("menu-play"),
        fl!("menu-sleep"),
        fl!("menu-settings"),
        fl!("menu-dismiss"),
    ]
}

/// Индекс строки меню под точкой `local` (координаты кадра меню).
fn hover_index(rects: &[Rect], local: Vec2) -> Option<usize> {
    rects.iter().position(|r| r.contains(local))
}

/// Прижать левый верхний угол меню к экрану так, чтобы меню целиком
/// влезло (меню больше экрана — прижимаем к левому/верхнему краю).
fn clamp_menu_origin(p: Vec2, (w, h): (f32, f32), screen: &Rect) -> Vec2 {
    Vec2::new(
        p.x.clamp(screen.x, (screen.right() - w).max(screen.x)),
        p.y.clamp(screen.y, (screen.bottom() - h).max(screen.y)),
    )
}

/// Минуты сна между моментами приложения `since` и `now`; None — сон
/// короче порога [`SLEPT_MIN_MINUTES`] (шум, в журнал не пишется).
fn slept_minutes(since: f64, now: f64) -> Option<f32> {
    let minutes = ((now - since).max(0.0) / 60.0) as f32;
    (minutes >= SLEPT_MIN_MINUTES).then_some(minutes)
}

/// Пузырь «Привет!» с окном показа [from, from + secs).
fn hello_bubble(from: f64, secs: f64) -> Bubble {
    Bubble {
        frame: text::bubble_frame(&fl!("bubble-hello"), BUBBLE_PX),
        from,
        until: from + secs,
    }
}

/// Запустить окно настроек не блокируясь (пункт меню «Настройки»).
/// Та же стратегия поиска бинаря, что в tray.rs/main.rs: рядом с собой,
/// затем в PATH; ребёнка дожидается отдельный поток (не плодим зомби).
fn spawn_settings_detached() {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("driftling-settings")))
        .filter(|p| p.exists());
    let program = sibling.unwrap_or_else(|| "driftling-settings".into());
    match std::process::Command::new(&program).spawn() {
        Ok(mut child) => {
            log::info!("меню: настройки запущены ({program:?}, pid {})", child.id());
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("меню: не удалось запустить {program:?}: {e}"),
    }
}

/// Состояние демона между кадрами. Владеет SpriteSet: Scene заимствует
/// кадры из него, поэтому tick возвращает Scene<'_> с временем жизни self.
struct DaemonApp {
    sprites: SpriteSet,
    /// Журнал событий в памяти (отсортирован по id) — источник истины.
    events: Vec<JournalEvent>,
    /// HLC-часы устройства (device_id из pet.json v3); подтянуты под
    /// журнал при старте — новые метки строго больше записанных.
    clock: HlcClock,
    /// Игровые константы свёртки.
    fold_cfg: FoldCfg,
    /// Кэш свёртки журнала; обновляется в append_event и по таймеру tick.
    derived: DerivedPet,
    /// Монотонное время последней свёртки (таймер REFOLD_INTERVAL).
    last_fold: Instant,
    /// Журнал пишется на диск (false — деградация, см. load_storage).
    journal_writable: bool,
    /// Базовый размер спрайта (attributes.size), под который сгенерирован
    /// текущий набор кадров (сам набор отмасштабирован стадией).
    sprite_base: u32,
    /// Стадия, под которую сгенерирован набор, — детектор смены стадии
    /// (регенерация спрайтов + анимация вылупления Egg -> Baby).
    sprite_stage: Stage,
    /// Активный оверлей-экшен (еда/вылупление), B5/B6.
    overlay: Option<Overlay>,
    /// Речевой пузырь над питомцем (приветствие), B5/B6.
    bubble: Option<Bubble>,
    /// До этого момента питомец выглядит «радостно» (игра/поглаживание, B5).
    happy_until: Option<f64>,
    /// Открытое меню ПКМ (B3); пока Some — pace() держит Active.
    menu: Option<Menu>,
    /// Начало текущего периода сна (время приложения) — для события Slept.
    sleep_since: Option<f64>,
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
    /// Взводится обработчиком IPC Quit. run() по нему ждёт IPC-поток:
    /// serve() после Quit возвращается по контракту driftling_ipc, а без
    /// ожидания процесс мог умереть раньше, чем поток соединения допишет
    /// ответ клиенту — ctl quit изредка получал EOF вместо «ok».
    quit_via_ipc: Arc<AtomicBool>,
    rx: Receiver<IpcMessage>,
    /// Каталог данных (журнал + pet.json) — DI вместо env-переменных (ТД-26).
    data_dir: PathBuf,
    /// Взводится обработчиком SIGTERM/SIGINT; tick превращает в exit.
    sig_exit: Arc<AtomicBool>,
}

impl DaemonApp {
    /// Поднять демона из каталога данных: журнал -> свёртка -> спрайты
    /// под текущие характеристики и стадию. `legacy_attrs` — миграция
    /// старых секций config.toml, применяется только при рождении питомца;
    /// `growth_scale` — дебаг-ускорение роста (DRIFTLING_GROWTH_SCALE).
    fn new(
        rx: Receiver<IpcMessage>,
        data_dir: PathBuf,
        legacy_attrs: Option<PetAttributes>,
        growth_scale: Option<f64>,
    ) -> Self {
        let storage = load_storage(&data_dir, legacy_attrs);
        let mut fold_cfg = FoldCfg::default();
        if let Some(scale) = growth_scale {
            fold_cfg.growth_scale = scale;
        }
        let derived = fold(&storage.events, wall_now_ms(), &fold_cfg);
        log::info!(
            "питомец «{}»: стадия {}, событий в журнале: {}",
            derived.name,
            derived.stage.as_str(),
            storage.events.len()
        );
        let size = derived.attributes.clamped().size;
        let stage = derived.stage;
        Self {
            sprites: sprite::placeholder_for_stage(size, stage),
            events: storage.events,
            clock: storage.clock,
            fold_cfg,
            derived,
            last_fold: Instant::now(),
            journal_writable: storage.writable,
            sprite_base: size,
            sprite_stage: stage,
            overlay: None,
            bubble: None,
            happy_until: None,
            menu: None,
            sleep_since: None,
            pet: None,
            world: None,
            exit: false,
            started: Instant::now(),
            last_now: None,
            quit_via_ipc: Arc::new(AtomicBool::new(false)),
            rx,
            data_dir,
            sig_exit: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Пересвернуть журнал в текущее состояние (декей до «сейчас»).
    fn refold(&mut self) {
        self.derived = fold(&self.events, wall_now_ms(), &self.fold_cfg);
        self.last_fold = Instant::now();
    }

    /// Дописать событие ухода: диск (append + fsync) -> память -> свёртка.
    /// Ошибка диска = событие не случилось (Err наружу, состояние не
    /// трогаем). В деградации событие живёт только в памяти — уход
    /// работает, но рестарт его забудет (залогировано при старте).
    fn append_event(&mut self, kind: EventKind) -> Result<(), String> {
        let ev = JournalEvent {
            id: self.clock.next(wall_now_ms()),
            kind,
        };
        if self.journal_writable {
            Journal::append(&self.data_dir, &ev)?;
        }
        // Часы подтянуты под журнал при старте: новый id строго больше
        // всех прежних, сортировка сохраняется без пересортировки.
        self.events.push(ev);
        self.refold();
        Ok(())
    }

    /// Команда ухода: событие в журнал + Ok.
    fn care(&mut self, kind: EventKind) -> Response {
        log::info!("уход: {kind:?}");
        match self.append_event(kind) {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error(fl!("daemon-journal-append-failed", error = e)),
        }
    }

    /// Есть ли на экране питомец, которому видны реакции ухода. Яйцо
    /// исключается: у него нет рта и «радости» — все оверлеи, кроме
    /// вылупления, выглядели бы как чужой блоб поверх скорлупы.
    fn pet_visible_reactive(&self) -> bool {
        self.pet.is_some() && self.derived.stage != Stage::Egg
    }

    /// Спящего питомца будит явное действие ухода (еда/игра): дальше он
    /// снова решает сам. Конец сна заметит note_sleep текущего тика.
    fn wake_pet_for_action(&mut self) {
        if let Some(pet) = &mut self.pet {
            if pet.state == PetState::Sleep {
                pet.state = PetState::Idle;
                pet.state_time = 0.0;
                pet.state_left = 1.5;
            }
        }
    }

    /// Покормить (IPC и меню ПКМ): событие журнала + видимая еда (B5).
    fn feed(&mut self, treat: bool, now: f64) -> Response {
        let resp = self.care(EventKind::Fed { treat });
        if resp == Response::Ok && self.pet_visible_reactive() {
            self.wake_pet_for_action();
            self.overlay = Some(Overlay {
                look: ActionLook::Eating,
                from: now,
                until: now + EATING_SECS,
            });
        }
        resp
    }

    /// Поиграть (IPC и меню ПКМ): событие журнала + радостный вид (B5).
    fn play(&mut self, now: f64) -> Response {
        let resp = self.care(EventKind::Played);
        if resp == Response::Ok && self.pet_visible_reactive() {
            self.wake_pet_for_action();
            self.happy_until = Some(now + HAPPY_PLAY_SECS);
        }
        resp
    }

    /// Уложить спать (IPC и меню ПКМ): событие журнала + сон прямо сейчас
    /// (B5). Энергию вернёт Slept, когда сон закончится (note_sleep).
    fn put_to_sleep(&mut self) -> Response {
        let resp = self.care(EventKind::PutToSleep);
        if resp == Response::Ok {
            if let Some(pet) = &mut self.pet {
                if pet.force_sleep() {
                    log::info!("сон: питомец уложен принудительно");
                }
            }
        }
        resp
    }

    /// Убрать питомца с экрана (IPC, меню, трей): закрыть период сна,
    /// снять визуальные эффекты, записать Dismissed (ТД-17).
    fn dismiss(&mut self, now: f64) -> Response {
        self.close_sleep(now);
        self.clear_effects();
        if self.pet.take().is_some() {
            log::info!("dismiss: питомец убран с экрана");
        }
        if self.derived.summoned {
            return self.care(EventKind::Dismissed);
        }
        Response::Ok
    }

    /// Поглаживание (B5): потреблённый клик по питомцу — событие Petted
    /// и короткое «радостное» окно.
    fn petted(&mut self, now: f64) {
        if self.pet_visible_reactive() {
            self.happy_until = Some(now + HAPPY_PET_SECS);
        }
        let _ = self.care(EventKind::Petted);
    }

    /// Бухгалтерия сна (B5): заметить начало периода сна и его конец —
    /// по окончании записать Slept с настоящей длительностью. Вызывается
    /// каждый тик после симуляции; конец сна через dismiss закрывает
    /// close_sleep (питомца в этот момент уже нет).
    fn note_sleep(&mut self, now: f64) {
        let sleeping = self
            .pet
            .as_ref()
            .is_some_and(|p| p.state == PetState::Sleep);
        match (self.sleep_since, sleeping) {
            (None, true) => self.sleep_since = Some(now),
            (Some(_), false) => self.close_sleep(now),
            _ => {}
        }
    }

    /// Закрыть текущий период сна: длиннее порога — событие Slept.
    fn close_sleep(&mut self, now: f64) {
        let Some(since) = self.sleep_since.take() else {
            return;
        };
        if let Some(minutes) = slept_minutes(since, now) {
            let _ = self.care(EventKind::Slept { minutes });
        }
    }

    /// Снять все кратковременные визуальные состояния (dismiss).
    fn clear_effects(&mut self) {
        self.menu = None;
        self.overlay = None;
        self.bubble = None;
        self.happy_until = None;
    }

    /// Снять истёкшие эффекты — после этого pace() честно деэскалирует.
    fn expire_effects(&mut self, now: f64) {
        if self.overlay.as_ref().is_some_and(|o| now >= o.until) {
            self.overlay = None;
        }
        if self.bubble.as_ref().is_some_and(|b| now >= b.until) {
            self.bubble = None;
        }
        if self.happy_until.is_some_and(|t| now >= t) {
            self.happy_until = None;
        }
    }

    /// Пора ли пересворачивать журнал ради гейта вылупления: свёртка ещё
    /// в яйце, а настенное время уже пересекло гейт (с учётом дебаг-
    /// ускорения). Дешёвое сравнение на каждый тик — вылупление не ждёт
    /// минутного таймера REFOLD_INTERVAL.
    fn egg_gate_due(&self) -> bool {
        if self.derived.stage != Stage::Egg {
            return false;
        }
        let scale = self.fold_cfg.growth_scale;
        if scale <= 0.0 {
            return false;
        }
        let gate_ms = (growth::EGG_UNTIL_MS as f64 / scale).ceil() as u64;
        wall_now_ms() >= self.derived.born_ms.saturating_add(gate_ms)
    }

    /// Догнать спрайты до свёртки (B4/B6): смена стадии или базового
    /// размера пересоздаёт набор кадров и перенастраивает питомца; переход
    /// Egg -> дальше играет вылупление и пузырь «Привет!».
    fn sync_visuals(&mut self, now: f64) {
        let stage = self.derived.stage;
        let base = self.derived.attributes.clamped().size;
        if stage == self.sprite_stage && base == self.sprite_base {
            return;
        }
        let hatched = self.sprite_stage == Stage::Egg && stage > Stage::Egg;
        self.sprites = sprite::placeholder_for_stage(base, stage);
        self.sprite_stage = stage;
        self.sprite_base = base;
        let attrs = self.derived.attributes;
        if let Some(pet) = &mut self.pet {
            pet.apply_config(attrs.behavior_config(), self.sprites.size as f32);
            pet.set_grounded_only(stage == Stage::Egg);
        }
        if hatched && self.pet.is_some() {
            log::info!("вылупление: Egg -> {}", stage.as_str());
            self.overlay = Some(Overlay {
                look: ActionLook::Hatching,
                from: now,
                until: now + HATCH_SECS,
            });
            self.bubble = Some(hello_bubble(now + HATCH_SECS, HELLO_HATCH_SECS));
        }
    }

    /// Открыть меню ПКМ около точки `p` (прижав к краям экрана).
    /// Во время drag меню не открывается: пока оно открыто, события
    /// указателя не доходят до питомца, и Release потерялся бы — питомец
    /// завис бы в Dragged (родственник ТД-5).
    fn open_menu(&mut self, p: Vec2) {
        let (Some(pet), Some(world)) = (&self.pet, &self.world) else {
            return;
        };
        if pet.state == PetState::Dragged {
            return;
        }
        let rows = menu_rows();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let (frame, rects) = text::menu_frame(&refs, None, MENU_PX);
        let origin = clamp_menu_origin(p, (frame.w as f32, frame.h as f32), &world.screen);
        self.menu = Some(Menu {
            origin,
            rows,
            frame,
            rects,
            hovered: None,
        });
    }

    /// Движение курсора при открытом меню: пересчитать подсветку строки;
    /// кадр перепекается только при реальной смене hovered.
    fn menu_hover(&mut self, p: Vec2) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        let local = Vec2::new(p.x - menu.origin.x, p.y - menu.origin.y);
        let hovered = hover_index(&menu.rects, local);
        if hovered != menu.hovered {
            menu.hovered = hovered;
            menu.rebake();
        }
    }

    /// Нажатие при открытом меню: строка -> действие, любое другое место —
    /// просто закрыть. Меню закрывается в обоих случаях; питомцу это
    /// нажатие не отдаётся (закрывающий клик не должен начинать drag).
    fn menu_press(&mut self, p: Vec2, now: f64) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let local = Vec2::new(p.x - menu.origin.x, p.y - menu.origin.y);
        let Some(idx) = hover_index(&menu.rects, local) else {
            return;
        };
        let action = MENU_ACTIONS[idx];
        log::info!("меню: выбрано {action:?}");
        let resp = match action {
            MenuAction::Feed => self.feed(false, now),
            MenuAction::Treat => self.feed(true, now),
            MenuAction::Play => self.play(now),
            MenuAction::Sleep => self.put_to_sleep(),
            MenuAction::Settings => {
                spawn_settings_detached();
                Response::Ok
            }
            MenuAction::Dismiss => self.dismiss(now),
        };
        if let Response::Error(e) = resp {
            log::warn!("меню: действие {action:?} не удалось: {e}");
        }
    }

    /// Призвать питомца (идемпотентно). Требует известной геометрии выхода.
    fn summon(&mut self, now: f64) -> Response {
        let Some(world) = &self.world else {
            return Response::Error(fl!("daemon-output-not-ready"));
        };
        if self.pet.is_none() {
            let pos = Vec2::new(world.screen.w / 2.0, world.screen.h * 0.3);
            let mut pet = Pet::new(
                pos,
                self.sprites.size as f32,
                self.derived.attributes.behavior_config(),
                // Сид из битов монотонного времени: дёшево и достаточно.
                now.to_bits(),
            );
            // Яйцо не ходит (B6) — стоит, где вылупится.
            pet.set_grounded_only(self.derived.stage == Stage::Egg);
            self.pet = Some(pet);
            log::info!("summon: питомец появился в ({:.0}, {:.0})", pos.x, pos.y);
        }
        Response::Ok
    }

    /// Обработать один IPC-запрос. Каждый запрос получает ровно один ответ.
    fn handle(&mut self, req: Request, now: f64) -> Response {
        match req {
            Request::Summon => {
                let resp = self.summon(now);
                // Призванность переживает рестарт (ТД-17) — событием журнала.
                if resp == Response::Ok && !self.derived.summoned {
                    return self.care(EventKind::Summoned);
                }
                resp
            }
            Request::Dismiss => self.dismiss(now),
            Request::Status => Response::Status {
                pets: u32::from(self.pet.is_some()),
                state: match &self.pet {
                    Some(pet) => format!("{:?}", pet.state),
                    None => "dismissed".into(),
                },
                uptime_secs: self.started.elapsed().as_secs(),
            },
            Request::PetInfo => {
                // Карточка всегда со свежим декеем — минутный таймер не ждём.
                self.refold();
                Response::PetInfo {
                    name: self.derived.name.clone(),
                    state: self.pet.as_ref().map(|p| format!("{:?}", p.state)),
                    attributes: self.derived.attributes,
                    stats: self.derived.stats,
                    stage: self.derived.stage,
                    uptime_secs: self.started.elapsed().as_secs(),
                }
            }
            Request::Feed { treat } => self.feed(treat, now),
            Request::Play => self.play(now),
            Request::PutToSleep => self.put_to_sleep(),
            Request::Rename(name) => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return Response::Error(fl!("daemon-rename-empty"));
                }
                self.care(EventKind::Renamed { name })
            }
            Request::SetAttributes(attrs) => self.set_attributes(attrs),
            Request::Reload => self.reload(),
            Request::Quit => {
                log::info!("quit: завершаем демон по IPC");
                // Долгий сон не пропадает при штатном выходе — Slept в журнал.
                self.close_sleep(now);
                self.exit = true;
                // run() дождётся IPC-поток: ответ клиенту должен дописаться
                // до выхода процесса (иначе ctl quit изредка видел EOF).
                self.quit_via_ipc.store(true, Ordering::Relaxed);
                Response::Ok
            }
        }
    }

    /// Дебаг-панель: задать характеристики напрямую. Клампим, применяем
    /// поведение к живому питомцу, персистим событием журнала
    /// (AttributesSet). Смену размера спрайтов доведёт sync_visuals в
    /// этом же тике (набор кадров зависит ещё и от стадии).
    fn set_attributes(&mut self, attrs: PetAttributes) -> Response {
        let a = attrs.clamped();
        if let Some(pet) = &mut self.pet {
            let keep_size = pet.size;
            pet.apply_config(a.behavior_config(), keep_size);
        }
        log::info!("set_attributes: применены {a:?}");
        self.care(EventKind::AttributesSet { attributes: a })
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
    /// «обработано» — выходить из цикла оно не просит. Потреблённый клик
    /// (нажатие без захвата) — поглаживание (B5).
    fn pointer(&mut self, ev: PointerEvent, now: f64) -> bool {
        let mut clicked = false;
        if let (Some(pet), Some(world)) = (&mut self.pet, &self.world) {
            pet.pointer(world, ev, now as f32);
            clicked = pet.take_click();
        }
        if clicked {
            self.petted(now);
        }
        true
    }
}

impl App for DaemonApp {
    fn tick(&mut self, now: f64) -> Scene<'_> {
        // SIGTERM/SIGINT: graceful-выход (ТД-20). Сохранять нечего —
        // журнал пишется на диск в момент каждого события; только текущий
        // период сна закрывается событием Slept, чтобы не пропасть.
        if self.sig_exit.load(Ordering::Relaxed) && !self.exit {
            log::info!("получен SIGTERM/SIGINT: выходим (журнал уже на диске)");
            self.close_sleep(now);
            self.exit = true;
        }

        // Разбираем очередь IPC без блокировки. Клиент мог отвалиться по
        // таймауту — неудачная отправка ответа не считается ошибкой.
        while let Ok((req, reply)) = self.rx.try_recv() {
            let resp = self.handle(req, now);
            let _ = reply.send(resp);
        }

        // Ленивый декей: свёртка раз в минуту; гейт вылупления (B6) не
        // ждёт минутного таймера — egg_gate_due дешёвый.
        if self.last_fold.elapsed() >= REFOLD_INTERVAL || self.egg_gate_due() {
            self.refold();
        }

        // Спрайты догоняют свёртку (стадия/размер, вылупление), истёкшие
        // эффекты снимаются — pace() после тика видит честное состояние.
        self.sync_visuals(now);
        self.expire_effects(now);

        // dt с прошлого тика; кламп согласован с Pet::tick (ТД-3: при
        // адаптивном темпе Drowsy тики приходят ~раз в секунду).
        let dt = (now - self.last_now.unwrap_or(now)).clamp(0.0, 1.5) as f32;
        self.last_now = Some(now);

        if let (Some(pet), Some(world)) = (&mut self.pet, &self.world) {
            pet.tick(world, dt);
        }

        // Периоды сна: начало/конец (естественный или прерванный) — Slept.
        self.note_sleep(now);

        match (&self.pet, &self.world) {
            (Some(pet), Some(world)) => {
                let bounds = pet.bounds();
                // Полный вид (B2/B5): настроение из статов; «радостное»
                // окно после игры/поглаживания перекрывает настроение.
                let mood = if self.happy_until.is_some() {
                    MoodTier::Happy
                } else {
                    mood_tier(&self.derived.stats)
                };
                let look = Look {
                    state: pet.state,
                    stage: self.derived.stage,
                    mood,
                    overlay: self.overlay.as_ref().map(|o| o.look),
                };
                // Фаза анимации оверлея — от его старта, не от state_time.
                let t = match &self.overlay {
                    Some(o) => (now - o.from) as f32,
                    None => pet.state_time,
                };
                let mut sprites = vec![SpriteInstance {
                    frame: self.sprites.frame_look(&look, t),
                    origin: Vec2::new(bounds.x, bounds.y),
                    mirror: pet.facing == Direction::Left,
                }];
                let mut input_rects = vec![bounds];
                // Пузырь — над питомцем, в пределах экрана, без хит-области.
                if let Some(bubble) = self.bubble.as_ref().filter(|b| now >= b.from) {
                    let (bw, bh) = (bubble.frame.w as f32, bubble.frame.h as f32);
                    let x = (pet.pos.x - bw / 2.0).clamp(
                        world.screen.x,
                        (world.screen.right() - bw).max(world.screen.x),
                    );
                    let y = (bounds.y - bh - BUBBLE_GAP).max(world.screen.y);
                    sprites.push(SpriteInstance {
                        frame: &bubble.frame,
                        origin: Vec2::new(x, y),
                        mirror: false,
                    });
                }
                // Меню — поверх всего (последним в порядке блита) + хит-зона.
                if let Some(menu) = &self.menu {
                    sprites.push(SpriteInstance {
                        frame: &menu.frame,
                        origin: menu.origin,
                        mirror: false,
                    });
                    input_rects.push(menu.screen_rect());
                }
                Scene {
                    sprites,
                    input_rects,
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
                // не убирали до рестарта (ТД-17: dismissed в журнале).
                if first && self.derived.summoned {
                    let greeted = self.summon(now) == Response::Ok;
                    // Приветствие при старте (B5) — для уже вылупившихся:
                    // яйцо поздоровается после вылупления (B6).
                    if greeted && self.derived.stage != Stage::Egg {
                        self.bubble = Some(hello_bubble(now, HELLO_START_SECS));
                    }
                }
                true
            }
            // Пока меню открыто, указатель принадлежит меню: нажатие —
            // выбор строки/закрытие, движение — подсветка; питомцу эти
            // события не отдаются (клик по меню — не drag и не гладь).
            Event::PointerPress(p) => {
                if self.menu.is_some() {
                    self.menu_press(p, now);
                    return true;
                }
                self.pointer(PointerEvent::Press(p), now)
            }
            Event::PointerMotion(p) => {
                if self.menu.is_some() {
                    self.menu_hover(p);
                    return true;
                }
                self.pointer(PointerEvent::Motion(p), now)
            }
            Event::PointerRelease(p) => {
                if self.menu.is_some() {
                    return true;
                }
                self.pointer(PointerEvent::Release(p), now)
            }
            // ПКМ по питомцу — открыть меню (B3); повторный ПКМ закрывает.
            Event::PointerMenu(p) => {
                if self.menu.take().is_none() {
                    self.open_menu(p);
                }
                true
            }
            Event::OutputLost => {
                // Бэкенд пересоздаст слой сам; состояние питомца целиком в
                // журнале — терять нечего, просто ждём.
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
    /// нет питомца или выхода — рисовать нечего, спим (~1 Гц). Меню,
    /// оверлеи и пузыри держат Active только пока видимы/анимируются —
    /// expire_effects в tick снимает их, и темп деэскалирует сам.
    fn pace(&self) -> Pace {
        let effects_active = self.menu.is_some()
            || self.overlay.is_some()
            || self.bubble.is_some()
            || self.happy_until.is_some();
        match (&self.pet, &self.world) {
            (Some(pet), Some(_)) => {
                if effects_active {
                    return Pace::Active;
                }
                match pet.pace() {
                    SimPace::Active => Pace::Active,
                    SimPace::Calm => Pace::Calm,
                    SimPace::Drowsy => Pace::Drowsy,
                }
            }
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

    /// DaemonApp без Wayland: ручной канал запросов; legacy-конфиг и
    /// growth_scale не подмешиваем — тесты не зависят от реального
    /// config.toml и env (ТД-26).
    fn app_in(dir: &Path) -> (DaemonApp, Sender<IpcMessage>) {
        let (tx, rx) = mpsc::channel();
        (DaemonApp::new(rx, dir.to_path_buf(), None, None), tx)
    }

    fn app(tag: &str) -> (DaemonApp, Sender<IpcMessage>, PathBuf) {
        let dir = tmp_dir(tag);
        let (app, tx) = app_in(&dir);
        (app, tx, dir)
    }

    /// Демон с уже взрослым питомцем (мигрант v2): у взрослого видимы
    /// реакции ухода (у яйца оверлеи еды/радости скрыты).
    fn adult_app(tag: &str) -> (DaemonApp, Sender<IpcMessage>, PathBuf) {
        let dir = tmp_dir(tag);
        std::fs::write(
            dir.join("pet.json"),
            r#"{"schema_version":2,"name":"Взрослый","attributes":{},"summoned":true}"#,
        )
        .unwrap();
        let (app, tx) = app_in(&dir);
        (app, tx, dir)
    }

    /// Центр строки `idx` открытого меню в логических экранных координатах.
    fn row_center(app: &DaemonApp, idx: usize) -> Vec2 {
        let menu = app.menu.as_ref().expect("меню открыто");
        let r = menu.rects[idx];
        Vec2::new(
            menu.origin.x + r.x + r.w / 2.0,
            menu.origin.y + r.y + r.h / 2.0,
        )
    }

    /// Осадить питомца на землю: `secs` секунд симуляции тиками по 1/60.
    fn settle(app: &mut DaemonApp, now: &mut f64, secs: f64) {
        let steps = (secs * 60.0) as usize;
        for _ in 0..steps {
            *now += 1.0 / 60.0;
            app.tick(*now);
        }
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

    /// Виды событий журнала на диске (в порядке id).
    fn journal_kinds(dir: &Path) -> Vec<EventKind> {
        let (events, warnings) = Journal::open(dir).unwrap();
        assert_eq!(warnings, 0, "журнал без битых строк");
        events.into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn empty_scene_before_geometry() {
        let (mut app, _tx, dir) = app("empty");
        let scene = app.tick(0.0);
        assert!(scene.sprites.is_empty());
        assert!(scene.input_rects.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Первый запуск: рождение записано в журнал (Genesis, born_stage=Egg),
    /// pet.json создан как v3 (device_id), свёртка — новорождённый питомец.
    #[test]
    fn first_start_writes_genesis_and_v3_record() {
        let (app, _tx, dir) = app("genesis");
        let kinds = journal_kinds(&dir);
        assert_eq!(kinds.len(), 1);
        assert!(matches!(
            &kinds[0],
            EventKind::Genesis {
                born_stage: Stage::Egg,
                ..
            }
        ));
        let rec = PetRecord::load_in(&dir).unwrap().expect("pet.json создан");
        assert!(!rec.device_id.is_empty());
        assert_eq!(app.derived.name, fl!("default-pet-name"));
        assert_eq!(app.derived.stage, Stage::Egg, "новорождённый — яйцо");
        assert!(app.derived.summoned);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Миграция на уровне демона: v2 pet.json подхватывается стартом —
    /// имя/характеристики в свёртке, стадия Adult, журнал создан.
    #[test]
    fn start_migrates_v2_pet_json() {
        let dir = tmp_dir("migrate");
        std::fs::write(
            dir.join("pet.json"),
            r#"{"schema_version":2,"name":"Старожил","attributes":{"size":128},"summoned":true}"#,
        )
        .unwrap();
        let (app, _tx) = app_in(&dir);
        assert_eq!(app.derived.name, "Старожил");
        assert_eq!(app.derived.attributes.size, 128);
        assert_eq!(app.derived.stage, Stage::Adult, "мигрант не вылупляется");
        assert!(app.derived.summoned);
        assert_eq!(journal_kinds(&dir).len(), 1, "только Genesis");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_geometry_auto_summons() {
        let (mut app, _tx, dir) = app("autosummon");
        geometry(&mut app);
        let scene = app.tick(0.0);
        assert_eq!(scene.sprites.len(), 1);
        assert_eq!(scene.input_rects.len(), 1);
        // Свежее яйцо при старте не здоровается — «Привет!» будет после
        // вылупления (B6).
        assert!(app.bubble.is_none());
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

    /// ТД-17: dismiss/summon пишут события журнала, и «рестартовавший»
    /// демон с Dismissed в хвосте НЕ призывает питомца на первой геометрии.
    #[test]
    fn dismissed_survives_restart() {
        let (mut app, tx, dir) = app("dismissed");
        geometry(&mut app);

        let reply = send(&tx, Request::Dismiss);
        app.tick(0.1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Dismissed
        ));

        // «Рестарт»: новое приложение сворачивает журнал с диска.
        let (mut app2, tx2) = app_in(&dir);
        assert!(!app2.derived.summoned, "dismiss пережил рестарт");
        geometry(&mut app2);
        assert!(
            app2.tick(0.0).sprites.is_empty(),
            "убранный питомец не возвращается сам после рестарта"
        );

        // Явный Summon возвращает питомца и пишет событие Summoned.
        let reply = send(&tx2, Request::Summon);
        assert_eq!(app2.tick(0.1).sprites.len(), 1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Summoned
        ));
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

    /// Команды ухода отвечают Ok, дописывают события в journal.jsonl и
    /// мгновенно отражаются в свёртке (игра снимает энергию).
    #[test]
    fn care_commands_append_journal_events() {
        let (mut app, tx, dir) = app("care");
        geometry(&mut app);

        let requests = [
            Request::Feed { treat: false },
            Request::Feed { treat: true },
            Request::Play,
            Request::PutToSleep,
        ];
        for (i, req) in requests.into_iter().enumerate() {
            let reply = send(&tx, req);
            app.tick(0.1 * (i as f64 + 1.0));
            assert_eq!(reply.recv().unwrap(), Response::Ok);
        }

        let kinds = journal_kinds(&dir);
        assert!(matches!(kinds[0], EventKind::Genesis { .. }));
        assert!(matches!(kinds[1], EventKind::Fed { treat: false }));
        assert!(matches!(kinds[2], EventKind::Fed { treat: true }));
        assert!(matches!(kinds[3], EventKind::Played));
        assert!(matches!(kinds[4], EventKind::PutToSleep));
        assert!(
            app.derived.stats.energy < 100.0,
            "свёртка после Played: энергия снята"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Rename: пустое имя — ошибка без события; валидное — событие
    /// Renamed (с обрезанными пробелами) и новое имя в свёртке.
    #[test]
    fn rename_trims_validates_and_persists() {
        let (mut app, tx, dir) = app("rename");

        let reply = send(&tx, Request::Rename("   ".into()));
        app.tick(0.0);
        assert!(matches!(reply.recv().unwrap(), Response::Error(_)));
        assert_eq!(journal_kinds(&dir).len(), 1, "только Genesis");

        let reply = send(&tx, Request::Rename(" Дрифт ".into()));
        app.tick(0.1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert_eq!(app.derived.name, "Дрифт");
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Renamed { name } if name == "Дрифт"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_attributes_applies_and_petinfo_reports() {
        // Данные изолированы через DI-каталог, env не трогаем (ТД-26).
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
        // Спрайты пересозданы под новый базовый размер С УЧЁТОМ стадии:
        // новорождённый — яйцо, масштаб stage_scale(Egg).
        let expected =
            ((128.0 * driftling_core::sprite::stage_scale(Stage::Egg)).round() as u32).max(16);
        assert_eq!(app.sprites.size, expected);
        assert_eq!(app.pet.as_ref().unwrap().size, expected as f32);
        assert_eq!(app.pet.as_ref().unwrap().config().walk_speed, 200.0);

        // Характеристики реально доехали до журнала.
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::AttributesSet { .. }
        ));

        let reply = send(&tx, Request::PetInfo);
        app.tick(0.1);
        match reply.recv().unwrap() {
            Response::PetInfo {
                name,
                state,
                attributes,
                stats,
                stage,
                ..
            } => {
                assert_eq!(name, fl!("default-pet-name"));
                assert!(state.is_some());
                assert_eq!(attributes.size, 128);
                assert_eq!(attributes.walk_speed, 200.0);
                // Новорождённый: статы ещё не успели просесть, стадия — яйцо.
                assert!(stats.satiety > 99.0 && stats.health > 99.0);
                assert_eq!(stage, Stage::Egg);
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
        assert!(!app.quit_via_ipc.load(Ordering::Relaxed));

        let reply = send(&tx, Request::Quit);
        app.tick(0.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(app.wants_exit());
        // run() по этому флагу дожидается IPC-поток (ответ клиенту).
        assert!(app.quit_via_ipc.load(Ordering::Relaxed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ТД-20: сигнал завершения превращается в graceful-выход. Сохранять
    /// нечего: журнал (единственная персистентность) уже на диске.
    #[test]
    fn sigterm_flag_exits_gracefully() {
        let (mut app, _tx, dir) = app("sigterm");
        geometry(&mut app);
        assert!(!app.wants_exit());

        app.sig_exit.store(true, Ordering::Relaxed);
        app.tick(0.1);
        assert!(app.wants_exit());
        assert!(
            !journal_kinds(&dir).is_empty(),
            "журнал на диске (записан ещё при старте)"
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

    // --- B3: меню ПКМ -----------------------------------------------------

    /// Порядок действий совпадает со строками меню — контракт диспатча.
    #[test]
    fn menu_rows_match_actions_order() {
        assert_eq!(menu_rows().len(), MENU_ACTIONS.len());
        assert_eq!(
            MENU_ACTIONS,
            [
                MenuAction::Feed,
                MenuAction::Treat,
                MenuAction::Play,
                MenuAction::Sleep,
                MenuAction::Settings,
                MenuAction::Dismiss,
            ]
        );
    }

    /// Чистая математика меню: индекс строки под точкой и прижатие к экрану.
    #[test]
    fn hover_and_clamp_math() {
        let rects = [
            Rect::new(4.0, 4.0, 100.0, 20.0),
            Rect::new(4.0, 24.0, 100.0, 20.0),
        ];
        assert_eq!(hover_index(&rects, Vec2::new(10.0, 10.0)), Some(0));
        assert_eq!(hover_index(&rects, Vec2::new(10.0, 30.0)), Some(1));
        assert_eq!(hover_index(&rects, Vec2::new(-1.0, 10.0)), None);
        assert_eq!(hover_index(&rects, Vec2::new(10.0, 60.0)), None);

        let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        // Правый нижний угол: меню прижимается внутрь экрана.
        assert_eq!(
            clamp_menu_origin(Vec2::new(1900.0, 1070.0), (200.0, 150.0), &screen),
            Vec2::new(1720.0, 930.0)
        );
        // Левый верхний: не уезжает в минус.
        assert_eq!(
            clamp_menu_origin(Vec2::new(-5.0, -5.0), (200.0, 150.0), &screen),
            Vec2::new(0.0, 0.0)
        );
    }

    /// ПКМ открывает меню (вторая хит-область, спрайт поверх, Active-темп
    /// даже во сне), повторный ПКМ закрывает.
    #[test]
    fn rmb_toggles_menu_with_hit_area_and_active_pace() {
        let (mut app, _tx, dir) = app("menu-toggle");
        geometry(&mut app);
        app.pet.as_mut().unwrap().state = PetState::Sleep;
        assert_eq!(app.pace(), Pace::Drowsy);

        let p = app.pet.as_ref().unwrap().pos;
        assert!(app.event(Event::PointerMenu(p), 0.0));
        assert!(app.menu.is_some());
        assert_eq!(app.pace(), Pace::Active, "открытое меню держит Active");
        let scene = app.tick(0.1);
        assert_eq!(scene.sprites.len(), 2, "питомец + меню");
        assert_eq!(scene.input_rects.len(), 2, "хит-область меню добавлена");

        assert!(app.event(Event::PointerMenu(p), 0.2));
        assert!(app.menu.is_none(), "повторный ПКМ закрывает меню");
        assert_eq!(app.tick(0.3).sprites.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Во время drag меню не открывается (Release не должен потеряться).
    #[test]
    fn menu_does_not_open_while_dragged() {
        let (mut app, _tx, dir) = app("menu-drag");
        geometry(&mut app);
        let mut now = 0.0;
        settle(&mut app, &mut now, 2.0);
        let pos = app.pet.as_ref().unwrap().pos;
        let grab = Vec2::new(pos.x, pos.y - 10.0);
        assert!(app.event(Event::PointerPress(grab), now));
        assert!(app.event(Event::PointerMotion(Vec2::new(500.0, 300.0)), now + 0.1));
        assert_eq!(app.pet.as_ref().unwrap().state, PetState::Dragged);
        assert!(app.event(Event::PointerMenu(Vec2::new(500.0, 300.0)), now + 0.2));
        assert!(app.menu.is_none(), "во время drag меню не открывается");
        // Release доходит до питомца — drag завершается броском.
        assert!(app.event(Event::PointerRelease(Vec2::new(500.0, 300.0)), now + 0.3));
        assert_ne!(app.pet.as_ref().unwrap().state, PetState::Dragged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Меню прижимается к краям экрана целиком.
    #[test]
    fn menu_is_clamped_to_screen() {
        let (mut app, _tx, dir) = app("menu-clamp");
        geometry(&mut app);
        // ПКМ у самого правого нижнего угла.
        app.event(Event::PointerMenu(Vec2::new(1919.0, 1079.0)), 0.0);
        let rect = app.menu.as_ref().unwrap().screen_rect();
        assert!(rect.right() <= 1920.0 && rect.bottom() <= 1080.0);
        assert!(rect.x >= 0.0 && rect.y >= 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Движение курсора подсвечивает строку под ним (кадр перепекается
    /// только при смене строки), мимо строк — подсветка снимается.
    #[test]
    fn menu_hover_highlights_row_under_cursor() {
        let (mut app, _tx, dir) = app("menu-hover");
        geometry(&mut app);
        let p = app.pet.as_ref().unwrap().pos;
        app.event(Event::PointerMenu(p), 0.0);
        let before = app.menu.as_ref().unwrap().frame.argb.clone();

        let target = row_center(&app, 1);
        assert!(app.event(Event::PointerMotion(target), 0.1));
        let menu = app.menu.as_ref().unwrap();
        assert_eq!(menu.hovered, Some(1));
        assert_ne!(menu.frame.argb, before, "кадр перепечён с подсветкой");

        // Тот же ховер — кадр не трогаем (дешёвый путь).
        let ptr_before = app.menu.as_ref().unwrap().frame.argb.as_ptr();
        assert!(app.event(Event::PointerMotion(target), 0.15));
        assert!(core::ptr::eq(
            ptr_before,
            app.menu.as_ref().unwrap().frame.argb.as_ptr()
        ));

        let origin = app.menu.as_ref().unwrap().origin;
        assert!(app.event(
            Event::PointerMotion(Vec2::new(origin.x - 5.0, origin.y - 5.0)),
            0.2
        ));
        assert_eq!(app.menu.as_ref().unwrap().hovered, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Клик по строке зовёт те же обработчики, что IPC, и закрывает меню.
    #[test]
    fn menu_row_press_dispatches_same_as_ipc() {
        let (mut app, _tx, dir) = app("menu-press");
        geometry(&mut app);
        let p = app.pet.as_ref().unwrap().pos;

        // «Покормить».
        app.event(Event::PointerMenu(p), 0.0);
        let target = row_center(&app, 0);
        assert!(app.event(Event::PointerPress(target), 0.1));
        assert!(app.menu.is_none(), "выбор строки закрывает меню");
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Fed { treat: false }
        ));

        // «Вкусняшка».
        app.event(Event::PointerMenu(p), 0.2);
        let target = row_center(&app, 1);
        app.event(Event::PointerPress(target), 0.3);
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Fed { treat: true }
        ));

        // «Поиграть».
        app.event(Event::PointerMenu(p), 0.4);
        let target = row_center(&app, 2);
        app.event(Event::PointerPress(target), 0.5);
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Played
        ));

        // «Уложить спать» — питомец засыпает прямо сейчас.
        let mut now = 0.6;
        settle(&mut app, &mut now, 2.0);
        app.event(Event::PointerMenu(p), now);
        let target = row_center(&app, 3);
        app.event(Event::PointerPress(target), now);
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::PutToSleep
        ));
        assert_eq!(app.pet.as_ref().unwrap().state, PetState::Sleep);

        // «Убрать с экрана».
        app.event(Event::PointerMenu(p), now + 0.1);
        let target = row_center(&app, 5);
        app.event(Event::PointerPress(target), now + 0.2);
        assert!(app.pet.is_none(), "Dismiss из меню убирает питомца");
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Dismissed
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Клик мимо строк закрывает меню без события журнала и без drag.
    #[test]
    fn press_outside_menu_closes_without_action() {
        let (mut app, _tx, dir) = app("menu-outside");
        geometry(&mut app);
        let p = app.pet.as_ref().unwrap().pos;
        app.event(Event::PointerMenu(p), 0.0);
        let events_before = journal_kinds(&dir).len();

        assert!(app.event(Event::PointerPress(Vec2::new(5.0, 5.0)), 0.1));
        assert!(app.menu.is_none(), "клик мимо закрывает меню");
        assert_eq!(journal_kinds(&dir).len(), events_before, "без события");
        // Закрывающий клик не начал drag: питомец не следует за курсором.
        assert!(app.event(Event::PointerMotion(Vec2::new(400.0, 400.0)), 0.2));
        assert_ne!(app.pet.as_ref().unwrap().state, PetState::Dragged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- B5: видимый уход --------------------------------------------------

    /// Кормление показывает оверлей еды, который истекает сам.
    #[test]
    fn feed_shows_eating_overlay_until_expiry() {
        let (mut app, tx, dir) = adult_app("feedlook");
        geometry(&mut app);
        let reply = send(&tx, Request::Feed { treat: false });
        app.tick(1.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(matches!(
            app.overlay,
            Some(Overlay {
                look: ActionLook::Eating,
                ..
            })
        ));
        assert_eq!(app.pace(), Pace::Active, "оверлей анимируется в Active");
        app.tick(1.0 + EATING_SECS + 0.1);
        assert!(app.overlay.is_none(), "оверлей еды истёк");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Игра включает «радостное» окно, которое истекает само.
    #[test]
    fn play_sets_happy_window() {
        let (mut app, tx, dir) = adult_app("playlook");
        geometry(&mut app);
        let reply = send(&tx, Request::Play);
        app.tick(1.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(app.happy_until.is_some());
        app.tick(1.0 + HAPPY_PLAY_SECS + 0.1);
        assert!(app.happy_until.is_none(), "радость истекла");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// У яйца видимых реакций ухода нет: событие пишется, оверлей — нет.
    #[test]
    fn egg_has_no_visible_care_reactions() {
        let (mut app, tx, dir) = app("carelook-egg");
        geometry(&mut app);
        let reply = send(&tx, Request::Feed { treat: false });
        app.tick(1.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(app.overlay.is_none(), "у яйца нет оверлея еды");
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Fed { treat: false }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Клик по питомцу — поглаживание: событие Petted + краткая радость.
    #[test]
    fn click_pets_the_pet_with_happy_look() {
        let (mut app, _tx, dir) = adult_app("petting");
        geometry(&mut app);
        let mut now = 0.0;
        settle(&mut app, &mut now, 3.5);
        let pos = app.pet.as_ref().unwrap().pos;
        let point = Vec2::new(pos.x, pos.y - 10.0);
        assert!(app.event(Event::PointerPress(point), now));
        assert!(app.event(Event::PointerRelease(point), now + 0.05));
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Petted
        ));
        assert!(app.happy_until.is_some(), "радость от поглаживания");
        app.tick(now + HAPPY_PET_SECS + 0.2);
        assert!(app.happy_until.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// «Уложить спать» укладывает сразу; настоящая длительность сна после
    /// пробуждения записана событием Slept.
    #[test]
    fn put_to_sleep_forces_sleep_and_logs_slept() {
        let (mut app, tx, dir) = adult_app("sleepcare");
        geometry(&mut app);
        let mut now = 0.0;
        settle(&mut app, &mut now, 2.5);
        let reply = send(&tx, Request::PutToSleep);
        now += 0.05;
        app.tick(now);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert_eq!(app.pet.as_ref().unwrap().state, PetState::Sleep);

        // Сон длится sleep_range.1 (30 с дефолта) — тикаем посекундно,
        // как настоящий Drowsy-темп.
        for _ in 0..40 {
            now += 1.0;
            app.tick(now);
        }
        let minutes = journal_kinds(&dir)
            .iter()
            .find_map(|k| match k {
                EventKind::Slept { minutes } => Some(*minutes),
                _ => None,
            })
            .expect("Slept записан после пробуждения");
        assert!(
            (SLEPT_MIN_MINUTES..0.8).contains(&minutes),
            "минуты сна: {minutes}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Прерванный почти сразу сон (dismiss) — шум: Slept не пишется,
    /// но период закрывается корректно.
    #[test]
    fn short_sleep_is_not_logged_on_dismiss() {
        let (mut app, tx, dir) = adult_app("shortsleep");
        geometry(&mut app);
        let mut now = 0.0;
        settle(&mut app, &mut now, 2.5);
        let reply = send(&tx, Request::PutToSleep);
        now += 0.05;
        app.tick(now);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert_eq!(app.pet.as_ref().unwrap().state, PetState::Sleep);

        let reply = send(&tx, Request::Dismiss);
        now += 2.0;
        app.tick(now);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        let kinds = journal_kinds(&dir);
        assert!(
            !kinds.iter().any(|k| matches!(k, EventKind::Slept { .. })),
            "сон короче порога не пишется"
        );
        assert!(matches!(kinds.last().unwrap(), EventKind::Dismissed));
        assert!(app.sleep_since.is_none(), "период сна закрыт");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- B6: яйцо, вылупление, приветствие ---------------------------------

    /// Яйцо никогда не ходит: за 100 секунд симуляции ни одного Walk.
    #[test]
    fn egg_pet_never_walks() {
        let (mut app, _tx, dir) = app("eggwalk");
        geometry(&mut app);
        let mut now = 0.0;
        for _ in 0..3000 {
            now += 1.0 / 30.0;
            app.tick(now);
            assert_ne!(
                app.pet.as_ref().unwrap().state,
                PetState::Walk,
                "яйцо не ходит"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Переход Egg -> дальше: анимация вылупления, затем пузырь «Привет!»,
    /// спрайты пересозданы под новую стадию, яйцу разрешают ходить.
    #[test]
    fn hatching_plays_overlay_then_hello_bubble() {
        let (mut app, _tx, dir) = app("hatch");
        geometry(&mut app);
        let egg_size = app.sprites.size;
        // Дебаг-ускорение прямо в конфиге свёртки (env в тестах не трогаем,
        // ТД-26); гейт яйца пересекается за миллисекунды настенных часов.
        app.fold_cfg.growth_scale = 1e9;
        std::thread::sleep(std::time::Duration::from_millis(3));

        let sprites_during_hatch = app.tick(1.0).sprites.len();
        assert_eq!(sprites_during_hatch, 1, "пузырь ждёт конца вылупления");
        assert!(app.derived.stage > Stage::Egg, "гейт яйца пройден");
        assert!(matches!(
            app.overlay,
            Some(Overlay {
                look: ActionLook::Hatching,
                ..
            })
        ));
        assert_eq!(app.pace(), Pace::Active);
        assert_eq!(app.sprite_stage, app.derived.stage);
        assert!(app.sprites.size > egg_size, "набор больше яичного");

        // Вылупление кончилось — показан пузырь без хит-области.
        let counts = {
            let scene = app.tick(1.0 + HATCH_SECS + 0.1);
            (scene.sprites.len(), scene.input_rects.len())
        };
        assert!(app.overlay.is_none());
        assert_eq!(counts, (2, 1), "питомец + «Привет!» без хит-области");

        // Пузырь истёк — сцена снова обычная.
        let sprites_after = app
            .tick(1.0 + HATCH_SECS + HELLO_HATCH_SECS + 0.2)
            .sprites
            .len();
        assert_eq!(sprites_after, 1);
        assert!(app.bubble.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Старт демона с уже вылупившимся призванным питомцем — приветствие.
    #[test]
    fn start_greets_existing_pet_with_bubble() {
        let (mut app, _tx, dir) = adult_app("greet");
        geometry(&mut app);
        let scene = app.tick(0.0);
        assert_eq!(scene.sprites.len(), 2, "питомец + приветствие");
        assert_eq!(scene.input_rects.len(), 1);
        assert_eq!(app.pace(), Pace::Active);
        let scene = app.tick(HELLO_START_SECS + 0.1);
        assert_eq!(scene.sprites.len(), 1, "пузырь истёк");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Чистая арифметика сна: порог шума и защита от времени назад.
    #[test]
    fn slept_minutes_threshold() {
        assert_eq!(slept_minutes(0.0, 29.0), None, "короткий сон — шум");
        let m = slept_minutes(0.0, 60.0).unwrap();
        assert!((m - 1.0).abs() < 1e-6);
        assert_eq!(slept_minutes(10.0, 5.0), None, "время назад — не сон");
    }
}
