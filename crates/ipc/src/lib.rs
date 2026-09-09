//! Управляющий сокет: `driftling ctl …` разговаривает с демоном
//! JSON-строками (одна строка = одно сообщение) через unix-сокет
//! в `$XDG_RUNTIME_DIR/driftling.sock`.
//!
//! Протокол v1 (ТД-10): каждая строка — конверт с версией:
//! запрос `{"v":1,"req":{...}}`, ответ `{"v":1,"resp":{...}}`.
//! Несовпадение версий — внятная ошибка с советом перезапустить демон,
//! а не загадочный parse error.

use anyhow::{anyhow, bail, Context, Result};
use driftling_core::PetAttributes;

mod i18n;
use i18n::fl;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Версия wire-протокола. Меняется при любом несовместимом изменении
/// `Request`/`Response`; политика — версии либо равны, либо ошибка.
/// v2 (фаза B): care-команды (Feed/Play/PutToSleep/Rename), в PetInfo
/// добавлены статы и стадия роста.
/// v3: пользовательский цвет питомца — Recolor, в PetInfo добавлен color.
/// v4 (фаза E): SyncStatus — статус синхронизации для ctl и настроек.
/// v5 (фаза H): мир вещей — Ride/Mob/Toy/StopRide/PlaceProp/TakeProp,
/// картина мира (World), конфиг через демона (GetConfig/SetConfig) и
/// честный ответ на Reload (Reloaded вместо голого Ok).
pub const PROTOCOL_VERSION: u32 = 5;

/// Сколько сервер ждёт строку запроса от подключившегося клиента,
/// прежде чем молча бросить соединение (ТД-12: защита от зависших клиентов).
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Вещь на экране глазами окна настроек: машинные имена, локализует UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropInfo {
    pub id: u64,
    /// Машинное имя вида ([`driftling_core::PropKind::as_str`]).
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    /// Состояние: rest | falling | held | carried | ridden.
    pub state: String,
    /// Живёт в журнале (переживает рестарт и синкается).
    pub persistent: bool,
    /// Незваный гость (режим войны), а не вещь быта.
    pub mob: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Summon,
    Dismiss,
    Status,
    /// Полная карточка питомца для настроек: имя, характеристики, состояние.
    PetInfo,
    /// Покормить: `treat=false` — обычная еда, `treat=true` — вкусняшка
    /// (настроение выше, но злоупотребление бьёт по здоровью, ТЗ §3.2).
    Feed {
        treat: bool,
    },
    /// Поиграть с питомцем.
    Play,
    /// Уложить спать.
    PutToSleep,
    /// Запустить на экран незваного гостя (фаза H6, режим войны).
    /// `kind` — dustball, roach, bug; None — случайный.
    Mob {
        kind: Option<String>,
    },
    /// Прокатить питомца (фаза H5). `kind` — вид транспорта (skate, bike,
    /// moped, car, copter, plane); None — случайный. Повторный вызов во
    /// время поездки высаживает питомца.
    Ride {
        kind: Option<String>,
    },
    /// Переименовать питомца (= событие журнала).
    Rename(String),
    /// Перекрасить питомца: базовый цвет тела ARGB8888 (альфу демон
    /// нормализует в ff). Пользовательская косметика, не дебаг.
    Recolor(u32),
    /// ТОЛЬКО дебаг-панель (ТЗ §3.3): напрямую задать характеристики.
    /// Демон клампит значения, применяет к живому питомцу и персистит.
    SetAttributes(PetAttributes),
    /// Прочитать настройки глазами демона: окно не читает config.toml само
    /// и не может разойтись с тем, что реально применено. Токен синка не
    /// отдаётся никогда — только флаг «задан».
    GetConfig,
    /// Записать настройки патчем по секциям и применить их на лету.
    SetConfig {
        patch: driftling_core::ConfigPatch,
    },
    /// Картина мира для окна настроек: что сейчас на экране.
    World,
    /// Достать/убрать мяч (то же, что кнопка «Мяч» в меню). None —
    /// переключить.
    Toy {
        show: Option<bool>,
    },
    /// Высадить питомца из транспорта (идемпотентно).
    StopRide,
    /// Поставить вещь в мир (постоянная — событием журнала).
    PlaceProp {
        kind: String,
    },
    /// Убрать вещь из мира.
    TakeProp {
        kind: String,
    },
    /// Убрать питомца с экрана по-человечески: помашет и убежит за край.
    DismissWithWave,
    /// Перечитать config.toml (настройки приложения). Демон перечитывает
    /// и секцию [sync] — воркер синка переconfигурируется на лету (фаза E).
    Reload,
    /// Статус синхронизации (фаза E): режим, последние push/pull, курсоры,
    /// держатель lease. Работает в любом режиме (off — почти пустой ответ).
    SyncStatus,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Ok,
    /// Ответ на Reload/SetConfig: что применилось на лету, что потребует
    /// перезапуска и о чём стоит знать. Строки машинные — локализует UI.
    /// Голый Ok здесь врал: смена каталога журналов в деградации записи
    /// молча не применялась, а клиент рапортовал успех.
    Reloaded {
        applied: Vec<String>,
        needs_restart: Vec<String>,
        warnings: Vec<String>,
    },
    /// Настройки глазами демона (ответ на GetConfig).
    Config {
        config: driftling_core::Config,
        /// Путь к файлу — окно показывает его и умеет открыть.
        path: String,
        /// Токен синка задан (сам токен наружу не отдаётся).
        token_set: bool,
    },
    /// Картина мира (ответ на World).
    World {
        /// Рабочая область: None — экран ещё не известен (нет геометрии).
        screen: Option<(f32, f32, f32, f32)>,
        ground_y: Option<f32>,
        /// Питомец спрятан вежливостью к полноэкранному окну.
        fullscreen_hidden: bool,
        /// Питомец сейчас катается на этом транспорте.
        ride: Option<String>,
        props: Vec<PropInfo>,
    },
    /// Поставленная вещь (ответ на PlaceProp).
    Prop(PropInfo),
    Status {
        pets: u32,
        state: String,
        uptime_secs: u64,
    },
    PetInfo {
        name: String,
        /// None = питомец сейчас убран с экрана (dismiss).
        state: Option<String>,
        attributes: PetAttributes,
        /// Статы тамагочи (сытость/энергия/настроение/здоровье).
        stats: driftling_core::PetStats,
        /// Стадия роста (машинное имя, локализует UI).
        stage: driftling_core::Stage,
        /// Базовый цвет тела (ARGB, альфа ff) — акцент UI следует за ним.
        color: u32,
        uptime_secs: u64,
    },
    /// Статус синка (фаза E). Времена — «секунд назад» (None = ещё ни разу),
    /// строки режима/держателя — машинные, локализует UI.
    SyncStatus {
        /// Режим: "off" | "server" | "folder" (driftling_core::SyncMode).
        mode: String,
        /// Адрес сервера (mode = server) без токена.
        address: Option<String>,
        /// Каталог журналов (mode = folder).
        folder: Option<String>,
        /// Секунд с последнего успешного push (None — не было).
        last_push_secs: Option<u64>,
        /// Секунд с последнего успешного pull (None — не было).
        last_pull_secs: Option<u64>,
        /// Устройств в курсорах локального журнала (включая своё).
        devices: u32,
        /// Событий в журнале (после слияния).
        events: u64,
        /// Этот демон считает себя держателем lease присутствия.
        holding: bool,
        /// Последний известный держатель lease (device_id; None — неизвестен).
        holder: Option<String>,
        /// Последняя ошибка синка (None — ошибок не было/прошло).
        last_error: Option<String>,
    },
    Error(String),
}

// --- Конверты протокола -------------------------------------------------
//
// Входящие конверты разбираются в два шага: сначала `v` + сырой JSON,
// потом полезная нагрузка. Иначе запрос новой версии с неизвестным
// вариантом енума дал бы parse error вместо ошибки о версии.

#[derive(Serialize)]
struct ReqEnvelopeOut<'a> {
    v: u32,
    req: &'a Request,
}

#[derive(Deserialize)]
struct ReqEnvelopeIn {
    v: u32,
    req: serde_json::Value,
}

#[derive(Serialize)]
struct RespEnvelopeOut<'a> {
    v: u32,
    resp: &'a Response,
}

#[derive(Deserialize)]
struct RespEnvelopeIn {
    v: u32,
    resp: serde_json::Value,
}

// --- Пути ---------------------------------------------------------------

/// Каталог рантайма. Строго `$XDG_RUNTIME_DIR` — /tmp-фолбэка нет (ТД-13):
/// /tmp общий для всех пользователей, сокет там — дыра и источник коллизий.
pub fn runtime_dir() -> Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!(fl!("ipc-no-runtime-dir")))
}

/// Путь сокета внутри заданного каталога рантайма (DI для тестов, ТД-26).
pub fn socket_path_in(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("driftling.sock")
}

/// Путь лок-файла второго инстанса внутри каталога рантайма.
pub fn lock_path_in(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("driftling.lock")
}

/// Путь управляющего сокета в реальном окружении.
pub fn socket_path() -> Result<PathBuf> {
    Ok(socket_path_in(&runtime_dir()?))
}

// --- Клиент -------------------------------------------------------------

/// Клиентский вызов: соединиться, отправить запрос, дождаться ответа.
pub fn call(req: &Request) -> Result<Response> {
    call_in(&runtime_dir()?, req)
}

/// То же, но с явным каталогом рантайма (тесты работают без env-переменных).
pub fn call_in(runtime_dir: &Path, req: &Request) -> Result<Response> {
    let path = socket_path_in(runtime_dir);
    let mut stream = UnixStream::connect(&path)
        .with_context(|| fl!("ipc-connect-failed", path = path.display().to_string()))?;
    // Демон обязан ответить быстро; не виснем вечно, если он замер.
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut line = serde_json::to_string(&ReqEnvelopeOut {
        v: PROTOCOL_VERSION,
        req,
    })?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    let envelope: RespEnvelopeIn = serde_json::from_str(buf.trim())
        .map_err(|_| anyhow!(fl!("ipc-response-no-envelope", version = PROTOCOL_VERSION)))?;
    if envelope.v != PROTOCOL_VERSION {
        bail!(fl!(
            "ipc-incompatible-protocol",
            client = PROTOCOL_VERSION,
            daemon = envelope.v
        ));
    }
    serde_json::from_value(envelope.resp).with_context(|| fl!("ipc-bad-response"))
}

// --- Сервер -------------------------------------------------------------

/// Слушатель на стороне демона. Блокирующий `accept` — запускать в своём
/// потоке; каждое соединение обслуживается отдельным потоком (ТД-11).
///
/// Держит flock на `driftling.lock` весь срок жизни (ТД-13): второй демон
/// не поднимется, а протухший сокет убирается только ПОСЛЕ взятия лока —
/// гонки «оба увидели мёртвый сокет и оба его удалили» нет.
pub struct Server {
    listener: UnixListener,
    socket_path: PathBuf,
    /// Файл под flock; сам лок живёт, пока открыт файл.
    _lock: std::fs::File,
    read_timeout: Duration,
}

impl Server {
    pub fn bind() -> Result<Self> {
        Self::bind_in(&runtime_dir()?)
    }

    /// Занять сокет в явном каталоге рантайма (DI для тестов, ТД-26).
    pub fn bind_in(runtime_dir: &Path) -> Result<Self> {
        // 1. Сначала лок: он — единственный арбитр «кто здесь демон».
        let lock_path = lock_path_in(runtime_dir);
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| {
                fl!(
                    "ipc-lock-open-failed",
                    path = lock_path.display().to_string()
                )
            })?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |_| {
                anyhow!(fl!(
                    "ipc-already-running",
                    path = lock_path.display().to_string()
                ))
            },
        )?;

        // 2. Лок наш — любой существующий сокет заведомо протухший.
        let socket_path = socket_path_in(runtime_dir);
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| fl!("ipc-bind-failed", path = socket_path.display().to_string()))?;
        Ok(Self {
            listener,
            socket_path,
            _lock: lock,
            read_timeout: READ_TIMEOUT,
        })
    }

    /// Укоротить таймаут чтения соединений (в тестах — миллисекунды).
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Обслуживать соединения до успешно обработанного `Quit`.
    ///
    /// Поток-на-соединение (scoped): отвалившийся или зависший клиент
    /// стоит демону одного потока, а не всего сервера. Обработчик зовётся
    /// из этих потоков, поэтому `Fn + Send + Sync` (без `'static`:
    /// scope гарантирует, что потоки не переживут serve).
    pub fn serve<F>(&self, handler: F) -> Result<()>
    where
        F: Fn(Request) -> Response + Send + Sync,
    {
        let shutdown = AtomicBool::new(false);
        let handler = &handler;
        let shutdown = &shutdown;
        std::thread::scope(|scope| {
            for stream in self.listener.incoming() {
                // Поток Quit-соединения взводит флаг и будит accept
                // холостым подключением к собственному сокету.
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("IPC: accept не удался: {e}");
                        continue;
                    }
                };
                let wake_path = &self.socket_path;
                let timeout = self.read_timeout;
                scope.spawn(move || {
                    if handle_connection(&stream, handler, timeout) {
                        shutdown.store(true, Ordering::SeqCst);
                        let _ = UnixStream::connect(wake_path);
                    }
                });
            }
        });
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Обслужить одно соединение: строка запроса -> строка ответа.
/// Возвращает true, если успешно обработан `Quit` (сигнал остановить serve).
fn handle_connection<F>(stream: &UnixStream, handler: &F, timeout: Duration) -> bool
where
    F: Fn(Request) -> Response,
{
    let _ = stream.set_read_timeout(Some(timeout));
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    match reader.read_line(&mut buf) {
        // Таймаут или обрыв до запроса — молча бросаем соединение (ТД-12).
        Err(e) => {
            log::debug!("IPC: соединение брошено без запроса: {e}");
            return false;
        }
        Ok(0) => return false, // клиент закрыл, не написав ни строки
        Ok(_) => {}
    }

    let (resp, quit) = match serde_json::from_str::<ReqEnvelopeIn>(buf.trim()) {
        Ok(envelope) if envelope.v == PROTOCOL_VERSION => {
            match serde_json::from_value::<Request>(envelope.req) {
                Ok(req) => {
                    let quit = req == Request::Quit;
                    (handler(req), quit)
                }
                Err(e) => (
                    Response::Error(fl!("ipc-bad-request", error = e.to_string())),
                    false,
                ),
            }
        }
        Ok(envelope) => (
            Response::Error(fl!(
                "ipc-incompatible-protocol",
                client = envelope.v,
                daemon = PROTOCOL_VERSION
            )),
            false,
        ),
        Err(_) => (
            Response::Error(fl!("ipc-request-no-envelope", version = PROTOCOL_VERSION)),
            false,
        ),
    };

    // Клиент мог уже отвалиться (EPIPE) — это не ошибка сервера (ТД-11).
    if let Err(e) = write_resp(stream, &resp) {
        log::debug!("IPC: не удалось отправить ответ: {e}");
    }
    // Quit считается обработанным после вызова handler — даже если клиент
    // не дождался ответа, демон обязан завершиться (ТД-14).
    quit
}

fn write_resp(mut stream: &UnixStream, resp: &Response) -> Result<()> {
    let mut line = serde_json::to_string(&RespEnvelopeOut {
        v: PROTOCOL_VERSION,
        resp,
    })?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Временный каталог рантайма; путь передаётся явно (никаких env).
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("driftling-ipc-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Стандартный тестовый обработчик: Status отвечает картинкой,
    /// остальное — Ok.
    fn spawn_server(server: Server) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            server
                .serve(|req| match req {
                    Request::Status => Response::Status {
                        pets: 1,
                        state: "Idle".into(),
                        uptime_secs: 5,
                    },
                    _ => Response::Ok,
                })
                .unwrap();
        })
    }

    /// Отправить сырую строку в сокет и прочитать строку ответа.
    fn raw_exchange(dir: &Path, line: &str) -> String {
        let mut stream = UnixStream::connect(socket_path_in(dir)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(line.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        let mut reader = BufReader::new(stream);
        let mut buf = String::new();
        reader.read_line(&mut buf).unwrap();
        buf
    }

    /// Разобрать ответный конверт; проверяет версию сервера в нём.
    fn parse_resp(line: &str) -> Response {
        let envelope: RespEnvelopeIn = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(envelope.v, PROTOCOL_VERSION);
        serde_json::from_value(envelope.resp).unwrap()
    }

    #[test]
    fn happy_path_roundtrip() {
        let dir = TestDir::new("happy");
        let server = Server::bind_in(dir.path()).unwrap();
        let t = spawn_server(server);

        match call_in(dir.path(), &Request::Status).unwrap() {
            Response::Status { pets, .. } => assert_eq!(pets, 1),
            other => panic!("неожиданный ответ: {other:?}"),
        }
        assert_eq!(call_in(dir.path(), &Request::Quit).unwrap(), Response::Ok);
        t.join().unwrap();
    }

    #[test]
    fn client_with_newer_version_gets_error_and_server_survives() {
        let dir = TestDir::new("vmismatch");
        let server = Server::bind_in(dir.path()).unwrap();
        let t = spawn_server(server);

        let line = raw_exchange(dir.path(), r#"{"v":99,"req":"Status"}"#);
        match parse_resp(&line) {
            // Точное сравнение с fl!-рендером — тест не зависит от локали.
            Response::Error(e) => {
                assert_eq!(
                    e,
                    fl!(
                        "ipc-incompatible-protocol",
                        client = 99,
                        daemon = PROTOCOL_VERSION
                    )
                );
                assert!(e.contains("99"), "нет версии клиента: {e}");
            }
            other => panic!("ожидалась ошибка, получено: {other:?}"),
        }

        // Несовпадение версии не убило сервер.
        assert!(matches!(
            call_in(dir.path(), &Request::Status).unwrap(),
            Response::Status { .. }
        ));
        // И даже Quit чужой версии не остановил serve — только валидный.
        let _ = raw_exchange(dir.path(), r#"{"v":99,"req":"Quit"}"#);
        assert!(matches!(
            call_in(dir.path(), &Request::Status).unwrap(),
            Response::Status { .. }
        ));
        call_in(dir.path(), &Request::Quit).unwrap();
        t.join().unwrap();
    }

    #[test]
    fn daemon_with_other_version_yields_clear_client_error() {
        let dir = TestDir::new("oldd");
        // Поддельный «демон v99»: одно соединение, ответ с чужой версией.
        let listener = UnixListener::bind(socket_path_in(dir.path())).unwrap();
        let t = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut buf = String::new();
            reader.read_line(&mut buf).unwrap();
            let mut s = &stream;
            s.write_all(b"{\"v\":99,\"resp\":\"Ok\"}\n").unwrap();
        });

        let err = call_in(dir.path(), &Request::Status)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            fl!(
                "ipc-incompatible-protocol",
                client = PROTOCOL_VERSION,
                daemon = 99
            )
        );
        t.join().unwrap();
    }

    #[test]
    fn legacy_line_without_envelope_gets_readable_error() {
        let dir = TestDir::new("legacy");
        let server = Server::bind_in(dir.path()).unwrap();
        let t = spawn_server(server);

        // Старый клиент (до v1) слал запрос без конверта.
        let line = raw_exchange(dir.path(), r#""Status""#);
        match parse_resp(&line) {
            Response::Error(e) => {
                assert_eq!(
                    e,
                    fl!("ipc-request-no-envelope", version = PROTOCOL_VERSION)
                );
            }
            other => panic!("ожидалась ошибка, получено: {other:?}"),
        }

        call_in(dir.path(), &Request::Quit).unwrap();
        t.join().unwrap();
    }

    #[test]
    fn early_disconnect_does_not_kill_serve() {
        let dir = TestDir::new("epipe");
        let server = Server::bind_in(dir.path()).unwrap();
        let t = spawn_server(server);

        // Клиент шлёт валидный запрос и рвёт соединение, не читая ответ.
        {
            let mut stream = UnixStream::connect(socket_path_in(dir.path())).unwrap();
            let line = format!("{{\"v\":{PROTOCOL_VERSION},\"req\":\"Status\"}}\n");
            stream.write_all(line.as_bytes()).unwrap();
        } // drop: сокет закрыт, ответ полетит в закрытую трубу

        std::thread::sleep(Duration::from_millis(100));
        assert!(matches!(
            call_in(dir.path(), &Request::Status).unwrap(),
            Response::Status { .. }
        ));
        call_in(dir.path(), &Request::Quit).unwrap();
        t.join().unwrap();
    }

    #[test]
    fn silent_client_is_dropped_by_read_timeout() {
        let dir = TestDir::new("timeout");
        let server = Server::bind_in(dir.path())
            .unwrap()
            .with_read_timeout(Duration::from_millis(50));
        let t = spawn_server(server);

        // Молчаливый клиент: подключился и ничего не пишет.
        let silent = UnixStream::connect(socket_path_in(dir.path())).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        // Сервер бросил его и продолжает обслуживать остальных.
        assert!(matches!(
            call_in(dir.path(), &Request::Status).unwrap(),
            Response::Status { .. }
        ));
        drop(silent);
        call_in(dir.path(), &Request::Quit).unwrap();
        t.join().unwrap();
    }

    #[test]
    fn second_daemon_is_rejected_by_lock() {
        let dir = TestDir::new("lock");
        let _first = Server::bind_in(dir.path()).unwrap();
        let err = match Server::bind_in(dir.path()) {
            Ok(_) => panic!("второй демон не должен был подняться"),
            Err(e) => e.to_string(),
        };
        assert_eq!(
            err,
            fl!(
                "ipc-already-running",
                path = lock_path_in(dir.path()).display().to_string()
            )
        );
    }

    #[test]
    fn stale_socket_is_removed_after_lock() {
        let dir = TestDir::new("stale");
        // Протухший сокет от «убитого» демона: файл есть, никто не слушает.
        {
            let _dead = UnixListener::bind(socket_path_in(dir.path())).unwrap();
        } // listener умер, файл сокета остался

        assert!(socket_path_in(dir.path()).exists());
        let server = Server::bind_in(dir.path()).unwrap();
        let t = spawn_server(server);
        assert!(matches!(
            call_in(dir.path(), &Request::Status).unwrap(),
            Response::Status { .. }
        ));
        call_in(dir.path(), &Request::Quit).unwrap();
        t.join().unwrap();
    }
}
