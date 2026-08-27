//! KWin-провайдер (KDE Plasma 6): QML-скрипт внутри KWin шлёт снапшоты
//! окон по D-Bus, мы принимаем их на собственное имя шины.
//!
//! Схема (clean-room, идея как у wl_shimeji kwinsupport, код свой):
//!  1. занимаем имя `org.driftling.WorldSense` и вешаем объект `/sense`
//!     с методом `Update(s)` (zbus в блокирующем режиме, БЕЗ tokio —
//!     см. предупреждение у ksni в корневом Cargo.toml);
//!  2. пишем `assets/kwin/driftling-sense.qml` во временный файл и грузим
//!     в KWin через `org.kde.kwin.Scripting` (`loadDeclarativeScript` +
//!     `run` на объекте `/Scripting/Script<id>` — проверено на KWin 6.3.6);
//!  3. скрипт подписывается на события окон и шлёт JSON-снапшоты (коалесценция
//!     50 мс + heartbeat 5 с) — свежий снапшот всегда лежит в мьютексе;
//!  4. рестарт KWin: скрипты пропадают молча, поэтому при тишине дольше
//!     10 с лениво проверяем `isScriptLoaded` (не чаще раза в 5 с) и при
//!     необходимости перезагружаем скрипт. Любая ошибка D-Bus — деградация
//!     в `None`, никаких паник.
//!
//! Файл скрипта каждый раз получает НОВОЕ имя: QML-движок KWin кэширует
//! компиляцию по URL, и перезагрузка по старому пути может подсунуть старый
//! байткод (проверено живьём).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use zbus::blocking::{connection, Connection, Proxy};

use crate::parse;
use crate::{WorldSense, WorldSnapshot};

/// Текст KWin-скрипта; при загрузке кладётся во временный файл.
const SENSE_QML: &str = include_str!("../../../assets/kwin/driftling-sense.qml");

/// Наше имя на шине; скрипт шлёт `Update` сюда (продублировано в QML).
const BUS_NAME: &str = "org.driftling.WorldSense";
/// Путь объекта-приёмника (продублирован в QML).
const OBJ_PATH: &str = "/sense";
/// Имя плагина в KWin Scripting; по нему же выгрузка.
const PLUGIN_NAME: &str = "driftling-worldsense";

const KWIN_SERVICE: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_IFACE: &str = "org.kde.kwin.Script";

/// Скрипт шлёт heartbeat каждые 5 с; тишина дольше этого — повод проверить,
/// жив ли он (рестарт KWin выгружает скрипты молча).
const STALE_AFTER: Duration = Duration::from_secs(10);
/// Чаще этого KWin не опрашиваем и скрипт не перегружаем.
const PROBE_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Shared {
    snap: Option<WorldSnapshot>,
    /// Когда пришёл последний валидный снапшот.
    at: Option<Instant>,
}

/// Мьютекс без паник: отравление (паника в другом треде) не роняет физику.
fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// D-Bus-объект `/sense`: принимает снапшоты от KWin-скрипта.
struct SenseService {
    shared: Arc<Mutex<Shared>>,
}

#[zbus::interface(name = "org.driftling.WorldSense")]
impl SenseService {
    /// Единственный метод: JSON-снапшот от скрипта (см. driftling-sense.qml).
    fn update(&self, json: &str) {
        if let Some(snap) = parse::parse_and_filter(json) {
            let mut s = lock(&self.shared);
            s.snap = Some(snap);
            s.at = Some(Instant::now());
        } else {
            log::debug!(
                "worldsense: битый JSON от KWin-скрипта ({} байт) — игнор",
                json.len()
            );
        }
    }
}

pub(crate) struct KWinSense {
    conn: Connection,
    shared: Arc<Mutex<Shared>>,
    /// Текущий файл скрипта (для удаления при выгрузке).
    script_file: Option<PathBuf>,
    /// Счётчик загрузок — уникальное имя файла на каждую (кэш QML по URL).
    load_seq: u32,
    /// Последняя проверка `isScriptLoaded` (бэкофф).
    last_probe: Option<Instant>,
}

impl KWinSense {
    /// Поднимает приёмник и грузит скрипт. Ошибка — только если недоступна
    /// сама шина, занято наше имя или на шине нет KWin; неудача загрузки
    /// скрипта не фатальна (долечимся лениво из `latest`).
    pub(crate) fn new() -> anyhow::Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let conn = connection::Builder::session()
            .context("сессионная шина недоступна")?
            .serve_at(
                OBJ_PATH,
                SenseService {
                    shared: Arc::clone(&shared),
                },
            )?
            .build()
            .context("не поднялся приёмник снапшотов")?;
        // Имя шины — мьютекс единственного экземпляра. Обязательно DoNotQueue:
        // без него занятое имя ставит нас В ОЧЕРЕДЬ (builder.name() так и
        // делает), «второй» демон молча поднимался и перехватывал KWin-скрипт
        // у первого — проверено живьём. Не PrimaryOwner => чистая деградация
        // в null-провайдер уровнем выше.
        let reply = conn
            .request_name_with_flags(BUS_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())
            .context("имя шины не запросилось")?;
        if reply != zbus::fdo::RequestNameReply::PrimaryOwner {
            bail!("имя шины занято (driftling уже запущен?)");
        }

        // Требование detect(): KWin должен присутствовать на шине.
        let kwin_present = zbus::blocking::fdo::DBusProxy::new(&conn)?
            .name_has_owner(zbus::names::BusName::try_from(KWIN_SERVICE)?)?;
        if !kwin_present {
            bail!("{KWIN_SERVICE} отсутствует на шине");
        }

        let mut sense = Self {
            conn,
            shared,
            script_file: None,
            load_seq: 0,
            last_probe: None,
        };
        if let Err(e) = sense.load_script() {
            // Не фатально: например, KWin занят — перезагрузим из latest().
            log::warn!("worldsense: KWin-скрипт не загрузился ({e}), попробуем позже");
        }
        Ok(sense)
    }

    fn scripting_proxy(&self) -> zbus::Result<Proxy<'static>> {
        Proxy::new(&self.conn, KWIN_SERVICE, SCRIPTING_PATH, SCRIPTING_IFACE)
    }

    fn is_script_loaded(&self) -> anyhow::Result<bool> {
        let loaded: bool = self
            .scripting_proxy()?
            .call("isScriptLoaded", &(PLUGIN_NAME,))?;
        Ok(loaded)
    }

    /// Выгрузка нашего скрипта из KWin (по имени плагина) и удаление файла.
    /// Все ошибки глотаются: выгружать может быть уже нечего.
    fn unload_script(&mut self) {
        if let Ok(proxy) = self.scripting_proxy() {
            let _ = proxy.call::<_, _, bool>("unloadScript", &(PLUGIN_NAME,));
        }
        if let Some(path) = self.script_file.take() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Полный цикл (пере)загрузки: выгрузить старое, записать файл, загрузить,
    /// запустить. Идемпотентно; сироты от упавшего процесса тоже сносятся,
    /// потому что unload идёт по имени плагина.
    fn load_script(&mut self) -> anyhow::Result<()> {
        self.unload_script();

        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        self.load_seq = self.load_seq.wrapping_add(1);
        let path = dir.join(format!(
            "driftling-worldsense-{}-{}.qml",
            std::process::id(),
            self.load_seq
        ));
        std::fs::write(&path, SENSE_QML)
            .with_context(|| format!("не записался файл скрипта {}", path.display()))?;
        self.script_file = Some(path.clone());

        let path_str = path.to_str().context("не-UTF-8 путь к файлу скрипта")?;
        let scripting = self.scripting_proxy()?;
        let id: i32 = scripting
            .call("loadDeclarativeScript", &(path_str, PLUGIN_NAME))
            .context("loadDeclarativeScript")?;
        if id < 0 {
            bail!("loadDeclarativeScript вернул {id}");
        }
        // Запускаем именно наш скрипт, а не глобальный start() всех подряд.
        let script_path = format!("{SCRIPTING_PATH}/Script{id}");
        let run = Proxy::new(&self.conn, KWIN_SERVICE, script_path.as_str(), SCRIPT_IFACE)
            .and_then(|p| p.call::<_, _, ()>("run", &()));
        if let Err(e) = run {
            // Недозапущенный скрипт не оставляем: иначе isScriptLoaded вечно
            // говорил бы «жив» про мёртвого.
            self.unload_script();
            return Err(e).context("запуск скрипта");
        }
        log::debug!("worldsense: KWin-скрипт загружен (id {id})");
        Ok(())
    }
}

impl WorldSense for KWinSense {
    fn latest(&mut self) -> Option<WorldSnapshot> {
        let (snap, at) = {
            let s = lock(&self.shared);
            (s.snap.clone(), s.at)
        };
        // Обычный путь: данные свежие.
        if let Some(t) = at {
            if t.elapsed() < STALE_AFTER {
                return snap;
            }
        }
        // Тишина. Не чаще раза в PROBE_BACKOFF выясняем почему; в остальное
        // время отдаём что есть (потухший снапшот лучше пустоты на пару секунд).
        if let Some(t) = self.last_probe {
            if t.elapsed() < PROBE_BACKOFF {
                return snap;
            }
        }
        self.last_probe = Some(Instant::now());
        match self.is_script_loaded() {
            // Скрипт на месте: тихий рабочий стол — это норма.
            Ok(true) => snap,
            Ok(false) => {
                // Рестарт KWin: окна могли переехать — старый снапшот врёт.
                log::info!("worldsense: KWin-скрипт пропал (рестарт KWin?) — перезагрузка");
                let mut s = lock(&self.shared);
                s.snap = None;
                s.at = None;
                drop(s);
                if let Err(e) = self.load_script() {
                    log::debug!("worldsense: перезагрузка скрипта не удалась: {e}");
                }
                None
            }
            Err(e) => {
                log::debug!("worldsense: KWin недоступен: {e}");
                let mut s = lock(&self.shared);
                s.snap = None;
                s.at = None;
                None
            }
        }
    }
}

impl Drop for KWinSense {
    fn drop(&mut self) {
        // Вежливость: не оставляем скрипт в чужом композиторе.
        self.unload_script();
    }
}
