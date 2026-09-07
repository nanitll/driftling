//! sway-провайдер (D3): снапшоты «рельефа» через i3-IPC.
//!
//! Схема:
//!  1. сокет из `$SWAYSOCK`, бинарная рамка i3-ipc: магия `"i3-ipc"` +
//!     u32 длина + u32 тип (порядок байт — хостовый, по спецификации i3);
//!  2. при старте — синхронный `GET_TREE` (заодно проверка, что сокет живой);
//!  3. отдельное соединение подписывается (`SUBSCRIBE ["window","workspace"]`),
//!     фоновый тред копит события с коалесценцией 50 мс и перечитывает дерево
//!     свежим соединением; готовый снапшот лежит в мьютексе (паттерн kwin.rs);
//!  4. рестарт sway = сокет закрылся (а `$SWAYSOCK` нового инстанса другой) —
//!     провайдер помечает себя мёртвым и дальше честно отдаёт `None`.
//!
//! О геометрии: у окна берём `rect` — прямоугольник с рамками, но БЕЗ
//! заголовка (`deco_rect` — это заголовок над `rect`). То есть питомец стоит
//! на верхней кромке содержимого, чуть ниже видимого верха окна с титлбаром.
//! Осознанное упрощение: у sway титлбары тонкие и чаще выключены.
//!
//! Разбор дерева — чистые функции, юнит-тесты на фикстурах ниже.

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

/// Магия протокола i3-IPC.
const MAGIC: &[u8; 6] = b"i3-ipc";
/// Заголовок рамки: магия + длина + тип.
const HEADER: usize = 14;
/// Типы сообщений i3-IPC.
const MSG_SUBSCRIBE: u32 = 2;
const MSG_GET_TREE: u32 = 4;
/// У событий взведён старший бит типа.
const EVENT_BIT: u32 = 0x8000_0000;
/// Коалесценция шквала событий: перечитываем дерево не раньше, чем через
/// столько после первого события пачки.
const COALESCE: Duration = Duration::from_millis(50);
/// Кадр больше этого — мусор или рассинхрон (реальное дерево — десятки КБ).
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// Таймаут одиночного запроса-ответа (локальный сокет, щедро).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Рамки i3-IPC
// ---------------------------------------------------------------------------

/// Собирает рамку сообщения.
fn encode_frame(msg_type: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_ne_bytes(),
    );
    out.extend_from_slice(&msg_type.to_ne_bytes());
    out.extend_from_slice(payload);
    out
}

/// Инкрементальный декодер рамок: копит байты, отдаёт целые сообщения.
/// Нужен, потому что тред событий читает с таймаутами и может получить
/// рамку кусками.
#[derive(Default)]
struct FrameBuf {
    buf: Vec<u8>,
}

impl FrameBuf {
    fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Следующая целая рамка: `(тип, payload)`. `Ok(None)` — данных пока мало.
    /// `Err` — рассинхрон, соединению доверять больше нельзя.
    fn next_frame(&mut self) -> anyhow::Result<Option<(u32, Vec<u8>)>> {
        if self.buf.len() < HEADER {
            return Ok(None);
        }
        if &self.buf[..MAGIC.len()] != MAGIC {
            bail!("рассинхрон i3-ipc: нет магии в начале рамки");
        }
        let len = u32::from_ne_bytes(self.buf[6..10].try_into().expect("4 байта")) as usize;
        if len > MAX_FRAME {
            bail!("рамка i3-ipc {len} байт — похоже на мусор");
        }
        if self.buf.len() < HEADER + len {
            return Ok(None);
        }
        let msg_type = u32::from_ne_bytes(self.buf[10..14].try_into().expect("4 байта"));
        let payload = self.buf[HEADER..HEADER + len].to_vec();
        self.buf.drain(..HEADER + len);
        Ok(Some((msg_type, payload)))
    }
}

/// Синхронный запрос-ответ на СВЕЖЕМ соединении (события сюда не приходят,
/// поэтому первая же рамка нужного типа — наш ответ).
fn request(sock: &Path, msg_type: u32, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut s = UnixStream::connect(sock)
        .with_context(|| format!("нет соединения с {}", sock.display()))?;
    s.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    s.write_all(&encode_frame(msg_type, payload))?;
    let mut fb = FrameBuf::default();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = s.read(&mut chunk).context("чтение ответа i3-ipc")?;
        if n == 0 {
            bail!("сокет закрыт до ответа");
        }
        fb.feed(&chunk[..n]);
        while let Some((t, body)) = fb.next_frame()? {
            if t == msg_type {
                return Ok(body);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Разбор дерева (чистая часть)
// ---------------------------------------------------------------------------

/// Прямоугольник из JSON sway (`rect`); целые числа serde сам приводит к f32.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default)]
struct RawRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawWindowProps {
    /// Класс XWayland-окна (у Wayland-окон вместо него `app_id`).
    class: Option<String>,
}

/// Узел дерева sway. Лишние поля игнорируются, недостающие — по умолчанию:
/// sway и демон могут обновляться не в ногу.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Node {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    rect: RawRect,
    /// Есть только у оконных узлов: окно видно на экране прямо сейчас
    /// (текущий рабочий стол, не перекрыто фулскрином, не в скретчпаде).
    visible: Option<bool>,
    focused: bool,
    fullscreen_mode: Option<u8>,
    app_id: Option<String>,
    pid: Option<i64>,
    /// X11 id — признак XWayland-окна.
    window: Option<i64>,
    window_properties: Option<RawWindowProps>,
    nodes: Vec<Node>,
    floating_nodes: Vec<Node>,
}

impl Node {
    /// Листовой оконный контейнер (а не split-контейнер/воркспейс/выход).
    fn is_window(&self) -> bool {
        (self.kind == "con" || self.kind == "floating_con")
            && self.nodes.is_empty()
            && (self.app_id.is_some() || self.window.is_some() || self.pid.is_some())
    }

    /// Класс окна для фильтра собственных окон driftling.
    fn class(&self) -> &str {
        self.app_id
            .as_deref()
            .or_else(|| {
                self.window_properties
                    .as_ref()
                    .and_then(|p| p.class.as_deref())
            })
            .unwrap_or("")
    }
}

/// JSON `GET_TREE` → снапшот. `None` — мусор на входе.
fn snapshot_from_tree(json: &str) -> Option<WorldSnapshot> {
    let root: Node = serde_json::from_str(json).ok()?;
    Some(build_snapshot(&root))
}

/// Дерево → снапшот: видимые окна, пол из рабочей области, фулскрин.
fn build_snapshot(root: &Node) -> WorldSnapshot {
    let mut floating = Vec::new();
    let mut tiled = Vec::new();
    let mut fullscreen_active = false;
    collect_windows(
        root,
        false,
        &mut tiled,
        &mut floating,
        &mut fullscreen_active,
    );
    // Порядок «сверху вниз» упрощённый: плавающие окна всегда выше тайловых,
    // а тайловые между собой не пересекаются, так что их порядок не важен.
    let mut platforms = floating;
    platforms.append(&mut tiled);
    // Рабочая область воркспейса уже за вычетом баров: её низ = верх
    // нижнего swaybar (или низ экрана, если бара нет — тоже честный пол).
    let workspace_bottom = current_workspace(root).map(|ws| ws.rect.y + ws.rect.height);
    WorldSnapshot {
        platforms,
        workspace_bottom,
        // sway отдаёт прямоугольник текущего воркспейса; разбивки рабочих
        // областей по выходам тут нет — демон обойдётся workspace_bottom.
        screen_areas: Vec::new(),
        fullscreen_active,
    }
}

/// Рекурсивный сбор видимых окон; `in_floating` — узел из `floating_nodes`.
fn collect_windows(
    n: &Node,
    in_floating: bool,
    tiled: &mut Vec<WindowPlatform>,
    floating: &mut Vec<WindowPlatform>,
    fullscreen_active: &mut bool,
) {
    if n.is_window() && n.visible == Some(true) {
        if n.fullscreen_mode.unwrap_or(0) > 0 {
            *fullscreen_active = true;
        }
        if !is_own_window(n.class())
            && n.rect.width >= MIN_PLATFORM_SIZE
            && n.rect.height >= MIN_PLATFORM_SIZE
        {
            let platform = WindowPlatform {
                rect: Rect::new(n.rect.x, n.rect.y, n.rect.width, n.rect.height),
                // id контейнера уникален в пределах жизни sway; хэшируем вместе
                // с типом источника, чтобы не пересекаться со схемами других
                // провайдеров при смене окружения.
                id: fnv1a64(&format!("sway:{}", n.id)),
            };
            if in_floating {
                floating.push(platform);
            } else {
                tiled.push(platform);
            }
        }
    }
    for c in &n.nodes {
        collect_windows(c, in_floating, tiled, floating, fullscreen_active);
    }
    for c in &n.floating_nodes {
        collect_windows(c, true, tiled, floating, fullscreen_active);
    }
}

/// Текущий воркспейс: тот, в чьём поддереве фокус; фолбэк — первый воркспейс
/// с видимыми окнами (фокус мог уехать в бар/лаунчер).
fn current_workspace(root: &Node) -> Option<&Node> {
    fn dfs<'a>(n: &'a Node, ws: Option<&'a Node>) -> Option<&'a Node> {
        let ws = if n.kind == "workspace" { Some(n) } else { ws };
        if n.focused {
            return ws;
        }
        n.nodes
            .iter()
            .chain(n.floating_nodes.iter())
            .find_map(|c| dfs(c, ws))
    }
    dfs(root, None).or_else(|| first_visible_workspace(root))
}

fn first_visible_workspace(n: &Node) -> Option<&Node> {
    // "__i3_scratch" (скретчпад) — не настоящий воркспейс.
    if n.kind == "workspace"
        && !n.name.as_deref().unwrap_or("").starts_with("__i3")
        && has_visible(n)
    {
        return Some(n);
    }
    n.nodes
        .iter()
        .chain(n.floating_nodes.iter())
        .find_map(first_visible_workspace)
}

fn has_visible(n: &Node) -> bool {
    n.visible == Some(true)
        || n.nodes.iter().any(has_visible)
        || n.floating_nodes.iter().any(has_visible)
}

// ---------------------------------------------------------------------------
// Провайдер
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    snap: Option<WorldSnapshot>,
    dead: bool,
}

/// Мьютекс без паник (паттерн kwin.rs): отравление не роняет физику.
fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mark_dead(shared: &Mutex<Shared>) {
    let mut s = lock(shared);
    s.dead = true;
    // Мёртвый провайдер не знает, где окна: старый снапшот врёт.
    s.snap = None;
}

pub(crate) struct SwaySense {
    shared: Arc<Mutex<Shared>>,
    /// Клон событийного сокета — shutdown в Drop будит фоновый тред.
    events: UnixStream,
    /// Смерть уже залогирована (чтобы не спамить из latest()).
    death_logged: bool,
}

impl SwaySense {
    /// Подключается к `$SWAYSOCK`, снимает первый снапшот, подписывается на
    /// события и запускает фоновый тред. Ошибка — сокет недоступен или это
    /// не i3-IPC; уровнем выше это чистая деградация в null.
    pub(crate) fn new() -> anyhow::Result<Self> {
        let sock = PathBuf::from(std::env::var_os("SWAYSOCK").context("SWAYSOCK не задан")?);

        // Первый снапшот синхронно: заодно проверка, что сокет отвечает.
        let tree = request(&sock, MSG_GET_TREE, b"")?;
        let first = snapshot_from_tree(std::str::from_utf8(&tree).unwrap_or(""));
        if first.is_none() {
            bail!("GET_TREE вернул неразборчивый JSON");
        }

        // Подписка — на отдельном долгоживущем соединении.
        let mut events = UnixStream::connect(&sock).context("сокет событий не открылся")?;
        events.set_read_timeout(Some(REQUEST_TIMEOUT))?;
        events.write_all(&encode_frame(MSG_SUBSCRIBE, br#"["window","workspace"]"#))?;
        let mut fb = FrameBuf::default();
        let mut chunk = [0u8; 4096];
        let ok = loop {
            let n = events.read(&mut chunk).context("ответ на подписку")?;
            if n == 0 {
                bail!("сокет закрыт при подписке");
            }
            fb.feed(&chunk[..n]);
            if let Some((t, body)) = fb.next_frame()? {
                if t != MSG_SUBSCRIBE {
                    bail!("вместо ответа на подписку пришёл тип {t}");
                }
                break String::from_utf8_lossy(&body).contains("\"success\":true")
                    || String::from_utf8_lossy(&body).contains("\"success\": true");
            }
        };
        if !ok {
            bail!("sway отказал в подписке на события");
        }
        events.set_read_timeout(None)?;

        let shared = Arc::new(Mutex::new(Shared {
            snap: first,
            dead: false,
        }));
        let events_clone = events.try_clone().context("клон сокета событий")?;
        {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("worldsense-sway".into())
                .spawn(move || event_loop(events, fb, sock, &shared))
                .context("тред событий не запустился")?;
        }
        Ok(Self {
            shared,
            events: events_clone,
            death_logged: false,
        })
    }
}

/// Фоновый цикл: события → коалесценция 50 мс → перечитать дерево.
fn event_loop(mut events: UnixStream, mut fb: FrameBuf, sock: PathBuf, shared: &Mutex<Shared>) {
    let mut chunk = [0u8; 64 * 1024];
    // Когда пришло первое событие несделанной пачки (дедлайн refetch = +50 мс).
    let mut pending: Option<Instant> = None;
    loop {
        // Пока копим пачку — читаем с коротким таймаутом, иначе спим на сокете.
        if events.set_read_timeout(pending.map(|_| COALESCE)).is_err() {
            break;
        }
        match events.read(&mut chunk) {
            Ok(0) => break, // сокет закрыт: sway ушёл (рестарт/выход)
            Ok(n) => {
                fb.feed(&chunk[..n]);
                loop {
                    match fb.next_frame() {
                        Ok(Some((t, _))) => {
                            // Любое событие подписки — повод перечитать мир.
                            if t & EVENT_BIT != 0 {
                                pending.get_or_insert_with(Instant::now);
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            log::warn!("worldsense/sway: {e}");
                            mark_dead(shared);
                            return;
                        }
                    }
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        if let Some(t) = pending {
            if t.elapsed() >= COALESCE {
                pending = None;
                // Свежее соединение на каждый запрос: не путаемся с событиями.
                match request(&sock, MSG_GET_TREE, b"") {
                    Ok(tree) => {
                        match snapshot_from_tree(std::str::from_utf8(&tree).unwrap_or("")) {
                            Some(snap) => lock(shared).snap = Some(snap),
                            // Разовый битый JSON — пропускаем, старый снапшот
                            // остаётся (лучше чуть устаревший, чем никакого).
                            None => log::debug!("worldsense/sway: битое дерево — пропуск"),
                        }
                    }
                    Err(e) => {
                        log::info!("worldsense/sway: GET_TREE не прошёл ({e}) — провайдер умер");
                        mark_dead(shared);
                        return;
                    }
                }
            }
        }
    }
    log::info!("worldsense/sway: сокет событий закрыт — деградация в None");
    mark_dead(shared);
}

impl WorldSense for SwaySense {
    fn latest(&mut self) -> Option<WorldSnapshot> {
        let (snap, dead) = {
            let s = lock(&self.shared);
            (s.snap.clone(), s.dead)
        };
        if dead && !self.death_logged {
            self.death_logged = true;
            log::warn!("worldsense/sway: провайдер мёртв (рестарт sway?) — пол = низ экрана");
        }
        if dead {
            None
        } else {
            snap
        }
    }
}

impl Drop for SwaySense {
    fn drop(&mut self) {
        // Будим фоновый тред: read вернёт 0, тред выйдет.
        let _ = self.events.shutdown(std::net::Shutdown::Both);
    }
}

// ---------------------------------------------------------------------------
// Тесты (фикстуры по мотивам реального `swaymsg -t get_tree`, sway 1.9/1.10)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Дерево: скретчпад с невидимым окном; выход 1920×1080 со swaybar снизу
    /// (воркспейс 0..1057); ws1 — два тайловых окна (одно XWayland) + одно
    /// плавающее + своё окно driftling + окно-точка; ws2 невидим целиком.
    const TREE: &str = r#"{
      "id": 1, "type": "root", "name": "root",
      "rect": {"x": 0, "y": 0, "width": 1920, "height": 1080},
      "nodes": [
        {"id": 2, "type": "output", "name": "__i3",
         "rect": {"x": 0, "y": 0, "width": 1920, "height": 1080},
         "nodes": [
           {"id": 3, "type": "workspace", "name": "__i3_scratch",
            "rect": {"x": 0, "y": 0, "width": 1920, "height": 1080},
            "nodes": [],
            "floating_nodes": [
              {"id": 20, "type": "floating_con", "name": "hidden thing",
               "app_id": "org.kde.dolphin", "pid": 500, "visible": false,
               "rect": {"x": 10, "y": 10, "width": 600, "height": 400}}
            ]}
         ]},
        {"id": 4, "type": "output", "name": "eDP-1",
         "rect": {"x": 0, "y": 0, "width": 1920, "height": 1080},
         "nodes": [
           {"id": 5, "type": "workspace", "name": "1",
            "rect": {"x": 0, "y": 0, "width": 1920, "height": 1057},
            "nodes": [
              {"id": 6, "type": "con", "name": null, "layout": "splith",
               "rect": {"x": 0, "y": 0, "width": 1920, "height": 1057},
               "nodes": [
                 {"id": 10, "type": "con", "name": "term", "app_id": "foot",
                  "pid": 100, "visible": true, "focused": true,
                  "fullscreen_mode": 0,
                  "rect": {"x": 0, "y": 23, "width": 960, "height": 1034},
                  "deco_rect": {"x": 0, "y": 0, "width": 960, "height": 23}},
                 {"id": 11, "type": "con", "name": "browser", "app_id": null,
                  "window": 4194305,
                  "window_properties": {"class": "Chromium", "instance": "chromium"},
                  "pid": 101, "visible": true, "focused": false,
                  "fullscreen_mode": 0,
                  "rect": {"x": 960, "y": 23, "width": 960, "height": 1034}},
                 {"id": 15, "type": "con", "name": "dot", "app_id": "xwaylandvideobridge",
                  "pid": 103, "visible": true,
                  "rect": {"x": 46, "y": 99, "width": 1, "height": 1}}
               ],
               "floating_nodes": []}
            ],
            "floating_nodes": [
              {"id": 12, "type": "floating_con", "name": "calc", "app_id": "org.gnome.Calculator",
               "pid": 102, "visible": true, "fullscreen_mode": 0,
               "rect": {"x": 500, "y": 300, "width": 400, "height": 500}},
              {"id": 13, "type": "floating_con", "name": "pet settings",
               "app_id": "io.github.nanitll.Driftling", "pid": 104, "visible": true,
               "rect": {"x": 100, "y": 100, "width": 700, "height": 500}}
            ]},
           {"id": 7, "type": "workspace", "name": "2",
            "rect": {"x": 0, "y": 0, "width": 1920, "height": 1057},
            "nodes": [
              {"id": 14, "type": "con", "name": "editor", "app_id": "code",
               "pid": 105, "visible": false,
               "rect": {"x": 0, "y": 23, "width": 1920, "height": 1034}}
            ],
            "floating_nodes": []}
         ]}
      ],
      "floating_nodes": []
    }"#;

    #[test]
    fn otbor_poryadok_i_pol() {
        let snap = snapshot_from_tree(TREE).expect("валидный JSON");
        // Видимые и не отсеянные: калькулятор (плавающий — первым), foot,
        // chromium. Скретчпад, ws2, своё окно и 1×1-точка — вон.
        assert_eq!(snap.platforms.len(), 3);
        assert_eq!(snap.platforms[0].id, fnv1a64("sway:12"));
        let tiled_ids: Vec<u64> = snap.platforms[1..].iter().map(|p| p.id).collect();
        assert!(tiled_ids.contains(&fnv1a64("sway:10")));
        assert!(tiled_ids.contains(&fnv1a64("sway:11")));
        // rect переносится как есть (рамки включены, титлбар — нет).
        let foot = snap
            .platforms
            .iter()
            .find(|p| p.id == fnv1a64("sway:10"))
            .unwrap();
        assert_eq!(
            (foot.rect.x, foot.rect.y, foot.rect.w, foot.rect.h),
            (0.0, 23.0, 960.0, 1034.0)
        );
        // Низ рабочей области воркспейса = верх нижнего swaybar.
        assert_eq!(snap.workspace_bottom, Some(1057.0));
        assert!(!snap.fullscreen_active);
    }

    #[test]
    fn fullscreen_iz_vidimogo_okna() {
        let s = r#"{"id":1,"type":"root","nodes":[{"id":2,"type":"output","name":"eDP-1","nodes":[
            {"id":3,"type":"workspace","name":"1","rect":{"x":0,"y":0,"width":1920,"height":1080},"nodes":[
              {"id":4,"type":"con","app_id":"mpv","pid":1,"visible":true,"focused":true,
               "fullscreen_mode":1,"rect":{"x":0,"y":0,"width":1920,"height":1080}}
            ]}]}]}"#;
        let snap = snapshot_from_tree(s).unwrap();
        assert!(snap.fullscreen_active);
        assert_eq!(snap.platforms.len(), 1);
    }

    #[test]
    fn nevidimyy_fullscreen_ne_schitaetsya() {
        // Фулскрин на другом воркспейсе (visible=false) — не прячемся.
        let s = r#"{"id":1,"type":"root","nodes":[{"id":2,"type":"output","name":"eDP-1","nodes":[
            {"id":3,"type":"workspace","name":"2","rect":{"x":0,"y":0,"width":1920,"height":1080},"nodes":[
              {"id":4,"type":"con","app_id":"mpv","pid":1,"visible":false,
               "fullscreen_mode":1,"rect":{"x":0,"y":0,"width":1920,"height":1080}}
            ]}]}]}"#;
        let snap = snapshot_from_tree(s).unwrap();
        assert!(!snap.fullscreen_active);
        assert!(snap.platforms.is_empty());
    }

    #[test]
    fn pol_bez_fokusa_beret_vidimyy_workspace() {
        // Фокус нигде (уехал в бар): фолбэк — воркспейс с видимыми окнами.
        let s = r#"{"id":1,"type":"root","nodes":[{"id":2,"type":"output","name":"eDP-1","nodes":[
            {"id":3,"type":"workspace","name":"1","rect":{"x":0,"y":0,"width":1920,"height":1050},"nodes":[
              {"id":4,"type":"con","app_id":"foot","pid":1,"visible":true,
               "rect":{"x":0,"y":0,"width":800,"height":600}}
            ]}]}]}"#;
        let snap = snapshot_from_tree(s).unwrap();
        assert_eq!(snap.workspace_bottom, Some(1050.0));
    }

    #[test]
    fn pustoe_derevo_bez_pola() {
        let snap = snapshot_from_tree(r#"{"id":1,"type":"root"}"#).unwrap();
        assert!(snap.platforms.is_empty());
        assert_eq!(snap.workspace_bottom, None);
    }

    #[test]
    fn musor_na_vhode_daet_none() {
        assert!(snapshot_from_tree("").is_none());
        assert!(snapshot_from_tree("не json").is_none());
        assert!(snapshot_from_tree(r#"{"nodes": 42}"#).is_none());
    }

    #[test]
    fn ramki_kodiruyutsya_i_dekodiruyutsya() {
        let frame = encode_frame(MSG_GET_TREE, b"payload");
        let mut fb = FrameBuf::default();
        // Скармливаем кусками: заголовок пополам, потом хвост.
        fb.feed(&frame[..7]);
        assert!(fb.next_frame().unwrap().is_none());
        fb.feed(&frame[7..16]);
        assert!(fb.next_frame().unwrap().is_none());
        fb.feed(&frame[16..]);
        let (t, body) = fb.next_frame().unwrap().expect("целая рамка");
        assert_eq!(t, MSG_GET_TREE);
        assert_eq!(body, b"payload");
        // Две рамки подряд одной подачей.
        let mut two = encode_frame(MSG_SUBSCRIBE, b"a");
        two.extend_from_slice(&encode_frame(EVENT_BIT | 3, b"bb"));
        fb.feed(&two);
        assert_eq!(fb.next_frame().unwrap().unwrap().0, MSG_SUBSCRIBE);
        assert_eq!(
            fb.next_frame().unwrap().unwrap(),
            (EVENT_BIT | 3, b"bb".to_vec())
        );
        assert!(fb.next_frame().unwrap().is_none());
    }

    #[test]
    fn dekoder_lomaetsya_na_musore() {
        let mut fb = FrameBuf::default();
        fb.feed(b"definitely-not-i3-ipc-data");
        assert!(fb.next_frame().is_err());
    }
}
