//! Hyprland-провайдер (D3): снапшоты «рельефа» через IPC-сокеты Hyprland.
//!
//! Сокеты лежат в `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/`:
//!  - `.socket.sock` — запрос-ответ (`j/clients`, `j/monitors` — JSON);
//!  - `.socket2.sock` — поток событий строками `СОБЫТИЕ>>ДАННЫЕ\n`.
//!
//! ВАЖНО: запросный сокет НИКОГДА не переиспользуем — Hyprland рассчитан на
//! «одно соединение = один запрос», удержание соединения подвешивает его IPC.
//! Каждый refetch = два свежих коннекта (clients + monitors).
//!
//! Схема — как у sway.rs: событие → коалесценция 50 мс → refetch → снапшот
//! в мьютексе. Рестарт Hyprland меняет сигнатуру инстанса, старые сокеты
//! умирают — провайдер помечает себя мёртвым и отдаёт `None`.
//!
//! Пол (`workspace_bottom`): низ зарезервированной области сфокусированного
//! монитора (`reserved` = [слева, сверху, справа, снизу] — там живут бары).
//! Если снизу ничего не зарезервировано — `None` (пол = низ экрана, то же
//! самое). Координаты окон у Hyprland уже логические; размер монитора —
//! физический, делим на scale (трансформации 90° не учитываем — D6).

use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use driftling_core::Rect;
use serde::Deserialize;

use crate::parse::{fnv1a64, is_own_window, MIN_PLATFORM_SIZE};
use crate::{WindowPlatform, WorldSense, WorldSnapshot};

/// Коалесценция шквала событий (тот же ритм, что у kwin/sway).
const COALESCE: Duration = Duration::from_millis(50);
/// Таймаут одиночного запроса.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// Строка событий длиннее этого без `\n` — мусор на сокете.
const MAX_LINE: usize = 1024 * 1024;

/// События, после которых рельеф мог измениться. Сверяем по префиксу:
/// покрывает и v2-варианты (`movewindowv2`, `workspacev2`, ...).
const RELEVANT_EVENTS: &[&str] = &[
    "openwindow",
    "closewindow",
    "movewindow",
    "resizewindow", // на будущее: сейчас Hyprland его не шлёт, но вдруг
    "fullscreen",
    "workspace",
    "focusedmon",
    "changefloatingmode",
    "monitoradded",
    "monitorremoved",
];

/// `СОБЫТИЕ>>ДАННЫЕ` → менять ли снапшот.
fn is_relevant_event(line: &str) -> bool {
    let name = line.split(">>").next().unwrap_or("");
    RELEVANT_EVENTS.iter().any(|e| name.starts_with(e))
}

// ---------------------------------------------------------------------------
// Разбор ответов (чистая часть)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawWsRef {
    id: i64,
}

/// Клиент из `j/clients`. Лишние поля игнорируются, недостающие — по
/// умолчанию: Hyprland меняет схему от версии к версии.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawClient {
    address: String,
    at: [f32; 2],
    size: [f32; 2],
    mapped: bool,
    hidden: bool,
    class: String,
    workspace: RawWsRef,
    /// В старых версиях bool, в новых — битовая маска (2 = фулскрин,
    /// 1 = максимизация). Разбираем оба варианта.
    fullscreen: serde_json::Value,
    /// 0 = самое недавно сфокусированное (считаем самым верхним).
    #[serde(rename = "focusHistoryID")]
    focus_history_id: Option<i64>,
}

impl RawClient {
    /// Настоящий фулскрин (максимизация — не повод прятаться).
    fn is_fullscreen(&self) -> bool {
        match &self.fullscreen {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(0) & 2 != 0,
            _ => false,
        }
    }

    /// Стабильный id: адрес окна — hex-строка вида `0x55a1b2...`.
    fn stable_id(&self) -> u64 {
        self.address
            .strip_prefix("0x")
            .and_then(|h| u64::from_str_radix(h, 16).ok())
            .unwrap_or_else(|| fnv1a64(&self.address))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMonitor {
    x: f32,
    y: f32,
    /// Физические пиксели; логическая высота = height / scale.
    width: f32,
    height: f32,
    scale: f32,
    focused: bool,
    #[serde(rename = "activeWorkspace")]
    active_workspace: RawWsRef,
    /// Активный спец-воркспейс (скретчпад); id 0 = нет.
    #[serde(rename = "specialWorkspace")]
    special_workspace: RawWsRef,
    /// Зарезервированные краевые зоны: [слева, сверху, справа, снизу].
    reserved: Vec<f32>,
}

/// JSON `j/clients` + `j/monitors` → снапшот. `None` — мусор на входе.
fn build_snapshot(clients_json: &str, monitors_json: &str) -> Option<WorldSnapshot> {
    let clients: Vec<RawClient> = serde_json::from_str(clients_json).ok()?;
    let monitors: Vec<RawMonitor> = serde_json::from_str(monitors_json).ok()?;

    // Видимые воркспейсы: активные на каждом мониторе + открытые спец.
    let active: HashSet<i64> = monitors
        .iter()
        .flat_map(|m| {
            let special = (m.special_workspace.id != 0).then_some(m.special_workspace.id);
            std::iter::once(m.active_workspace.id).chain(special)
        })
        .collect();

    let mut visible: Vec<&RawClient> = clients
        .iter()
        .filter(|c| c.mapped && !c.hidden && active.contains(&c.workspace.id))
        .collect();
    // 0 = в фокусе = наверху; без поля — в конец (низ стопки).
    visible.sort_by_key(|c| c.focus_history_id.unwrap_or(i64::MAX));

    let fullscreen_active = visible.iter().any(|c| c.is_fullscreen());

    let platforms: Vec<WindowPlatform> = visible
        .iter()
        .filter(|c| !is_own_window(&c.class))
        .filter(|c| c.size[0] >= MIN_PLATFORM_SIZE && c.size[1] >= MIN_PLATFORM_SIZE)
        .map(|c| WindowPlatform {
            rect: Rect::new(c.at[0], c.at[1], c.size[0], c.size[1]),
            id: c.stable_id(),
        })
        .collect();

    let workspace_bottom = monitors.iter().find(|m| m.focused).and_then(|m| {
        let bottom_reserve = *m.reserved.get(3)?;
        if bottom_reserve <= 0.0 {
            return None; // ничего не зарезервировано: пол = низ экрана и так
        }
        let scale = if m.scale > 0.0 { m.scale } else { 1.0 };
        Some(m.y + m.height / scale - bottom_reserve)
    });

    Some(WorldSnapshot {
        platforms,
        workspace_bottom,
        // Hyprland отдаёт резерв баров одним числом на монитор — разбивки
        // по выходам нет, демон возьмёт workspace_bottom.
        screen_areas: Vec::new(),
        fullscreen_active,
    })
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

/// Каталог сокетов текущего инстанса Hyprland.
fn socket_dir() -> anyhow::Result<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE не задана")?;
    if sig.is_empty() {
        bail!("HYPRLAND_INSTANCE_SIGNATURE пуста");
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR не задан")?;
    Ok(PathBuf::from(runtime).join("hypr").join(sig))
}

/// Один запрос = одно свежее соединение (см. предупреждение в шапке файла).
fn request(dir: &Path, cmd: &str) -> anyhow::Result<String> {
    let path = dir.join(".socket.sock");
    let mut s = UnixStream::connect(&path)
        .with_context(|| format!("нет соединения с {}", path.display()))?;
    s.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    s.write_all(cmd.as_bytes()).context("отправка запроса")?;
    let mut out = String::new();
    s.read_to_string(&mut out)
        .with_context(|| format!("чтение ответа на {cmd}"))?;
    Ok(out)
}

/// Полный refetch мира.
fn fetch_snapshot(dir: &Path) -> anyhow::Result<Option<WorldSnapshot>> {
    let clients = request(dir, "j/clients")?;
    let monitors = request(dir, "j/monitors")?;
    Ok(build_snapshot(&clients, &monitors))
}

// ---------------------------------------------------------------------------
// Провайдер
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    snap: Option<WorldSnapshot>,
    dead: bool,
}

/// Мьютекс без паник (паттерн kwin.rs).
fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mark_dead(shared: &Mutex<Shared>) {
    let mut s = lock(shared);
    s.dead = true;
    s.snap = None;
}

pub(crate) struct HyprlandSense {
    shared: Arc<Mutex<Shared>>,
    /// Клон событийного сокета — shutdown в Drop будит фоновый тред.
    events: UnixStream,
    death_logged: bool,
}

impl HyprlandSense {
    /// Снимает первый снапшот, подключается к потоку событий и запускает
    /// фоновый тред. Ошибка — сокеты инстанса недоступны; уровнем выше это
    /// чистая деградация в null.
    pub(crate) fn new() -> anyhow::Result<Self> {
        let dir = socket_dir()?;
        let first = fetch_snapshot(&dir)?;
        if first.is_none() {
            bail!("j/clients или j/monitors вернули неразборчивый JSON");
        }
        let events =
            UnixStream::connect(dir.join(".socket2.sock")).context("сокет событий не открылся")?;
        let shared = Arc::new(Mutex::new(Shared {
            snap: first,
            dead: false,
        }));
        let events_clone = events.try_clone().context("клон сокета событий")?;
        {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("worldsense-hyprland".into())
                .spawn(move || event_loop(events, dir, &shared))
                .context("тред событий не запустился")?;
        }
        Ok(Self {
            shared,
            events: events_clone,
            death_logged: false,
        })
    }
}

/// Фоновый цикл: строки событий → коалесценция 50 мс → refetch.
fn event_loop(mut events: UnixStream, dir: PathBuf, shared: &Mutex<Shared>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut pending: Option<Instant> = None;
    loop {
        if events.set_read_timeout(pending.map(|_| COALESCE)).is_err() {
            break;
        }
        match events.read(&mut chunk) {
            Ok(0) => break, // сокет закрыт: Hyprland ушёл
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                // Разбираем целые строки; хвост без \n остаётся в буфере.
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                    if is_relevant_event(&line) {
                        pending.get_or_insert_with(Instant::now);
                    }
                }
                if buf.len() > MAX_LINE {
                    log::warn!("worldsense/hyprland: строка событий без конца — рассинхрон");
                    mark_dead(shared);
                    return;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        if let Some(t) = pending {
            if t.elapsed() >= COALESCE {
                pending = None;
                match fetch_snapshot(&dir) {
                    Ok(Some(snap)) => lock(shared).snap = Some(snap),
                    // Разовый битый JSON — оставляем прошлый снапшот.
                    Ok(None) => log::debug!("worldsense/hyprland: битый ответ — пропуск"),
                    Err(e) => {
                        log::info!("worldsense/hyprland: refetch не прошёл ({e}) — провайдер умер");
                        mark_dead(shared);
                        return;
                    }
                }
            }
        }
    }
    log::info!("worldsense/hyprland: сокет событий закрыт — деградация в None");
    mark_dead(shared);
}

impl WorldSense for HyprlandSense {
    fn latest(&mut self) -> Option<WorldSnapshot> {
        let (snap, dead) = {
            let s = lock(&self.shared);
            (s.snap.clone(), s.dead)
        };
        if dead && !self.death_logged {
            self.death_logged = true;
            log::warn!(
                "worldsense/hyprland: провайдер мёртв (рестарт Hyprland?) — пол = низ экрана"
            );
        }
        if dead {
            None
        } else {
            snap
        }
    }
}

impl Drop for HyprlandSense {
    fn drop(&mut self) {
        let _ = self.events.shutdown(std::net::Shutdown::Both);
    }
}

// ---------------------------------------------------------------------------
// Тесты (фикстуры по мотивам `hyprctl -j clients` / `-j monitors`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Два монитора; на первом (focused) активен ws 1 и бар снизу 30 px,
    /// на втором — ws 3 без баров.
    const MONITORS: &str = r#"[
      {"id": 0, "name": "DP-1", "x": 0, "y": 0, "width": 2560, "height": 1440,
       "scale": 1.0, "focused": true,
       "activeWorkspace": {"id": 1, "name": "1"},
       "specialWorkspace": {"id": 0, "name": ""},
       "reserved": [0, 0, 0, 30]},
      {"id": 1, "name": "HDMI-A-1", "x": 2560, "y": 0, "width": 1920, "height": 1080,
       "scale": 1.0, "focused": false,
       "activeWorkspace": {"id": 3, "name": "3"},
       "specialWorkspace": {"id": 0, "name": ""},
       "reserved": [0, 0, 0, 0]}
    ]"#;

    /// Терминал и браузер на ws1 (браузер в фокусе), редактор на невидимом
    /// ws2, окно на ws3 второго монитора, скрытое (grouped), немапнутое,
    /// своё окно driftling и окно-точка.
    const CLIENTS: &str = r#"[
      {"address": "0x55d1aa0010", "at": [0, 0], "size": [1280, 1410],
       "mapped": true, "hidden": false, "floating": false, "class": "foot",
       "workspace": {"id": 1, "name": "1"}, "fullscreen": false,
       "focusHistoryID": 1},
      {"address": "0x55d1aa0020", "at": [1280, 0], "size": [1280, 1410],
       "mapped": true, "hidden": false, "floating": false, "class": "firefox",
       "workspace": {"id": 1, "name": "1"}, "fullscreen": false,
       "focusHistoryID": 0},
      {"address": "0x55d1aa0030", "at": [0, 0], "size": [2560, 1440],
       "mapped": true, "hidden": false, "floating": false, "class": "code",
       "workspace": {"id": 2, "name": "2"}, "fullscreen": false,
       "focusHistoryID": 2},
      {"address": "0x55d1aa0040", "at": [2560, 0], "size": [1920, 1080],
       "mapped": true, "hidden": false, "floating": false, "class": "mpv",
       "workspace": {"id": 3, "name": "3"}, "fullscreen": false,
       "focusHistoryID": 3},
      {"address": "0x55d1aa0050", "at": [100, 100], "size": [800, 600],
       "mapped": true, "hidden": true, "floating": false, "class": "kitty",
       "workspace": {"id": 1, "name": "1"}, "fullscreen": false,
       "focusHistoryID": 4},
      {"address": "0x55d1aa0060", "at": [0, 0], "size": [500, 400],
       "mapped": false, "hidden": false, "floating": true, "class": "chromium",
       "workspace": {"id": 1, "name": "1"}, "fullscreen": false,
       "focusHistoryID": 5},
      {"address": "0x55d1aa0070", "at": [300, 300], "size": [700, 500],
       "mapped": true, "hidden": false, "floating": true,
       "class": "io.github.nanitll.Driftling",
       "workspace": {"id": 1, "name": "1"}, "fullscreen": false,
       "focusHistoryID": 6},
      {"address": "0x55d1aa0080", "at": [46, 99], "size": [1, 1],
       "mapped": true, "hidden": false, "floating": true,
       "class": "xwaylandvideobridge",
       "workspace": {"id": 1, "name": "1"}, "fullscreen": false,
       "focusHistoryID": 7}
    ]"#;

    #[test]
    fn otbor_poryadok_i_pol() {
        let snap = build_snapshot(CLIENTS, MONITORS).expect("валидный JSON");
        // Видимые платформы: firefox (фокус, наверху), foot, mpv.
        // ws2, hidden, unmapped, своё окно и 1×1 — вон.
        assert_eq!(snap.platforms.len(), 3);
        assert_eq!(snap.platforms[0].id, 0x55d1aa0020);
        assert_eq!(snap.platforms[1].id, 0x55d1aa0010);
        assert_eq!(snap.platforms[2].id, 0x55d1aa0040);
        // Геометрия как есть (логические координаты Hyprland).
        let ff = &snap.platforms[0];
        assert_eq!(
            (ff.rect.x, ff.rect.y, ff.rect.w, ff.rect.h),
            (1280.0, 0.0, 1280.0, 1410.0)
        );
        // Пол: низ сфокусированного монитора минус бар (1440 - 30).
        assert_eq!(snap.workspace_bottom, Some(1410.0));
        assert!(!snap.fullscreen_active);
    }

    #[test]
    fn pol_s_drobnym_masshtabom() {
        // scale 2: логическая высота 1440/2 = 720, минус бар 24.
        let monitors = r#"[{"id":0,"x":0,"y":0,"width":2560,"height":1440,"scale":2.0,
            "focused":true,"activeWorkspace":{"id":1},"specialWorkspace":{"id":0},
            "reserved":[0,0,0,24]}]"#;
        let snap = build_snapshot("[]", monitors).unwrap();
        assert_eq!(snap.workspace_bottom, Some(696.0));
    }

    #[test]
    fn bez_rezerva_pol_neizvesten() {
        let monitors = r#"[{"id":0,"x":0,"y":0,"width":1920,"height":1080,"scale":1.0,
            "focused":true,"activeWorkspace":{"id":1},"specialWorkspace":{"id":0},
            "reserved":[0,0,0,0]}]"#;
        let snap = build_snapshot("[]", monitors).unwrap();
        assert_eq!(snap.workspace_bottom, None);
    }

    #[test]
    fn fullscreen_bool_i_maska() {
        let monitors = r#"[{"id":0,"focused":true,"activeWorkspace":{"id":1},
            "specialWorkspace":{"id":0},"reserved":[0,0,0,0],
            "x":0,"y":0,"width":1920,"height":1080,"scale":1.0}]"#;
        // Старая схема: bool.
        let clients = r#"[{"address":"0x1","at":[0,0],"size":[1920,1080],"mapped":true,
            "hidden":false,"class":"mpv","workspace":{"id":1},"fullscreen":true}]"#;
        assert!(build_snapshot(clients, monitors).unwrap().fullscreen_active);
        // Новая схема: маска, 2 = фулскрин.
        let clients = r#"[{"address":"0x1","at":[0,0],"size":[1920,1080],"mapped":true,
            "hidden":false,"class":"mpv","workspace":{"id":1},"fullscreen":2}]"#;
        assert!(build_snapshot(clients, monitors).unwrap().fullscreen_active);
        // 1 = только максимизация — не прячемся.
        let clients = r#"[{"address":"0x1","at":[0,0],"size":[1920,1080],"mapped":true,
            "hidden":false,"class":"mpv","workspace":{"id":1},"fullscreen":1}]"#;
        assert!(!build_snapshot(clients, monitors).unwrap().fullscreen_active);
        // Фулскрин на неактивном воркспейсе — не считается.
        let clients = r#"[{"address":"0x1","at":[0,0],"size":[1920,1080],"mapped":true,
            "hidden":false,"class":"mpv","workspace":{"id":9},"fullscreen":2}]"#;
        assert!(!build_snapshot(clients, monitors).unwrap().fullscreen_active);
    }

    #[test]
    fn specialnyy_workspace_vidim() {
        let monitors = r#"[{"id":0,"focused":true,"activeWorkspace":{"id":1},
            "specialWorkspace":{"id":-98},"reserved":[0,0,0,0],
            "x":0,"y":0,"width":1920,"height":1080,"scale":1.0}]"#;
        let clients = r#"[{"address":"0xaa","at":[200,200],"size":[800,500],"mapped":true,
            "hidden":false,"class":"foot","workspace":{"id":-98},"fullscreen":false}]"#;
        let snap = build_snapshot(clients, monitors).unwrap();
        assert_eq!(snap.platforms.len(), 1);
        assert_eq!(snap.platforms[0].id, 0xaa);
    }

    #[test]
    fn krivoy_adres_hesh_folbek() {
        let c = RawClient {
            address: "странный-адрес".into(),
            ..RawClient::default()
        };
        assert_eq!(c.stable_id(), fnv1a64("странный-адрес"));
        let c = RawClient {
            address: "0xff".into(),
            ..RawClient::default()
        };
        assert_eq!(c.stable_id(), 255);
    }

    #[test]
    fn sobytiya_filtruyutsya_po_prefiksu() {
        assert!(is_relevant_event("openwindow>>0x1,1,foot,foot"));
        assert!(is_relevant_event("movewindowv2>>0x1,1,1"));
        assert!(is_relevant_event("workspacev2>>2,2"));
        assert!(is_relevant_event("fullscreen>>1"));
        assert!(!is_relevant_event("activewindow>>foot,~"));
        assert!(!is_relevant_event("windowtitle>>0x1"));
        assert!(!is_relevant_event(""));
    }

    #[test]
    fn musor_na_vhode_daet_none() {
        assert!(build_snapshot("не json", "[]").is_none());
        assert!(build_snapshot("[]", "не json").is_none());
        assert!(build_snapshot("{}", "[]").is_none());
    }
}
