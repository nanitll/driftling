//! Демон: симуляция + платформа + IPC.
//!
//! Связывает:
//! - `driftling_core::journal` — журнал событий, единственный источник
//!   истины о питомце (имя, характеристики, статы, стадия, summoned);
//! - `driftling_core::Pet` (поведение на экране, tick + pointer);
//! - `driftling_core::pack` (кадры арт-пака; процедурный sprite.rs — фолбэк);
//! - `driftling_platform::run_auto` (оверлей Wayland/X11: App::tick -> Scene);
//! - `driftling_ipc::Server` в отдельном потоке -> канал -> DaemonApp::tick.
//!
//! Контракт App::tick: собрать Scene из текущего кадра питомца
//! (`SpriteSet::frame_look(...)`, origin = bounds().{x,y}, ориентация —
//! `Pet::orient()`: зеркало по взгляду плюс поворот под поверхность, на
//! которой питомец сидит) и input_rects = [bounds()], когда он призван.
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
//!   свёрнутой стадии или размера ([`stage_sprites`]: арт-пак фазы C с
//!   фолбэком на процедурный блоб); в памяти живёт ТОЛЬКО набор текущей
//!   стадии — старый дропается при смене (бюджет RSS); кадр
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
//!
//! Фаза D, волна 1 — «окна-рельеф» (D1/D2 + демонная половина D4/D5):
//! - **worldsense**: провайдер (`driftling_worldsense::detect`) создаётся в
//!   [`run`] и отдаётся демону параметром — тесты DaemonApp подсовывают свой
//!   или ничего (реальный провайдер грузил бы скрипт в живой KWin, ТД-26);
//! - **опрос**: `latest()` — чтение мьютекса, зовём каждый тик при активном
//!   питомце и при скрытии (fullscreen), иначе раз в [`SENSE_POLL_IDLE`];
//! - **координаты**: KWin отдаёт окна в глобальном пространстве, мир питомца
//!   локален выходу — переводим через `origin` из [`Event::OutputGeometry`];
//! - **физика**: изменившийся снапшот кладётся в `World.platforms` (+ пол из
//!   workArea) и объявляется питомцу через [`Pet::world_changed`];
//! - **вежливость (D5)**: `fullscreen_active` прячет сцену (пустые спрайты и
//!   input region), симуляция продолжает тикать «за кадром»; уход из
//!   fullscreen возвращает питомца на место.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use driftling_core::palette;
use driftling_core::physics::{self, Platform};
use driftling_core::sprite::{self, mood_tier, ActionLook, Frame, Look, MoodTier, SpriteSet};
use driftling_core::{
    apply_remote, cursors_of, device_journal_path_in, fold, growth, text, Config, DerivedPet,
    Direction, Event as JournalEvent, EventKind, FoldCfg, HlcClock, Journal, Orient, Pet,
    PetAttributes, PetRecord, PetState, PhysicsConfig, PointerEvent, Rect, SimPace, Stage, Surface,
    SyncConfig, SyncMode, Vec2, World,
};
use driftling_ipc::{Request, Response, Server};
use driftling_platform::{App, Event, Pace, Scene, SpriteInstance};
use driftling_worldsense::WorldSense;

use crate::i18n::fl;
use crate::sync::{self, SyncCmd, SyncHandle, SyncNote};

/// Сколько IPC-поток ждёт ответа от цикла приложения, прежде чем сдаться.
const IPC_REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Как часто пересворачивать журнал без новых событий: декей статов в
/// Status/PetInfo между командами. Сам fold дешёвый (файлы малы), но
/// гонять его каждый кадр незачем.
const REFOLD_INTERVAL: Duration = Duration::from_secs(60);

/// Кегль строк меню ПКМ, px (язык дизайна настроек, text::menu_frame).
const MENU_PX: f32 = 15.0;
/// Акцент подсветки меню: цвет питомца осветляется — на тёмной карточке
/// сам тёмный цвет тела был бы не виден.
const MENU_ACCENT_LIGHTEN: f32 = 1.25;
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
/// Опрос worldsense при спокойном питомце (фаза D): снапшоты пушатся
/// провайдером сами, `latest()` — только чтение мьютекса, но и его незачем
/// дёргать чаще при Calm/Drowsy. Активный питомец и скрытый (fullscreen)
/// опрашиваются каждый тик.
const SENSE_POLL_IDLE: Duration = Duration::from_secs(2);
/// Режим «папки» (фаза E): период проверки mtime чужих journal.*.jsonl.
const FOLDER_POLL: Duration = Duration::from_secs(10);
/// Скорость пробежки присутствия («убежал/прибежал», фаза E), лог. px/с —
/// заметно быстрее любой прогулки: питомец именно УБЕГАЕТ за край.
const PRESENCE_RUN_SPEED: f32 = 420.0;
/// Куда прибегает питомец при run-in: доля ширины экрана от края входа.
const PRESENCE_RUN_IN_DEPTH: f32 = 0.3;
/// Троттлинг lease-claim от пользовательских взаимодействий.
const CLAIM_THROTTLE: Duration = Duration::from_secs(2);

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

    // Повторный запуск при живом демоне (клик по иконке в меню приложений) —
    // не ошибка, а «позови питомца»: призываем в работающем демоне и тихо
    // выходим. Bind идёт ДО запуска трея, чтобы второй процесс не успел
    // зарегистрировать дублирующую SNI-иконку.
    let server = match Server::bind() {
        Ok(server) => server,
        Err(bind_err) => {
            return match driftling_ipc::call(&Request::Summon) {
                Ok(_) => {
                    log::info!("демон уже работает — питомец призван, второй экземпляр выходит");
                    Ok(())
                }
                // Демон не отвечает — значит, дело не во втором экземпляре:
                // отдаём исходную ошибку bind.
                Err(_) => Err(bind_err),
            };
        }
    };

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
    // Синк (фаза E): секция [sync] config.toml; битый конфиг не роняет
    // демона — просто работаем без синка (и говорим об этом).
    let (sync_cfg, physics_cfg) = match Config::load() {
        Ok(cfg) => (cfg.sync, cfg.physics),
        Err(e) => {
            log::warn!("config.toml не прочитан ({e}) — синк выключен");
            (
                driftling_core::SyncConfig::default(),
                PhysicsConfig::default(),
            )
        }
    };
    // Восприятие мира (фаза D): KWin-провайдер на KDE, null-провайдер
    // (пол = низ экрана) везде ещё; деградация штатная, демон работает всегда.
    let sense = driftling_worldsense::detect();
    let mut app = DaemonApp::new(
        rx,
        data_dir,
        legacy_attrs,
        growth_scale,
        Some(sense),
        sync_cfg,
    );
    app.physics = physics_cfg;
    // Разовая нормализация характеристик из эпохи ручного config.toml
    // (фаза G): 400 px/s и непоседливость 100 — это не характер, а баг.
    app.tame_attributes();

    // SIGTERM/SIGINT (ТД-20): флаг проверяется в tick -> graceful-выход.
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&app.sig_exit))?;
    }
    install_panic_hook();

    let quit_via_ipc = Arc::clone(&app.quit_via_ipc);
    let result = driftling_platform::run_auto(app);
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
    /// Каталог журналов: `data_dir`, либо `sync.folder` (режим «папки»).
    journal_dir: PathBuf,
}

/// Каталог журналов по конфигу синка (фаза E): режим «папки» с указанным
/// путём уводит журналы в синкаемый каталог; pet.json (device_id!) и
/// sync-state.json ВСЕГДА остаются локальными в `data_dir`.
fn resolve_journal_dir(data_dir: &Path, sync: &SyncConfig) -> PathBuf {
    let folder = sync.folder.trim();
    if sync.mode == SyncMode::Folder && !folder.is_empty() {
        PathBuf::from(folder)
    } else {
        data_dir.to_path_buf()
    }
}

/// Подписи ЧУЖИХ журнальных файлов каталога (режим «папки»): имя ->
/// (длина, mtime). Свой файл исключён — его меняем мы сами; легаси
/// `journal.jsonl` считается чужим (мог принести синкер).
fn folder_signature(dir: &Path, own_device: &str) -> BTreeMap<String, (u64, Option<SystemTime>)> {
    let own_name = format!("journal.{own_device}.jsonl");
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_journal = name == "journal.jsonl"
            || (name.len() > "journal..jsonl".len()
                && name.starts_with("journal.")
                && name.ends_with(".jsonl"));
        if !is_journal || name == own_name {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        out.insert(name.to_string(), (meta.len(), meta.modified().ok()));
    }
    out
}

/// Привести файл СВОЕГО устройства в целевой каталог журналов: среди
/// кандидатов (локальный каталог данных и настроенная папка) выбирается
/// самая длинная копия `journal.<device>.jsonl` — файл append-only и
/// single-writer, поэтому все копии — префиксы истинного лога, и «длиннее»
/// = «полнее». Так переживаются включение папочного режима (файл уезжает
/// в папку), его выключение (возврат в data_dir) и смена самой папки.
fn adopt_own_journal(candidates: &[&Path], journal_dir: &Path, device: &str) {
    let target = device_journal_path_in(journal_dir, device);
    let len_of = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let target_len = len_of(&target);
    let best = candidates
        .iter()
        .filter(|dir| ***dir != *journal_dir)
        .map(|dir| device_journal_path_in(dir, device))
        .filter(|p| p.is_file())
        .max_by_key(|p| len_of(p))
        .filter(|p| len_of(p) > target_len);
    let Some(best) = best else { return };
    if let Err(e) = std::fs::create_dir_all(journal_dir) {
        log::warn!("каталог журналов {} не создан: {e}", journal_dir.display());
        return;
    }
    match std::fs::copy(&best, &target) {
        Ok(_) => log::info!(
            "синк: журнал устройства перенесён {} -> {}",
            best.display(),
            target.display()
        ),
        Err(e) => log::warn!("синк: журнал устройства не скопирован ({e})"),
    }
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
///
/// Фаза E: `sync` определяет КАТАЛОГ журналов (режим «папки» уводит их в
/// синкаемую папку); pet.json и sync-state.json всегда в `data_dir`, файл
/// своего устройства при смене каталога догоняется [`adopt_own_journal`].
fn load_storage(
    data_dir: &Path,
    sync: &SyncConfig,
    legacy_attrs: Option<PetAttributes>,
) -> PetStorage {
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

    let journal_dir = resolve_journal_dir(data_dir, sync);
    if journal_dir != data_dir {
        log::info!("синк: журналы в папке {}", journal_dir.display());
        // Легаси-файл data_dir (до фазы E) сперва уводится в файл своего
        // устройства — open_dir целевого каталога его бы не увидел.
        if driftling_core::journal_path_in(data_dir).exists() {
            if let Err(e) = Journal::open_dir(data_dir, &record.device_id) {
                log::warn!("миграция легаси-журнала: {e}");
            }
        }
    }
    // Свой файл догоняет смену каталога (вкл/выкл/смена папки): побеждает
    // самая длинная копия — префикс-свойство append-only делает это точным.
    let folder = PathBuf::from(sync.folder.trim());
    let mut candidates: Vec<&Path> = vec![data_dir];
    if !sync.folder.trim().is_empty() {
        candidates.push(&folder);
    }
    adopt_own_journal(&candidates, &journal_dir, &record.device_id);

    // E-core: журнал пер-девайсный (ТЗ §3.5) — open_dir мигрирует
    // одиночный journal.jsonl в journal.<device_id>.jsonl и сливает
    // файлы всех устройств (чужие — read-only входы).
    let (mut events, warnings) = match Journal::open_dir(&journal_dir, &record.device_id) {
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
            // E-core: запись только в файл своего устройства.
            if let Err(e) = Journal::append(&journal_dir, clock.device(), &ev) {
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
        journal_dir,
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
    /// Акцент подсветки строк: осветлённый цвет питомца на момент открытия.
    accent: u32,
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
        let (frame, rects) = text::menu_frame(&rows, self.hovered, MENU_PX, self.accent);
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

/// Множитель длительности сна по энергии: бодрый дремлет как обычно,
/// вымотанный (0) спит вшестеро дольше. Линейно между.
fn sleep_scale_for(energy: f32) -> f32 {
    1.0 + 5.0 * (1.0 - energy.clamp(0.0, 100.0) / 100.0)
}

/// Пересечение прямоугольников (пустое — нулевой размер).
fn intersect(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let w = (a.right().min(b.right()) - x).max(0.0);
    let h = (a.bottom().min(b.bottom()) - y).max(0.0);
    Rect::new(x, y, w, h)
}

/// Сдвиг рисунка к поверхности на прозрачное поле кадра (фаза G2):
/// лапки должны касаться стены и потолка, а не висеть в пикселе от них.
///
/// К стене питомец повёрнут ПЕРЕДОМ (профильные кадры лазания нарисованы
/// мордой вправо, к левой стене зеркалятся) — берём переднее поле кадра.
/// К потолку он обращён ногами — там нижнее поле кадров ходьбы.
fn grip_shift(bounds: Rect, surface: Surface, sprites: &SpriteSet) -> Rect {
    let front = sprites.grip_inset.right as f32;
    let feet = sprites.feet_inset.bottom as f32;
    let (dx, dy) = match surface {
        Surface::Floor => (0.0, 0.0),
        Surface::WallLeft => (-front, 0.0),
        Surface::WallRight => (front, 0.0),
        Surface::Ceiling => (0.0, -feet),
    };
    Rect::new(bounds.x + dx, bounds.y + dy, bounds.w, bounds.h)
}

/// Набор кадров стадии из встроенного арт-пака (фаза C), колоризованный
/// цветом питомца. Целевой размер — базовый размер (attributes.size),
/// отмасштабированный [`sprite::stage_scale`]: та же лестница роста, что
/// была у процедурного блоба; пак сам берёт ближайший целый множитель
/// native (пиксель-арт не мылится). Битый пак — не смерть: фолбэк на
/// процедурный спрайт с логом (внешние .driftpack придут в M4, у
/// встроенного пака валидность гарантируют тесты core).
///
/// Возвращается ОДИН набор — вызывающий хранит только текущую стадию,
/// присваивание дропает старый набор (бюджет RSS фазы B).
fn stage_sprites(base: u32, stage: Stage, color: u32) -> SpriteSet {
    let target = ((base as f32) * sprite::stage_scale(stage)).round() as u32;
    match driftling_core::pack::default_sprite_set(stage, target, color) {
        Ok(set) => set,
        Err(e) => {
            log::warn!("арт-пак не загрузился ({e}) — процедурный фолбэк");
            sprite::placeholder_colored(base, stage, color)
        }
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

/// Сценарная пробежка присутствия (фаза E, фирменный UX ТЗ §3.5): демон
/// ведёт питомца за руку мимо машины поведения — быстрый бег к краю
/// («убежал на другое устройство») или от края («прибежал»). Пока анимация
/// активна, обычный `Pet::tick` не зовётся и хит-области нет (убегающего
/// не поймать).
#[derive(Debug, Clone, Copy, PartialEq)]
enum PresenceAnim {
    /// Бег к краю: dir = -1.0 (влево) | +1.0 (вправо); за краем питомец
    /// снимается БЕЗ события журнала (присутствие — не уход).
    RunOff { dir: f32 },
    /// Бег от края внутрь до `target_x`, дальше — обычное поведение.
    RunIn { dir: f32, target_x: f32 },
}

/// Подписи чужих журнальных файлов для горячей перечитки режима «папки»:
/// имя -> (длина, mtime). Смена подписи = синкер что-то принёс.
type FolderSeen = BTreeMap<String, (u64, Option<SystemTime>)>;

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
    /// Цвет тела, под который сгенерирован набор, — детектор перекраски
    /// (Recolored в журнале меняет derived.color, спрайты догоняют).
    sprite_color: u32,
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
    /// Восприятие мира (фаза D): None — тестовый демон без провайдера.
    sense: Option<Box<dyn WorldSense>>,
    /// Глобальная логическая позиция выхода питомца (из OutputGeometry):
    /// снапшоты worldsense приходят в глобальных координатах композитора.
    screen_origin: Vec2,
    /// Полный прямоугольник выхода в локальных координатах (из
    /// OutputGeometry). `World::screen` может быть уже — это рабочая
    /// область без панелей (фаза G2), а сюда возвращаемся, когда данных
    /// о ней нет.
    output_rect: Rect,
    /// Последний опрос sense.latest() — троттлинг при спокойном питомце.
    last_sense_poll: Option<Instant>,
    /// Интервал спокойного опроса; в тестах ужимается до нуля.
    sense_poll_idle: Duration,
    /// Вежливость (D5): на экране полноэкранное окно — сцена прячется
    /// (пустые спрайты и input region), симуляция тикает дальше.
    fullscreen_hidden: bool,
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
    /// Каталог данных (pet.json, sync-state.json) — DI вместо env (ТД-26).
    data_dir: PathBuf,
    /// Каталог журналов: data_dir либо sync.folder (режим «папки», фаза E).
    journal_dir: PathBuf,
    /// Конфиг синка (фаза E); перечитывается `ctl reload`.
    sync_cfg: SyncConfig,
    /// Физика мира (фаза G3): рост питомца «в жизни» задаёт масштаб, из
    /// которого выводятся настоящие 9.81 м/с² и вес. Перечитывается
    /// `ctl reload`.
    physics: PhysicsConfig,
    /// Воркер синка (mode = server); None — off/folder. Дроп ручки
    /// завершает поток воркера.
    sync: Option<SyncHandle>,
    /// Питомец скрыт lease-ом («перебежал» на другое устройство). Это НЕ
    /// dismiss: события журнала нет, derived.summoned остаётся true.
    lease_hidden: bool,
    /// Активная пробежка присутствия (run-off/run-in).
    presence_anim: Option<PresenceAnim>,
    /// Троттлинг lease-claim от пользовательских взаимодействий.
    last_claim: Option<Instant>,
    /// Режим «папки»: последняя проверка чужих файлов и их подписи.
    folder_poll_at: Option<Instant>,
    folder_seen: FolderSeen,
    /// Интервал опроса папки; тесты ужимают до нуля (ТД-26).
    folder_poll: Duration,
    /// Последнее успешное вливание чужих файлов (статус синка папки).
    folder_merged_at: Option<Instant>,
    /// Взводится обработчиком SIGTERM/SIGINT; tick превращает в exit.
    sig_exit: Arc<AtomicBool>,
}

impl DaemonApp {
    /// Поднять демона из каталога данных: журнал -> свёртка -> спрайты
    /// под текущие характеристики и стадию. `legacy_attrs` — миграция
    /// старых секций config.toml, применяется только при рождении питомца;
    /// `growth_scale` — дебаг-ускорение роста (DRIFTLING_GROWTH_SCALE);
    /// `sense` — провайдер worldsense (фаза D), создаётся в run(): реальный
    /// провайдер грузит скрипт в живой KWin, тестам такое нельзя (ТД-26);
    /// `sync_cfg` — секция [sync] config.toml (фаза E), тесты дают дефолт
    /// (off) — сеть и потоки воркера им не нужны.
    fn new(
        rx: Receiver<IpcMessage>,
        data_dir: PathBuf,
        legacy_attrs: Option<PetAttributes>,
        growth_scale: Option<f64>,
        sense: Option<Box<dyn WorldSense>>,
        sync_cfg: SyncConfig,
    ) -> Self {
        for w in sync_cfg.warnings() {
            log::warn!("конфиг синка: {w}");
        }
        let storage = load_storage(&data_dir, &sync_cfg, legacy_attrs);
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
        // Воркер синка (фаза E, режим «свой сервер»): push/pull + lease.
        let sync = (sync_cfg.mode == SyncMode::Server).then(|| {
            sync::spawn(
                &sync_cfg,
                &data_dir,
                &storage.journal_dir,
                storage.clock.device(),
                derived.summoned,
                sync::Tuning::default(),
            )
        });
        // Режим «папки»: стартовая подпись чужих файлов, чтобы первый
        // опрос не перечитывал каталог зря.
        let folder_seen = if sync_cfg.mode == SyncMode::Folder {
            folder_signature(&storage.journal_dir, storage.clock.device())
        } else {
            FolderSeen::default()
        };
        let size = derived.attributes.clamped().size;
        let stage = derived.stage;
        let color = derived.color;
        Self {
            sprites: stage_sprites(size, stage, color),
            events: storage.events,
            clock: storage.clock,
            fold_cfg,
            derived,
            last_fold: Instant::now(),
            journal_writable: storage.writable,
            sprite_base: size,
            sprite_stage: stage,
            sprite_color: color,
            overlay: None,
            bubble: None,
            happy_until: None,
            menu: None,
            sleep_since: None,
            pet: None,
            world: None,
            sense,
            screen_origin: Vec2::default(),
            output_rect: Rect::default(),
            last_sense_poll: None,
            sense_poll_idle: SENSE_POLL_IDLE,
            fullscreen_hidden: false,
            exit: false,
            started: Instant::now(),
            last_now: None,
            quit_via_ipc: Arc::new(AtomicBool::new(false)),
            rx,
            data_dir,
            journal_dir: storage.journal_dir,
            sync_cfg,
            sync,
            lease_hidden: false,
            presence_anim: None,
            last_claim: None,
            folder_poll_at: None,
            folder_seen,
            physics: PhysicsConfig::default(),
            folder_poll: FOLDER_POLL,
            folder_merged_at: None,
            sig_exit: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Пересвернуть журнал в текущее состояние (декей до «сейчас»).
    fn refold(&mut self) {
        self.derived = fold(&self.events, wall_now_ms(), &self.fold_cfg);
        self.last_fold = Instant::now();
        // Усталость -> длина сна (фаза G3): при нулевой энергии питомец
        // спит вшестеро дольше обычной дрёмы и просыпается заряженным.
        if let Some(pet) = &mut self.pet {
            pet.set_sleep_scale(sleep_scale_for(self.derived.stats.energy));
        }
    }

    /// Разовое «успокоение» характеристик (фаза G).
    ///
    /// Питомцы, приехавшие из ручного `config.toml` эпохи M0, несут в
    /// журнале экстремальные значения (скорость 400 px/s, непоседливость
    /// 100 при нулевой сонливости): такой питомец без остановки носится
    /// по экрану. Нормализация — обычное журнальное событие
    /// `AttributesSet`: она разъедется на все устройства и откатывается
    /// дебаг-панелью, а не переписывает историю задним числом.
    /// Питомцы в норме (и уже нормализованные) не трогаются.
    fn tame_attributes(&mut self) {
        let Some(tamed) = self.derived.attributes.tamed() else {
            return;
        };
        let before = self.derived.attributes;
        match self.append_event(EventKind::AttributesSet { attributes: tamed }) {
            Ok(()) => log::info!(
                "характер успокоен (фаза G): скорость {:.0}->{:.0}, непоседливость {}->{}, \
                 сонливость {}->{}",
                before.walk_speed,
                tamed.walk_speed,
                before.curiosity,
                tamed.curiosity,
                before.sleepiness,
                tamed.sleepiness
            ),
            Err(e) => log::warn!("не удалось записать нормализацию характеристик: {e}"),
        }
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
            // E-core: запись только в файл своего устройства (ТЗ §3.5).
            Journal::append(&self.journal_dir, self.clock.device(), &ev)?;
        }
        // Часы подтянуты под журнал при старте: новый id строго больше
        // всех прежних, сортировка сохраняется без пересортировки.
        self.events.push(ev);
        self.refold();
        // Фаза E: локальное событие — повод синкнуться немедленно.
        if let Some(sync) = &self.sync {
            sync.send(SyncCmd::Wake);
        }
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
    /// Явный dismiss — воля пользователя: он же отпускает lease
    /// присутствия и снимает lease-скрытие (фаза E).
    fn dismiss(&mut self, now: f64) -> Response {
        self.close_sleep(now);
        self.clear_effects();
        self.presence_anim = None;
        self.lease_hidden = false;
        if let Some(sync) = &self.sync {
            sync.send(SyncCmd::SetSummoned(false));
            sync.send(SyncCmd::Release);
        }
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

    /// Догнать спрайты до свёртки (B4/B6): смена стадии, базового размера
    /// или цвета тела пересоздаёт набор кадров и перенастраивает питомца;
    /// переход Egg -> дальше играет вылупление и пузырь «Привет!».
    fn sync_visuals(&mut self, now: f64) {
        let stage = self.derived.stage;
        let base = self.derived.attributes.clamped().size;
        let color = self.derived.color;
        if stage == self.sprite_stage && base == self.sprite_base && color == self.sprite_color {
            return;
        }
        let hatched = self.sprite_stage == Stage::Egg && stage > Stage::Egg;
        if color != self.sprite_color {
            log::info!(
                "перекраска: спрайты пересозданы под #{:06x}",
                color & 0x00ff_ffff
            );
        }
        let (old_stage, old_px) = (self.sprite_stage, self.sprites.size);
        // Присваивание дропает набор прежней стадии — кэшируется ровно один.
        self.sprites = stage_sprites(base, stage, color);
        if stage != old_stage {
            // Видимость роста в логе — парная строка к «перекраске» выше.
            log::info!(
                "рост: {} -> {} (спрайт {} -> {} px)",
                old_stage.as_str(),
                stage.as_str(),
                old_px,
                self.sprites.size
            );
        }
        self.sprite_stage = stage;
        self.sprite_base = base;
        self.sprite_color = color;
        let attrs = self.derived.attributes;
        if let Some(pet) = &mut self.pet {
            pet.apply_config(
                attrs.behavior_config_for(stage, self.physics.height_m()),
                self.sprites.size as f32,
            );
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
        // Подсветка строк — осветлённый цвет питомца (контраст на карточке).
        let accent = palette::lighten(self.derived.color, MENU_ACCENT_LIGHTEN);
        let (frame, rects) = text::menu_frame(&refs, None, MENU_PX, accent);
        let origin = clamp_menu_origin(p, (frame.w as f32, frame.h as f32), &world.screen);
        self.menu = Some(Menu {
            origin,
            rows,
            frame,
            rects,
            hovered: None,
            accent,
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
                self.derived
                    .attributes
                    .behavior_config_for(self.derived.stage, self.physics.height_m()),
                // Сид из битов монотонного времени: дёшево и достаточно.
                now.to_bits(),
            );
            // Яйцо не ходит (B6) — стоит, где вылупится.
            pet.set_grounded_only(self.derived.stage == Stage::Egg);
            pet.set_sleep_scale(sleep_scale_for(self.derived.stats.energy));
            self.pet = Some(pet);
            log::info!("summon: питомец появился в ({:.0}, {:.0})", pos.x, pos.y);
        }
        Response::Ok
    }

    // -- Присутствие (фаза E, ТЗ §3.5): lease + «убежал/прибежал» ------------

    /// Призыв по воле пользователя (IPC/трей): после lease-скрытия питомец
    /// ПРИБЕГАЕТ от края (фирменный UX), иначе — обычный summon. В любом
    /// случае забираем lease: питомец теперь здесь. Локально-первично:
    /// питомец появляется сразу, не дожидаясь сервера (оффлайн — штатно).
    fn summon_requested(&mut self, now: f64) -> Response {
        let run_in = self.lease_hidden && self.pet.is_none() && self.derived.stage != Stage::Egg;
        let resp = if run_in {
            self.summon_run_in(now)
        } else {
            self.summon(now)
        };
        if resp == Response::Ok {
            self.lease_hidden = false;
            if let Some(sync) = &self.sync {
                sync.send(SyncCmd::SetSummoned(true));
                sync.send(SyncCmd::Claim);
            }
        }
        resp
    }

    /// Появление с пробежкой от края (возврат после «убежал»). Питомец
    /// ставится за левым краем на уровне пола и бежит внутрь экрана.
    fn summon_run_in(&mut self, now: f64) -> Response {
        let Some(world) = &self.world else {
            return Response::Error(fl!("daemon-output-not-ready"));
        };
        if self.pet.is_none() {
            let size = self.sprites.size as f32;
            let start = Vec2::new(world.screen.x - size, world.ground_y());
            let mut pet = Pet::new(
                start,
                size,
                self.derived
                    .attributes
                    .behavior_config_for(self.derived.stage, self.physics.height_m()),
                now.to_bits(),
            );
            pet.state = PetState::Walk;
            pet.vel = Vec2::default();
            pet.facing = Direction::Right;
            pet.set_sleep_scale(sleep_scale_for(self.derived.stats.energy));
            self.pet = Some(pet);
            self.presence_anim = Some(PresenceAnim::RunIn {
                dir: 1.0,
                target_x: world.screen.x + world.screen.w * PRESENCE_RUN_IN_DEPTH,
            });
            log::info!("присутствие: питомец прибегает (run-in)");
        }
        Response::Ok
    }

    /// Lease забрало другое устройство: «питомец перебегает» — быстрый бег
    /// к ближайшему краю и снятие с экрана БЕЗ события журнала (присутствие
    /// — не уход, ТЗ §3.5). Яйцо бегать не умеет — исчезает сразу; за
    /// fullscreen анимацию всё равно не видно.
    fn presence_lost(&mut self, holder: &str, now: f64) {
        if !self.derived.summoned && self.pet.is_none() {
            return;
        }
        log::info!("присутствие: lease у {holder} — питомец перебегает");
        self.lease_hidden = true;
        self.menu = None;
        let Some(pet) = &mut self.pet else { return };
        let instant = self.derived.stage == Stage::Egg || self.fullscreen_hidden;
        if instant {
            self.pet = None;
            self.presence_anim = None;
            self.clear_effects();
            self.close_sleep(now);
            log::info!("присутствие: питомец скрыт (без пробежки)");
            return;
        }
        let Some(world) = &self.world else {
            self.pet = None;
            return;
        };
        // Со стены/потолка (фаза G) питомец сперва встаёт на ноги —
        // пробежку присутствия демон ведёт по полу.
        pet.detach_to_floor();
        // Ближайший край; спящего пробежка будит (Slept закроет note_sleep).
        let dir = if pet.pos.x - world.screen.x <= world.screen.right() - pet.pos.x {
            -1.0
        } else {
            1.0
        };
        pet.state = PetState::Walk;
        pet.state_time = 0.0;
        pet.vel = Vec2::default();
        self.presence_anim = Some(PresenceAnim::RunOff { dir });
    }

    /// Claim удался: если питомец был lease-скрыт, а журнал считает его
    /// призванным — вернуть с пробежкой (гонки claim/summon идемпотентны).
    fn presence_gained(&mut self, now: f64) {
        if self.lease_hidden && self.pet.is_none() && self.derived.summoned {
            let resp = if self.derived.stage == Stage::Egg {
                self.summon(now)
            } else {
                self.summon_run_in(now)
            };
            if resp == Response::Ok {
                self.lease_hidden = false;
            }
        }
    }

    /// Пользователь взаимодействует с питомцем (нажатие/меню) — питомец
    /// теперь на этом устройстве: забрать lease (с троттлингом).
    fn user_claim(&mut self) {
        let Some(sync) = &self.sync else { return };
        let due = self
            .last_claim
            .is_none_or(|t| t.elapsed() >= CLAIM_THROTTLE);
        if due {
            self.last_claim = Some(Instant::now());
            sync.send(SyncCmd::Claim);
        }
    }

    /// Разобрать заметки синк-воркера (фаза E): чужие события — в память
    /// и свёртку, потеря/возврат lease — в анимации присутствия.
    fn drain_sync_notes(&mut self, now: f64) {
        let mut remote: Vec<JournalEvent> = Vec::new();
        let mut lost: Option<String> = None;
        let mut gained = false;
        if let Some(sync) = &self.sync {
            while let Ok(note) = sync.notes.try_recv() {
                match note {
                    SyncNote::Remote(events) => remote.extend(events),
                    SyncNote::LeaseLost { holder } => lost = Some(holder),
                    SyncNote::LeaseGained => gained = true,
                }
            }
        }
        if !remote.is_empty() {
            // Часы подтягиваются под чужие метки (и под свои, вернувшиеся
            // с сервера после потери локального журнала) — повторная
            // выдача уже занятых id исключена.
            for ev in &remote {
                self.clock.catch_up(&ev.id);
            }
            let added = apply_remote(&mut self.events, &remote);
            if added > 0 {
                log::info!("синк: влито чужих событий: {added}");
                self.refold();
                // Имя/цвет/статы/стадия обновляются вживую.
                self.sync_visuals(now);
            }
        }
        if let Some(holder) = lost {
            self.presence_lost(&holder, now);
        }
        if gained {
            self.presence_gained(now);
        }
    }

    /// Тик сценарной пробежки присутствия (вместо обычного Pet::tick).
    fn presence_anim_tick(&mut self, dt: f32, now: f64) {
        let Some(anim) = self.presence_anim else {
            return;
        };
        let (Some(pet), Some(world)) = (&mut self.pet, &self.world) else {
            self.presence_anim = None;
            return;
        };
        // Ручное ведение: машина поведения выключена, вид — быстрый шаг.
        pet.state = PetState::Walk;
        pet.state_time += dt;
        pet.vel = Vec2::default();
        match anim {
            PresenceAnim::RunOff { dir } => {
                pet.facing = if dir < 0.0 {
                    Direction::Left
                } else {
                    Direction::Right
                };
                pet.pos.x += dir * PRESENCE_RUN_SPEED * dt;
                let b = pet.bounds();
                let gone = b.right() < world.screen.x || b.x > world.screen.right();
                if gone {
                    self.pet = None;
                    self.presence_anim = None;
                    self.clear_effects();
                    self.close_sleep(now);
                    log::info!("присутствие: питомец убежал за край (журнал не тронут)");
                }
            }
            PresenceAnim::RunIn { dir, target_x } => {
                pet.facing = if dir < 0.0 {
                    Direction::Left
                } else {
                    Direction::Right
                };
                pet.pos.x += dir * PRESENCE_RUN_SPEED * dt;
                let arrived =
                    (dir > 0.0 && pet.pos.x >= target_x) || (dir < 0.0 && pet.pos.x <= target_x);
                if arrived {
                    pet.pos.x = target_x;
                    // Дальше решает обычная машина поведения (сразу).
                    pet.state_left = 0.0;
                    self.presence_anim = None;
                    log::info!("присутствие: питомец прибежал");
                }
            }
        }
    }

    /// Горячая перечитка чужих журналов в режиме «папки» (фаза E):
    /// раз в `folder_poll` сверяем подписи (len+mtime) чужих
    /// journal.*.jsonl; изменились — перечитываем каталог и вливаем новое.
    fn poll_folder(&mut self, now: f64) {
        if self.sync_cfg.mode != SyncMode::Folder {
            return;
        }
        if self
            .folder_poll_at
            .is_some_and(|t| t.elapsed() < self.folder_poll)
        {
            return;
        }
        self.folder_poll_at = Some(Instant::now());
        let seen = folder_signature(&self.journal_dir, self.clock.device());
        if seen == self.folder_seen {
            return;
        }
        self.folder_seen = seen;
        match Journal::open(&self.journal_dir) {
            Ok((events, warnings)) => {
                if warnings > 0 {
                    log::warn!("папка: пропущено битых строк: {warnings}");
                }
                for ev in &events {
                    self.clock.catch_up(&ev.id);
                }
                let added = apply_remote(&mut self.events, &events);
                if added > 0 {
                    log::info!("папка: синкер принёс событий: {added}");
                    self.refold();
                    self.sync_visuals(now);
                }
                self.folder_merged_at = Some(Instant::now());
            }
            Err(e) => log::warn!("папка: журналы не перечитаны: {e}"),
        }
    }

    /// Ответ на `ctl sync status` (фаза E).
    fn sync_status(&self) -> Response {
        let cursors = cursors_of(&self.events);
        let mode = self.sync_cfg.mode;
        let (last_push, last_pull, holding, holder, last_error) = match (&self.sync, mode) {
            (Some(sync), _) => {
                let sh = sync.shared.lock().unwrap();
                (
                    sh.last_push,
                    sh.last_pull,
                    sh.holding,
                    sh.holder.clone(),
                    sh.last_error.clone(),
                )
            }
            // Папка: push = наши append-ы в свой файл (мгновенно, синкер
            // заберёт сам), pull = последняя перечитка чужих файлов.
            (None, SyncMode::Folder) => (None, self.folder_merged_at, false, None, None),
            (None, _) => (None, None, false, None, None),
        };
        Response::SyncStatus {
            mode: mode.as_str().to_string(),
            address: (mode == SyncMode::Server && !self.sync_cfg.address.trim().is_empty())
                .then(|| self.sync_cfg.address.trim().to_string()),
            folder: (mode == SyncMode::Folder).then(|| self.journal_dir.display().to_string()),
            last_push_secs: last_push.map(|t| t.elapsed().as_secs()),
            last_pull_secs: last_pull.map(|t| t.elapsed().as_secs()),
            devices: cursors.0.len() as u32,
            events: self.events.len() as u64,
            holding,
            holder,
            last_error,
        }
    }

    /// Обработать один IPC-запрос. Каждый запрос получает ровно один ответ.
    fn handle(&mut self, req: Request, now: f64) -> Response {
        match req {
            Request::Summon => {
                let resp = self.summon_requested(now);
                // Призванность переживает рестарт (ТД-17) — событием журнала.
                if resp == Response::Ok && !self.derived.summoned {
                    return self.care(EventKind::Summoned);
                }
                resp
            }
            Request::Dismiss => self.dismiss(now),
            Request::SyncStatus => self.sync_status(),
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
                    color: self.derived.color,
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
            Request::Recolor(argb) => {
                // Косметика уровня пользователя: цвет — событие журнала,
                // альфа принудительно ff; спрайты догонит sync_visuals
                // этого же тика.
                self.care(EventKind::Recolored {
                    argb: 0xff00_0000 | (argb & 0x00ff_ffff),
                })
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
            pet.apply_config(
                a.behavior_config_for(self.derived.stage, self.physics.height_m()),
                keep_size,
            );
        }
        log::info!("set_attributes: применены {a:?}");
        self.care(EventKind::AttributesSet { attributes: a })
    }

    /// Перечитать config.toml (настройки приложения). Характеристики питомца
    /// сюда больше не входят — они меняются только через SetAttributes.
    /// Секция [sync] применяется на лету (фаза E): воркер пересоздаётся,
    /// смена каталога журналов переоткрывает хранилище.
    fn reload(&mut self) -> Response {
        match Config::load() {
            Ok(cfg) => {
                log::info!("reload: настройки приложения перечитаны");
                self.physics = cfg.physics;
                self.apply_pet_config();
                self.apply_sync_config(cfg.sync);
                Response::Ok
            }
            Err(e) => Response::Error(fl!("daemon-config-unreadable", error = e)),
        }
    }

    /// Применить физику из конфига к живому питомцу (после reload).
    fn apply_pet_config(&mut self) {
        let cfg = self
            .derived
            .attributes
            .behavior_config_for(self.derived.stage, self.physics.height_m());
        let size = self.sprites.size as f32;
        if let Some(pet) = &mut self.pet {
            pet.apply_config(cfg, size);
        }
    }

    /// Применить новую секцию [sync] (фаза E). Старый воркер завершается
    /// дропом ручки; смена каталога журналов переоткрывает хранилище
    /// (свой файл догоняет каталог, см. adopt_own_journal) — кроме
    /// деградации без записи, там менять каталог опасно (память ≠ диск).
    fn apply_sync_config(&mut self, cfg: SyncConfig) {
        if cfg == self.sync_cfg {
            log::debug!("reload: [sync] без изменений");
            return;
        }
        for w in cfg.warnings() {
            log::warn!("конфиг синка: {w}");
        }
        self.sync = None; // дроп cmd_tx: воркер увидит и завершится
        let new_dir = resolve_journal_dir(&self.data_dir, &cfg);
        if new_dir != self.journal_dir {
            if self.journal_writable {
                log::info!(
                    "reload: каталог журналов {} -> {}",
                    self.journal_dir.display(),
                    new_dir.display()
                );
                // Старый каталог знает только демон (нового конфига в нём
                // нет) — свой файл догоняет смену отсюда, дальше
                // load_storage добавит кандидатов из самого конфига.
                adopt_own_journal(&[self.journal_dir.as_path()], &new_dir, self.clock.device());
                let storage = load_storage(&self.data_dir, &cfg, None);
                self.events = storage.events;
                self.clock = storage.clock;
                self.journal_writable = storage.writable;
                self.journal_dir = storage.journal_dir;
                self.refold();
            } else {
                log::warn!("reload: журнал в деградации (память без записи) — каталог не меняю");
            }
        }
        self.sync_cfg = cfg;
        if self.sync_cfg.mode == SyncMode::Server {
            self.sync = Some(sync::spawn(
                &self.sync_cfg,
                &self.data_dir,
                &self.journal_dir,
                self.clock.device(),
                self.derived.summoned,
                sync::Tuning::default(),
            ));
        }
        if self.sync_cfg.mode == SyncMode::Folder {
            self.folder_seen = folder_signature(&self.journal_dir, self.clock.device());
            self.folder_poll_at = None;
        }
        // Lease-скрытие принадлежит старому режиму.
        self.lease_hidden = false;
        log::info!("reload: синк в режиме {}", self.sync_cfg.mode.as_str());
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

    /// Опрос worldsense (фаза D): свежий снапшот -> платформы и пол в World,
    /// изменение объявляется питомцу (world_changed), флаг fullscreen
    /// прячет/возвращает сцену. Зовётся из tick ПЕРЕД симуляцией — физика
    /// этого же кадра ходит уже по новому рельефу.
    ///
    /// Троттлинг: активный питомец (Falling/Walk/Dragged) и скрытый режим
    /// (ждём ухода fullscreen) — каждый тик; спокойный — раз в
    /// `sense_poll_idle` (закрытое окно уронит питомца с опозданием ≤2 с).
    fn poll_worldsense(&mut self) {
        let Some(sense) = self.sense.as_mut() else {
            return;
        };
        let busy = self.fullscreen_hidden
            || self
                .pet
                .as_ref()
                .is_some_and(|p| p.pace() == SimPace::Active);
        if !busy {
            if let Some(t) = self.last_sense_poll {
                if t.elapsed() < self.sense_poll_idle {
                    return;
                }
            }
        }
        self.last_sense_poll = Some(Instant::now());
        let snap = sense.latest();
        let Some(world) = &mut self.world else {
            // До геометрии выхода некуда переводить координаты.
            return;
        };

        // Снапшота нет (не-KDE, KWin умер) — штатная деградация: рельеф
        // пустеет, пол возвращается к низу экрана, мир = весь выход.
        let (screen, platforms, ground_override, fullscreen) = match snap {
            Some(s) => {
                let o = self.screen_origin;
                let out_global = Rect::new(o.x, o.y, self.output_rect.w, self.output_rect.h);
                // Мир питомца — рабочая область СВОЕГО выхода (G2): по
                // панелям он не ходит, а на двух мониторах у выходов эти
                // области разные. Нет данных — весь выход, как раньше.
                let screen = s
                    .area_for_output(out_global)
                    .map(|a| Rect::new(a.x - o.x, a.y - o.y, a.w, a.h))
                    .map(|a| intersect(a, self.output_rect))
                    .filter(|a| a.w > 0.0 && a.h > 0.0)
                    .unwrap_or(self.output_rect);
                // Стекинг сверху вниз (как отдаёт worldsense) — обязателен
                // для отсечения накрытых кромок ниже.
                let stack: Vec<Platform> = s
                    .platforms
                    .iter()
                    .map(|p| Platform {
                        rect: Rect::new(p.rect.x - o.x, p.rect.y - o.y, p.rect.w, p.rect.h),
                        id: p.id,
                    })
                    .collect();
                // Стоять можно только на ВИДИМЫХ участках кромок (G2):
                // кромка под чужим окном — это «питомец в воздухе».
                // Карниз уже трети питомца — не место для жизни.
                let size = self.sprites.size as f32;
                let min_ledge = physics::MIN_LEDGE.max(size / 3.0);
                let platforms = physics::visible_ledges(&stack, screen, min_ledge, size);
                // Верх нижней панели -> пол. При известной рабочей области
                // пол — её низ, отдельный override не нужен.
                let ground = if s.screen_areas.is_empty() {
                    s.workspace_bottom
                        .map(|y| y - o.y)
                        .filter(|y| *y > screen.y && *y < screen.bottom())
                } else {
                    None
                };
                (screen, platforms, ground, s.fullscreen_active)
            }
            None => (self.output_rect, Vec::new(), None, false),
        };

        if world.platforms != platforms
            || world.ground_y_override != ground_override
            || world.screen != screen
        {
            world.platforms = platforms;
            world.ground_y_override = ground_override;
            world.screen = screen;
            log::debug!(
                "мир: {:.0}x{:.0} @ ({:.0},{:.0}), карнизов {}, пол {:.0}{}",
                world.screen.w,
                world.screen.h,
                world.screen.x,
                world.screen.y,
                world.platforms.len(),
                world.ground_y(),
                if world.ground_y_override.is_some() {
                    " (панель)"
                } else {
                    " (низ экрана)"
                }
            );
            // Разбор «почему питомец стоит вот тут»: перечисляем видимые
            // карнизы. Дешёвая диагностика — включается RUST_LOG=trace.
            if log::log_enabled!(log::Level::Trace) {
                for p in &world.platforms {
                    log::trace!(
                        "  карниз y={:.0} x={:.0}..{:.0} (окно {:x})",
                        p.rect.y,
                        p.rect.x,
                        p.rect.right(),
                        p.id
                    );
                }
            }
            if let Some(pet) = &mut self.pet {
                let before = pet.state;
                pet.world_changed(world);
                if pet.state != before {
                    // Обычно Sleep/Idle -> Falling: опору увезли/закрыли.
                    log::debug!(
                        "питомец: {:?} -> {:?} со сменой мира в ({:.0}, {:.0})",
                        before,
                        pet.state,
                        pet.pos.x,
                        pet.pos.y
                    );
                }
            }
        }

        if fullscreen != self.fullscreen_hidden {
            self.fullscreen_hidden = fullscreen;
            if fullscreen {
                log::info!("вежливость (D5): полноэкранное окно — питомец прячется");
                // Меню без сцены осталось бы висеть невидимо-некликабельным.
                self.menu = None;
            } else {
                log::info!("вежливость (D5): fullscreen закончился — питомец возвращается");
            }
        }
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

        // Синк (фаза E): заметки воркера (чужие события, lease) и горячая
        // перечитка папки — до свёртки и симуляции кадра.
        self.drain_sync_notes(now);
        self.poll_folder(now);

        // Ленивый декей: свёртка раз в минуту; гейт вылупления (B6) не
        // ждёт минутного таймера — egg_gate_due дешёвый.
        if self.last_fold.elapsed() >= REFOLD_INTERVAL || self.egg_gate_due() {
            self.refold();
        }

        // Спрайты догоняют свёртку (стадия/размер, вылупление), истёкшие
        // эффекты снимаются — pace() после тика видит честное состояние.
        self.sync_visuals(now);
        self.expire_effects(now);

        // Рельеф из worldsense (фаза D) — до симуляции: физика кадра
        // ходит по свежим кромкам, world_changed роняет потерявших опору.
        self.poll_worldsense();

        // dt с прошлого тика; кламп согласован с Pet::tick (ТД-3: при
        // адаптивном темпе Drowsy тики приходят ~раз в секунду).
        let dt = (now - self.last_now.unwrap_or(now)).clamp(0.0, 1.5) as f32;
        self.last_now = Some(now);

        if self.presence_anim.is_some() {
            // Пробежка присутствия (фаза E): демон ведёт питомца сам.
            self.presence_anim_tick(dt, now);
        } else if let (Some(pet), Some(world)) = (&mut self.pet, &self.world) {
            let before = pet.state;
            pet.tick(world, dt);
            // Смены состояния с координатами — отладка физики фазы D
            // («на какой кромке приземлился», «где сошёл с окна»).
            if pet.state != before {
                log::debug!(
                    "питомец: {:?} -> {:?} в ({:.0}, {:.0})",
                    before,
                    pet.state,
                    pet.pos.x,
                    pet.pos.y
                );
            }
        }

        // Периоды сна: начало/конец (естественный или прерванный) — Slept.
        self.note_sleep(now);

        match (&self.pet, &self.world) {
            // Вежливость (D5): под fullscreen-приложением сцена пустая —
            // ни спрайтов, ни input region; симуляция уже оттикала выше.
            (Some(_), Some(_)) if self.fullscreen_hidden => Scene {
                sprites: Vec::new(),
                input_rects: Vec::new(),
            },
            (Some(pet), Some(world)) => {
                // На стене и под потолком рисунок прижимается к поверхности:
                // пустое поле кадра иначе оставило бы питомца висеть в
                // паре пикселей от неё. Хит-область едет вместе с рисунком.
                let bounds = grip_shift(pet.bounds(), pet.surface, &self.sprites);
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
                    surface: pet.surface,
                    idle_action: pet.idle_action(),
                };
                // Фаза анимации оверлея — от его старта, не от state_time;
                // у мелких занятий в покое (фаза G) — своя фаза.
                let t = match &self.overlay {
                    Some(o) => (now - o.from) as f32,
                    None => pet.anim_time(),
                };
                let mut sprites = vec![SpriteInstance {
                    frame: self.sprites.frame_look(&look, t),
                    origin: Vec2::new(bounds.x, bounds.y),
                    // Поворот под поверхность + зеркало по взгляду (фаза G).
                    orient: pet.orient(),
                }];
                // Пробегающего мимо (run-off/run-in, фаза E) не поймать:
                // хит-области нет, указатель проходит насквозь.
                let mut input_rects = if self.presence_anim.is_some() {
                    Vec::new()
                } else {
                    vec![bounds]
                };
                // Пузырь — над питомцем, в пределах экрана, без хит-области.
                // Под потолком (фаза G) места сверху нет — пузырь уходит вниз.
                if let Some(bubble) = self.bubble.as_ref().filter(|b| now >= b.from) {
                    let (bw, bh) = (bubble.frame.w as f32, bubble.frame.h as f32);
                    let cx = bounds.x + bounds.w / 2.0;
                    let x = (cx - bw / 2.0).clamp(
                        world.screen.x,
                        (world.screen.right() - bw).max(world.screen.x),
                    );
                    let above = bounds.y - bh - BUBBLE_GAP;
                    let y = if above >= world.screen.y {
                        above
                    } else {
                        (bounds.bottom() + BUBBLE_GAP).min(world.ground_y() - bh)
                    };
                    sprites.push(SpriteInstance {
                        frame: &bubble.frame,
                        origin: Vec2::new(x, y),
                        orient: Orient::IDENTITY,
                    });
                }
                // Меню — поверх всего (последним в порядке блита) + хит-зона.
                if let Some(menu) = &self.menu {
                    sprites.push(SpriteInstance {
                        frame: &menu.frame,
                        origin: menu.origin,
                        orient: Orient::IDENTITY,
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
            Event::OutputGeometry {
                width,
                height,
                origin,
            } => {
                log::info!(
                    "выход: {width:.0}x{height:.0} @ ({:.0}, {:.0})",
                    origin.x,
                    origin.y
                );
                let first = self.world.is_none();
                self.screen_origin = origin;
                let screen = Rect::new(0.0, 0.0, width, height);
                self.output_rect = screen;
                match &mut self.world {
                    // Экран поменялся — рельеф (платформы/пол) переживает:
                    // ближайший poll_worldsense пересчитает его сам.
                    Some(world) => world.screen = screen,
                    None => self.world = Some(World::new(screen)),
                }
                // Демон стартует с питомцем на экране — но только если его
                // не убирали до рестарта (ТД-17: dismissed в журнале).
                if first && self.derived.summoned {
                    let greeted = self.summon(now) == Response::Ok;
                    // Приветствие при старте (B5) — для уже вылупившихся:
                    // яйцо поздоровается после вылупления (B6).
                    if greeted && self.derived.stage != Stage::Egg {
                        self.bubble = Some(hello_bubble(now, HELLO_START_SECS));
                    }
                    // Присутствие (фаза E): запуск демона = активность на
                    // этом устройстве, питомец перебегает сюда.
                    if greeted {
                        if let Some(sync) = &self.sync {
                            sync.send(SyncCmd::SetSummoned(true));
                            sync.send(SyncCmd::Claim);
                        }
                    }
                }
                true
            }
            // Пока меню открыто, указатель принадлежит меню: нажатие —
            // выбор строки/закрытие, движение — подсветка; питомцу эти
            // события не отдаются (клик по меню — не drag и не гладь).
            Event::PointerPress(p) => {
                // Нажатие по нашей поверхности = живой пользователь ЗДЕСЬ:
                // питомец принадлежит этому устройству (lease, фаза E).
                self.user_claim();
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
            // Захват потерян не по воле пользователя: питомец выпадает из
            // руки на месте, БЕЗ броска (фаза G3).
            Event::PointerCancel(_) => {
                if let Some(pet) = &mut self.pet {
                    if pet.cancel_drag() {
                        log::debug!("захват отменён — питомец выпал из руки, без броска");
                    }
                }
                true
            }
            // ПКМ по питомцу — открыть меню (B3); повторный ПКМ закрывает.
            Event::PointerMenu(p) => {
                self.user_claim();
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
    /// оверлеи, пузыри и пробежки присутствия держат Active только пока
    /// видимы/анимируются — expire_effects и presence_anim_tick снимают
    /// их, и темп деэскалирует сам.
    fn pace(&self) -> Pace {
        let effects_active = self.menu.is_some()
            || self.overlay.is_some()
            || self.bubble.is_some()
            || self.happy_until.is_some()
            || self.presence_anim.is_some();
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
    /// config.toml и env (ТД-26). Worldsense-провайдера нет: настоящий
    /// грузил бы KWin-скрипт в живой композитор (см. FakeSense); синк
    /// выключен (SyncConfig::default) — ни сети, ни воркера.
    fn app_in(dir: &Path) -> (DaemonApp, Sender<IpcMessage>) {
        let (tx, rx) = mpsc::channel();
        (
            DaemonApp::new(
                rx,
                dir.to_path_buf(),
                None,
                None,
                None,
                SyncConfig::default(),
            ),
            tx,
        )
    }

    /// Демон со «включённым» серверным синком, но БЕЗ настоящего воркера:
    /// ручные каналы вместо потока — тест сам играет за воркера (шлёт
    /// заметки) и подглядывает команды демона (ТД-26).
    fn sync_app_in(
        dir: &Path,
    ) -> (
        DaemonApp,
        Sender<IpcMessage>,
        Sender<SyncNote>,
        mpsc::Receiver<SyncCmd>,
    ) {
        let (app_base, tx) = app_in(dir);
        let mut app = app_base;
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (note_tx, notes) = mpsc::channel();
        app.sync_cfg.mode = driftling_core::SyncMode::Server;
        app.sync = Some(SyncHandle {
            cmd_tx,
            notes,
            shared: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
        });
        (app, tx, note_tx, cmd_rx)
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
                origin: Vec2::default(),
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

    /// Фаза G: питомец с характеристиками из ручного config.toml эпохи M0
    /// один раз успокаивается событием журнала; повторный старт события
    /// не плодит.
    #[test]
    fn legacy_wild_attributes_are_tamed_once() {
        let dir = tmp_dir("tame");
        let wild = PetAttributes {
            size: 90,
            walk_speed: 400.0,
            curiosity: 100,
            sleepiness: 0,
            ..PetAttributes::default()
        };
        // Демон с legacy-конфигом: Genesis унесёт «дикие» характеристики.
        let (tx, rx) = mpsc::channel::<IpcMessage>();
        let mut app = DaemonApp::new(
            rx,
            dir.clone(),
            Some(wild),
            None,
            None,
            SyncConfig::default(),
        );
        assert_eq!(app.derived.attributes.walk_speed, 400.0);
        app.tame_attributes();
        let calm = app.derived.attributes;
        assert!(calm.walk_speed <= driftling_core::attributes::CALM_WALK_SPEED);
        assert!(calm.curiosity <= driftling_core::attributes::CALM_CURIOSITY);
        assert_eq!(calm.size, 90, "размер не трогаем");
        let kinds = journal_kinds(&dir);
        assert_eq!(
            kinds
                .iter()
                .filter(|k| matches!(k, EventKind::AttributesSet { .. }))
                .count(),
            1
        );

        // Второй запуск на том же каталоге: успокаивать больше нечего.
        drop(tx);
        let (_tx2, rx2) = mpsc::channel::<IpcMessage>();
        let mut again = DaemonApp::new(rx2, dir.clone(), None, None, None, SyncConfig::default());
        again.tame_attributes();
        assert_eq!(
            journal_kinds(&dir)
                .iter()
                .filter(|k| matches!(k, EventKind::AttributesSet { .. }))
                .count(),
            1,
            "нормализация не повторяется"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Фаза G3: усталый питомец спит дольше — множитель растёт линейно
    /// от 1 (бодрый) до 6 (энергия на нуле).
    #[test]
    fn tired_pet_sleeps_longer() {
        assert_eq!(sleep_scale_for(100.0), 1.0);
        assert_eq!(sleep_scale_for(0.0), 6.0);
        assert!((sleep_scale_for(50.0) - 3.5).abs() < 1e-6);
        assert_eq!(sleep_scale_for(-20.0), 6.0, "мусор клампится");
    }

    /// Фаза G2: к стене питомец прижимается ПЕРЕДОМ (переднее поле
    /// профильного кадра), к потолку — ногами (нижнее поле ходьбы),
    /// а на полу рисунок не сдвигается.
    #[test]
    fn grip_shift_hugs_the_surface() {
        let mut sprites = sprite::placeholder(64);
        sprites.grip_inset = sprite::Inset {
            left: 3,
            right: 4,
            top: 2,
            bottom: 5,
        };
        sprites.feet_inset = sprite::Inset {
            left: 1,
            right: 1,
            top: 1,
            bottom: 6,
        };
        let b = Rect::new(100.0, 200.0, 64.0, 64.0);
        assert_eq!(grip_shift(b, Surface::Floor, &sprites), b);
        assert_eq!(grip_shift(b, Surface::WallLeft, &sprites).x, 96.0);
        assert_eq!(grip_shift(b, Surface::WallRight, &sprites).x, 104.0);
        assert_eq!(grip_shift(b, Surface::Ceiling, &sprites).y, 194.0);
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

    /// Recolor: альфа нормализуется, событие в журнале, свёртка и спрайты
    /// перекрашены в том же тике, PetInfo отдаёт цвет.
    #[test]
    fn recolor_appends_event_and_regenerates_sprites() {
        let (mut app, tx, dir) = app("recolor");
        geometry(&mut app);
        assert_eq!(app.sprite_color, driftling_core::DEFAULT_PET_COLOR);
        let before = app.sprites.egg[0].argb.clone();

        // Клиент прислал цвет без альфы — демон нормализует в ff.
        let reply = send(&tx, Request::Recolor(0x00_e8_94_4a));
        app.tick(0.1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert_eq!(app.derived.color, 0xff_e8_94_4a);
        assert_eq!(app.sprite_color, 0xff_e8_94_4a, "спрайты догнали свёртку");
        assert_ne!(app.sprites.egg[0].argb, before, "скорлупа перекрашена");
        assert!(matches!(
            journal_kinds(&dir).last().unwrap(),
            EventKind::Recolored {
                argb: 0xff_e8_94_4a
            }
        ));

        let reply = send(&tx, Request::PetInfo);
        app.tick(0.2);
        match reply.recv().unwrap() {
            Response::PetInfo { color, .. } => assert_eq!(color, 0xff_e8_94_4a),
            other => panic!("неожиданный ответ: {other:?}"),
        }

        // «Рестарт»: цвет переживает пересборку демона из журнала.
        let (app2, _tx2) = app_in(&dir);
        assert_eq!(app2.derived.color, 0xff_e8_94_4a);
        assert_eq!(app2.sprite_color, 0xff_e8_94_4a);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Акцент подсветки меню следует за цветом питомца.
    #[test]
    fn menu_hover_accent_follows_pet_color() {
        let (mut app, tx, dir) = adult_app("menu-accent");
        geometry(&mut app);
        let p = app.pet.as_ref().unwrap().pos;

        app.event(Event::PointerMenu(p), 0.0);
        assert_eq!(
            app.menu.as_ref().unwrap().accent,
            palette::lighten(driftling_core::DEFAULT_PET_COLOR, MENU_ACCENT_LIGHTEN)
        );
        app.event(Event::PointerMotion(row_center(&app, 1)), 0.1);
        let before = app.menu.as_ref().unwrap().frame.argb.clone();
        app.event(Event::PointerMenu(p), 0.2); // закрыть

        let reply = send(&tx, Request::Recolor(0xff_5f_bf_8f));
        app.tick(0.3);
        assert_eq!(reply.recv().unwrap(), Response::Ok);

        app.event(Event::PointerMenu(p), 0.4);
        app.event(Event::PointerMotion(row_center(&app, 1)), 0.5);
        let menu = app.menu.as_ref().unwrap();
        assert_eq!(
            menu.accent,
            palette::lighten(0xff_5f_bf_8f, MENU_ACCENT_LIGHTEN)
        );
        assert_ne!(menu.frame.argb, before, "подсветка в новом акценте");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_attributes_applies_and_petinfo_reports() {
        // Данные изолированы через DI-каталог, env не трогаем (ТД-26).
        let (mut app, tx, dir) = app("attrs");
        geometry(&mut app);

        let attrs = PetAttributes {
            size: 128,
            walk_speed: 120.0,
            ..PetAttributes::default()
        };
        let reply = send(&tx, Request::SetAttributes(attrs));
        app.tick(0.0);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        // Спрайты пересозданы под новый базовый размер С УЧЁТОМ стадии:
        // новорождённый — яйцо; ожидание считаем тем же путём, что и демон
        // (арт-пак: target = base * stage_scale, целый множитель native).
        let expected = stage_sprites(128, Stage::Egg, driftling_core::DEFAULT_PET_COLOR).size;
        assert!(
            expected > stage_sprites(32, Stage::Egg, driftling_core::DEFAULT_PET_COLOR).size,
            "базовый размер реально влияет на набор"
        );
        assert_eq!(app.sprites.size, expected);
        assert_eq!(app.pet.as_ref().unwrap().size, expected as f32);
        // Скорость доезжает с поправкой на стадию (фаза G): яйцо
        // двигалось бы медленнее взрослого.
        let expected_speed = 120.0 * sprite::stage_scale(Stage::Egg);
        assert_eq!(
            app.pet.as_ref().unwrap().config().walk_speed,
            expected_speed
        );

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
                assert_eq!(attributes.walk_speed, 120.0);
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
        assert!(scene.sprites[0].orient.flip_x);
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

    // --- Фаза D: worldsense -> мир -> физика ------------------------------

    use driftling_worldsense::{WindowPlatform, WorldSnapshot};

    /// Управляемый провайдер: тест кладёт снапшот, демон его читает.
    /// Настоящий провайдер в тестах запрещён — он грузит скрипт в живой KWin.
    #[derive(Clone, Default)]
    struct FakeSense(Arc<std::sync::Mutex<Option<WorldSnapshot>>>);

    impl FakeSense {
        fn set(&self, snap: Option<WorldSnapshot>) {
            *self.0.lock().unwrap() = snap;
        }
    }

    impl WorldSense for FakeSense {
        fn latest(&mut self) -> Option<WorldSnapshot> {
            self.0.lock().unwrap().clone()
        }
    }

    /// Демон с управляемым worldsense; троттлинг опроса снят (тесты не ждут).
    fn sense_app(tag: &str) -> (DaemonApp, FakeSense, Sender<IpcMessage>, PathBuf) {
        let dir = tmp_dir(tag);
        let sense = FakeSense::default();
        let (tx, rx) = mpsc::channel();
        let mut app = DaemonApp::new(
            rx,
            dir.to_path_buf(),
            None,
            None,
            Some(Box::new(sense.clone())),
            SyncConfig::default(),
        );
        app.sense_poll_idle = Duration::ZERO;
        (app, sense, tx, dir)
    }

    /// Снапшот из окон (x, y, w, h в глобальных координатах), низа рабочей
    /// области и флага fullscreen.
    fn world_snap(
        windows: &[(f32, f32, f32, f32)],
        bottom: Option<f32>,
        fullscreen: bool,
    ) -> WorldSnapshot {
        WorldSnapshot {
            platforms: windows
                .iter()
                .enumerate()
                .map(|(i, &(x, y, w, h))| WindowPlatform {
                    rect: Rect::new(x, y, w, h),
                    id: i as u64 + 1,
                })
                .collect(),
            workspace_bottom: bottom,
            screen_areas: Vec::new(),
            fullscreen_active: fullscreen,
        }
    }

    /// Снапшот в глобальных координатах композитора переводится в локальные
    /// координаты выхода (origin из OutputGeometry), и питомец приземляется
    /// на верхнюю кромку окна; низ workArea становится полом.
    #[test]
    fn worldsense_platforms_translated_and_landable() {
        let (mut app, sense, _tx, dir) = sense_app("sense-land");
        // Выход — «правый монитор» в глобальной точке (1920, 0).
        assert!(app.event(
            Event::OutputGeometry {
                width: 1920.0,
                height: 1080.0,
                origin: Vec2::new(1920.0, 0.0),
            },
            0.0,
        ));
        // Окно под точкой спавна (960 лок. = 2880 глоб.), верх кромки на 700.
        sense.set(Some(world_snap(
            &[(2660.0, 700.0, 600.0, 300.0)],
            Some(1040.0),
            false,
        )));

        let mut now = 0.0;
        settle(&mut app, &mut now, 3.0);
        let world = app.world.as_ref().unwrap();
        assert_eq!(world.platforms.len(), 1);
        let r = world.platforms[0].rect;
        assert_eq!((r.x, r.y, r.w, r.h), (740.0, 700.0, 600.0, 300.0));
        assert_eq!(world.ground_y_override, Some(1040.0), "верх панели — пол");
        let pet = app.pet.as_ref().unwrap();
        assert_eq!(pet.pos.y, 700.0, "питомец стоит на кромке окна");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Фаза G2, два монитора: мир питомца — рабочая область ЕГО выхода,
    /// а не активного экрана. По чужим панелям он не ходит.
    #[test]
    fn world_is_the_work_area_of_own_output() {
        let (mut app, sense, _tx, dir) = sense_app("sense-area");
        // Питомец на правом мониторе (глобально 1920..3840).
        assert!(app.event(
            Event::OutputGeometry {
                width: 1920.0,
                height: 1080.0,
                origin: Vec2::new(1920.0, 0.0),
            },
            0.0,
        ));
        let mut snap = world_snap(&[], None, false);
        snap.screen_areas = vec![
            // Левый монитор: нижняя панель 40 px.
            driftling_worldsense::ScreenArea {
                screen: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                area: Rect::new(0.0, 0.0, 1920.0, 1040.0),
            },
            // Правый (наш): вертикальная панель 60 px справа.
            driftling_worldsense::ScreenArea {
                screen: Rect::new(1920.0, 0.0, 1920.0, 1080.0),
                area: Rect::new(1920.0, 0.0, 1860.0, 1080.0),
            },
        ];
        sense.set(Some(snap));

        let mut now = 0.0;
        settle(&mut app, &mut now, 3.0);
        let world = app.world.as_ref().unwrap();
        assert_eq!(
            (world.screen.x, world.screen.w, world.screen.h),
            (0.0, 1860.0, 1080.0),
            "взята область своего выхода, а не активного"
        );
        assert_eq!(world.ground_y_override, None, "пол — низ своей области");
        // Питомец не заходит на панель: правый край мира — 1860.
        let pet = app.pet.as_mut().unwrap();
        pet.pos.x = 1859.0;
        pet.state = PetState::Walk;
        pet.facing = Direction::Right;
        settle(&mut app, &mut now, 2.0);
        let b = app.pet.as_ref().unwrap().bounds();
        assert!(b.right() <= 1860.0, "зашёл на панель: {b:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Окно закрыли под стоящим питомцем — он теряет опору, падает и долетает
    /// до земли (демонная половина D4: снапшот -> world_changed).
    #[test]
    fn window_vanish_drops_standing_pet() {
        let (mut app, sense, _tx, dir) = sense_app("sense-vanish");
        geometry(&mut app);
        sense.set(Some(world_snap(
            &[(660.0, 700.0, 600.0, 300.0)],
            None,
            false,
        )));
        let mut now = 0.0;
        settle(&mut app, &mut now, 3.0);
        assert_eq!(app.pet.as_ref().unwrap().pos.y, 700.0);

        sense.set(Some(world_snap(&[], None, false)));
        now += 1.0 / 60.0;
        app.tick(now);
        assert!(app.world.as_ref().unwrap().platforms.is_empty());
        assert_eq!(app.pet.as_ref().unwrap().state, PetState::Falling);
        settle(&mut app, &mut now, 3.0);
        assert_eq!(app.pet.as_ref().unwrap().pos.y, 1080.0, "долетел до земли");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Вежливость (D5): fullscreen прячет сцену целиком (ни спрайтов, ни
    /// input region) и закрывает меню; симуляция живёт за кадром; конец
    /// fullscreen возвращает питомца на место.
    #[test]
    fn fullscreen_hides_scene_and_restores() {
        let (mut app, sense, _tx, dir) = sense_app("sense-fs");
        geometry(&mut app);
        let mut now = 0.0;
        settle(&mut app, &mut now, 3.0);
        assert_eq!(app.tick(now).sprites.len(), 1);
        // Открытое меню не должно пережить скрытие.
        let p = app.pet.as_ref().unwrap().pos;
        assert!(app.event(Event::PointerMenu(p), now));
        assert!(app.menu.is_some());

        sense.set(Some(world_snap(&[], None, true)));
        now += 0.1;
        let counts = {
            let scene = app.tick(now);
            (scene.sprites.len(), scene.input_rects.len())
        };
        assert!(app.fullscreen_hidden);
        assert_eq!(counts, (0, 0), "сцена спрятана целиком");
        assert!(app.menu.is_none(), "меню закрыто при скрытии");
        assert!(app.pet.is_some(), "питомец живёт за кадром");

        sense.set(Some(world_snap(&[], None, false)));
        now += 0.1;
        let sprites = app.tick(now).sprites.len();
        assert!(!app.fullscreen_hidden);
        assert_eq!(sprites, 1, "питомец вернулся");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Троттлинг опроса: спокойный питомец читает мир раз в sense_poll_idle;
    /// обнуление интервала (= активный/скрытый режимы) — каждый тик.
    #[test]
    fn sense_poll_throttled_when_calm() {
        let (mut app, sense, _tx, dir) = sense_app("sense-throttle");
        geometry(&mut app);
        let mut now = 0.0;
        settle(&mut app, &mut now, 3.0); // осел, пейс Calm/Drowsy
        assert_ne!(app.pet.as_ref().unwrap().pace(), SimPace::Active);

        app.sense_poll_idle = Duration::from_secs(3600);
        sense.set(Some(world_snap(&[], None, true)));
        now += 1.0 / 60.0;
        app.tick(now);
        assert!(!app.fullscreen_hidden, "спокойный опрос затроттлен");

        app.sense_poll_idle = Duration::ZERO;
        now += 1.0 / 60.0;
        app.tick(now);
        assert!(app.fullscreen_hidden, "без троттлинга снапшот подхвачен");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Провайдер замолчал (None — рестарт KWin, не-KDE): рельеф пустеет, пол
    /// возвращается к низу экрана — штатная деградация, питомец не зависает.
    #[test]
    fn sense_none_degrades_to_bare_ground() {
        let (mut app, sense, _tx, dir) = sense_app("sense-none");
        geometry(&mut app);
        sense.set(Some(world_snap(
            &[(660.0, 700.0, 600.0, 300.0)],
            Some(1040.0),
            false,
        )));
        let mut now = 0.0;
        settle(&mut app, &mut now, 3.0);
        assert_eq!(app.pet.as_ref().unwrap().pos.y, 700.0);

        sense.set(None);
        now += 1.0 / 60.0;
        app.tick(now);
        let world = app.world.as_ref().unwrap();
        assert!(world.platforms.is_empty());
        assert_eq!(world.ground_y_override, None);
        assert_eq!(app.pet.as_ref().unwrap().state, PetState::Falling);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- Синк и присутствие (фаза E) ----

    /// Слить накопленные команды воркеру (ручной приёмник тестов).
    fn drain_cmds(rx: &mpsc::Receiver<SyncCmd>) -> Vec<SyncCmd> {
        let mut out = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            out.push(cmd);
        }
        out
    }

    /// Каждое локальное событие ухода будит воркер (Wake), а призыв на
    /// старте берёт lease (SetSummoned + Claim).
    #[test]
    fn care_append_wakes_sync_worker() {
        let dir = tmp_dir("sync-wake");
        let (mut app, tx, _notes, cmd_rx) = sync_app_in(&dir);
        geometry(&mut app);
        let boot = drain_cmds(&cmd_rx);
        assert!(boot.contains(&SyncCmd::SetSummoned(true)), "{boot:?}");
        assert!(boot.contains(&SyncCmd::Claim), "старт = активность здесь");

        let reply = send(&tx, Request::Feed { treat: false });
        app.tick(0.1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(
            drain_cmds(&cmd_rx).contains(&SyncCmd::Wake),
            "append -> немедленный синк"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Потеря lease: питомец УБЕГАЕТ за ближайший край (без хит-области),
    /// исчезает БЕЗ событий журнала; явный Summon возвращает его пробежкой
    /// от края и снова клеймит lease. Весь цикл не добавляет в журнал ни
    /// одного события — присутствие не уход (ТЗ §3.5).
    #[test]
    fn lease_loss_runs_pet_off_and_summon_runs_back_in() {
        let dir = tmp_dir("lease-runoff");
        std::fs::write(
            dir.join("pet.json"),
            r#"{"schema_version":2,"name":"Бегун","attributes":{},"summoned":true}"#,
        )
        .unwrap();
        let (mut app, tx, notes, cmd_rx) = sync_app_in(&dir);
        geometry(&mut app);
        let mut now = 0.0;
        settle(&mut app, &mut now, 2.0); // приземлился и живёт
        let kinds_before = journal_kinds(&dir);
        drain_cmds(&cmd_rx);

        notes
            .send(SyncNote::LeaseLost {
                holder: "работа".into(),
            })
            .unwrap();
        now += 1.0 / 60.0;
        let no_hit = app.tick(now).input_rects.is_empty();
        assert!(no_hit, "убегающего не поймать (хит-области нет)");
        assert!(
            matches!(app.presence_anim, Some(PresenceAnim::RunOff { .. })),
            "началась пробежка к краю"
        );
        assert_eq!(app.pet.as_ref().unwrap().state, PetState::Walk);

        // Добегает до края и исчезает; журнал не тронут.
        settle(&mut app, &mut now, 6.0);
        assert!(app.pet.is_none(), "питомец убежал за край");
        assert!(app.lease_hidden);
        assert!(app.presence_anim.is_none());
        assert_eq!(
            journal_kinds(&dir),
            kinds_before,
            "run-off не пишет событий (присутствие != уход)"
        );
        assert!(app.tick(now + 0.01).sprites.is_empty());

        // Возврат: пользователь призывает — питомец прибегает от края.
        let reply = send(&tx, Request::Summon);
        now += 1.0 / 60.0;
        app.tick(now);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        assert!(
            matches!(app.presence_anim, Some(PresenceAnim::RunIn { .. })),
            "возврат — пробежка от края"
        );
        let pet = app.pet.as_ref().unwrap();
        assert!(
            pet.bounds().right() <= 1.0,
            "старт из-за левого края: {:?}",
            pet.pos
        );
        assert!(!app.lease_hidden);
        let cmds = drain_cmds(&cmd_rx);
        assert!(cmds.contains(&SyncCmd::Claim), "возврат клеймит lease");
        assert!(cmds.contains(&SyncCmd::SetSummoned(true)));

        settle(&mut app, &mut now, 4.0);
        assert!(app.presence_anim.is_none(), "прибежал и живёт сам");
        let pet = app.pet.as_ref().unwrap();
        assert!(pet.pos.x > 100.0, "внутри экрана: {:?}", pet.pos);
        assert_eq!(
            journal_kinds(&dir),
            kinds_before,
            "полный цикл присутствия не пишет в журнал"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Яйцо бегать не умеет: потеря lease прячет его мгновенно.
    #[test]
    fn egg_lease_loss_hides_instantly() {
        let dir = tmp_dir("lease-egg");
        let (mut app, _tx, notes, _cmd_rx) = sync_app_in(&dir);
        geometry(&mut app);
        app.tick(0.0);
        assert!(app.pet.is_some());
        let kinds_before = journal_kinds(&dir);

        notes
            .send(SyncNote::LeaseLost {
                holder: "дом".into(),
            })
            .unwrap();
        let empty_scene = app.tick(0.1).sprites.is_empty();
        assert!(empty_scene);
        assert!(app.pet.is_none(), "яйцо скрыто сразу, без пробежки");
        assert!(app.lease_hidden);
        assert_eq!(journal_kinds(&dir), kinds_before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Чужие события от воркера вливаются вживую: имя и свёртка обновляются
    /// без рестарта (refold + sync_visuals в том же тике).
    #[test]
    fn remote_note_updates_derived_live() {
        let dir = tmp_dir("remote-note");
        let (mut app, _tx, notes, _cmd_rx) = sync_app_in(&dir);
        geometry(&mut app);
        let ev = JournalEvent {
            id: driftling_core::Hlc {
                wall_ms: wall_now_ms(),
                counter: 9,
                device: "другое-устройство".into(),
            },
            kind: EventKind::Renamed {
                name: "Пришелец".into(),
            },
        };
        notes.send(SyncNote::Remote(vec![ev])).unwrap();
        app.tick(0.1);
        assert_eq!(app.derived.name, "Пришелец", "чужое событие применено");
        // Повтор той же заметки идемпотентен.
        let events_len = app.events.len();
        app.tick(0.2);
        assert_eq!(app.events.len(), events_len);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Режим «папки»: чужой journal.*.jsonl, принесённый «синкером»,
    /// подхватывается горячо — без рестарта демона.
    #[test]
    fn folder_mode_hot_reloads_foreign_files() {
        let dir = tmp_dir("folder-hot");
        let (tx, rx) = mpsc::channel();
        let _ = tx; // канал IPC не нужен
        let cfg = SyncConfig {
            mode: driftling_core::SyncMode::Folder,
            ..SyncConfig::default()
        };
        let mut app = DaemonApp::new(rx, dir.to_path_buf(), None, None, None, cfg);
        app.folder_poll = Duration::ZERO;
        geometry(&mut app);
        app.tick(0.0);
        assert_ne!(app.derived.name, "Гость");

        // «Синкер принёс» файл чужого устройства.
        let ev = JournalEvent {
            id: driftling_core::Hlc {
                wall_ms: wall_now_ms(),
                counter: 0,
                device: "ghost".into(),
            },
            kind: EventKind::Renamed {
                name: "Гость".into(),
            },
        };
        Journal::append_remote(&dir, &[ev]).unwrap();
        app.tick(0.1);
        assert_eq!(app.derived.name, "Гость", "горячая перечитка папки");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Смена каталога журналов на лету (reload [sync]): файл своего
    /// устройства следует за конфигом (побеждает самая длинная копия),
    /// история не теряется в обе стороны.
    #[test]
    fn apply_sync_config_moves_journal_dir_both_ways() {
        let dir = tmp_dir("switch-dir");
        let folder = dir.join("synced");
        let (mut app, tx, _dir2) = {
            let (app, tx) = app_in(&dir);
            (app, tx, ())
        };
        geometry(&mut app);
        // Немного истории в data_dir.
        let reply = send(&tx, Request::Feed { treat: false });
        app.tick(0.1);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        let events_before = app.events.len();

        // Включаем папку: журнал уезжает, события целы.
        app.apply_sync_config(SyncConfig {
            mode: driftling_core::SyncMode::Folder,
            folder: folder.display().to_string(),
            ..SyncConfig::default()
        });
        assert_eq!(app.journal_dir, folder);
        assert_eq!(app.events.len(), events_before, "история переехала");
        // Новое событие пишется уже в папку.
        let reply = send(&tx, Request::Play);
        app.tick(0.2);
        assert_eq!(reply.recv().unwrap(), Response::Ok);
        let (in_folder, _) = Journal::open(&folder).unwrap();
        assert_eq!(in_folder.len(), events_before + 1);

        // Выключаем: журнал возвращается в data_dir, длинная копия побеждает.
        app.apply_sync_config(SyncConfig::default());
        assert_eq!(app.journal_dir, dir);
        assert_eq!(
            app.events.len(),
            events_before + 1,
            "возврат не теряет события, дописанные в папке"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ctl sync status` отвечает и в выключенном режиме.
    #[test]
    fn sync_status_answers_when_off() {
        let (mut app, tx, dir) = app("sync-status");
        let reply = send(&tx, Request::SyncStatus);
        app.tick(0.0);
        match reply.recv().unwrap() {
            Response::SyncStatus {
                mode,
                devices,
                events,
                holding,
                ..
            } => {
                assert_eq!(mode, "off");
                assert_eq!(devices, 1, "своё устройство в курсорах");
                assert!(events >= 1, "Genesis в журнале");
                assert!(!holding);
            }
            other => panic!("неожиданный ответ: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
