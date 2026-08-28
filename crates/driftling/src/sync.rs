//! Синк-воркер демона (фаза E, ТЗ §3.5): фоновый поток с блокирующим
//! HTTP (minreq, БЕЗ tokio — фичи zbus/ksni неприкосновенны, см.
//! workspace Cargo.toml) для режима «свой сервер».
//!
//! Обязанности воркера:
//! - **push**: свои события (файл `journal.<device>.jsonl`) строго новее
//!   подтверждённого курсора — `POST /v1/push`; курсор — в
//!   `<data_dir>/sync-state.json` (локальный файл, НЕ синкается);
//! - **pull**: `POST /v1/pull` с курсорами всего локального журнала;
//!   привезённые события материализуются на диск
//!   ([`Journal::append_remote`]) и уходят демону заметкой
//!   [`SyncNote::Remote`] — тот вливает их в память, пересворачивает
//!   и обновляет визуал;
//! - **lease присутствия**: claim по пользовательскому действию/призыву,
//!   heartbeat раз в TTL/3, пока питомец на экране И мы держатели;
//!   `granted:false` на heartbeat — заметка [`SyncNote::LeaseLost`]
//!   (демон играет «питомец перебегает»).
//!
//! Расписание: полный цикл push+pull раз в [`Tuning::sync_every`] и
//! немедленно по [`SyncCmd::Wake`] (демон шлёт его после каждого своего
//! append). Ошибки сети — не смерть: warn в лог (повторы — debug),
//! экспоненциальный бэкофф, оффлайн-режим штатен (ТЗ §3.5: партиция
//! продолжает заботиться локально, журналы сольются потом).
//!
//! Демон общается с воркером каналами ([`SyncHandle`]); статус для
//! `ctl sync status` — разделяемый [`SyncShared`]. Воркер завершается,
//! когда демон дропает `cmd_tx` (reload с новым конфигом или выход).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use driftling_core::{cursors_of, events_after, Event, Hlc, Journal, SyncConfig};
use serde::{Deserialize, Serialize};

/// Таймаут одного HTTP-запроса, сек (чтение и соединение вместе, minreq).
const HTTP_TIMEOUT_S: u64 = 10;

/// Расписание воркера. Продовые значения — [`Tuning::default`]; тесты
/// ужимают интервалы до миллисекунд (ТД-26: DI вместо ожиданий).
#[derive(Debug, Clone)]
pub(crate) struct Tuning {
    /// Полный цикл push+pull.
    pub sync_every: Duration,
    /// Heartbeat lease, пока питомец на экране (ТТL/3).
    pub heartbeat_every: Duration,
    /// TTL lease, сек (истекает у сервера при пропаже устройства).
    pub lease_ttl_s: u64,
    /// Потолок бэкоффа цикла синка при ошибках.
    pub backoff_max: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            sync_every: Duration::from_secs(30),
            heartbeat_every: Duration::from_secs(5),
            lease_ttl_s: 15,
            backoff_max: Duration::from_secs(300),
        }
    }
}

/// Команды демона воркеру.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncCmd {
    /// Локальный append: синкнуться немедленно, не ждать таймера.
    Wake,
    /// Пользовательское действие/призыв: забрать lease присутствия.
    Claim,
    /// Питомец на экране этого устройства? (включает/гасит heartbeat)
    SetSummoned(bool),
    /// Явный dismiss пользователем: отпустить lease (DELETE).
    Release,
}

/// Заметки воркера демону; разбираются в `DaemonApp::tick`.
#[derive(Debug)]
pub(crate) enum SyncNote {
    /// Свежие чужие события (уже записаны на диск) — влить в память,
    /// пересвернуть, обновить визуал.
    Remote(Vec<Event>),
    /// Lease забрало другое устройство — «питомец перебегает» на holder.
    LeaseLost { holder: String },
    /// Claim удался; после lease-скрытия демон играет «прибежал» (run-in).
    LeaseGained,
}

/// Разделяемый статус для `ctl sync status` и окна настроек.
#[derive(Debug, Default)]
pub(crate) struct SyncShared {
    /// Момент последнего успешного push (в т.ч. пустого).
    pub last_push: Option<Instant>,
    /// Момент последнего успешного pull.
    pub last_pull: Option<Instant>,
    /// Мы считаем себя держателем lease.
    pub holding: bool,
    /// Последний известный держатель lease (device_id).
    pub holder: Option<String>,
    /// Последняя ошибка синка; None — ошибок нет (или прошли).
    pub last_error: Option<String>,
}

/// Ручка воркера у демона. Дроп ручки (reload/выход) завершает поток:
/// воркер видит закрытый канал команд на ближайшем пробуждении.
pub(crate) struct SyncHandle {
    pub(crate) cmd_tx: Sender<SyncCmd>,
    pub(crate) notes: Receiver<SyncNote>,
    pub(crate) shared: Arc<Mutex<SyncShared>>,
}

impl SyncHandle {
    /// Отправить команду; мёртвый воркер (гонка на reload) — не ошибка.
    pub(crate) fn send(&self, cmd: SyncCmd) {
        let _ = self.cmd_tx.send(cmd);
    }
}

// ---------------------------------------------------------------------------
// sync-state.json: подтверждённый push-курсор
// ---------------------------------------------------------------------------

/// Постоянное состояние воркера: `<data_dir>/sync-state.json`. Хранится
/// ТОЛЬКО курсор подтверждённого push (максимальная своя метка, которую
/// сервер принял); pull-курсоры каждый раз выводятся из журнала на диске —
/// им отдельное состояние не нужно (диск и есть знание устройства).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct SyncState {
    pushed: Option<Hlc>,
}

fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sync-state.json")
}

/// Прочитать состояние; нет файла или битый — дефолт (повторный push
/// идемпотентен, upsert на сервере).
fn load_state(data_dir: &Path) -> SyncState {
    match std::fs::read_to_string(state_path(data_dir)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("sync-state.json битый ({e}) — начинаем с нуля (push идемпотентен)");
            SyncState::default()
        }),
        Err(_) => SyncState::default(),
    }
}

/// Атомарная запись состояния (tmp + rename), как у config/pet.json.
fn save_state(data_dir: &Path, state: &SyncState) -> Result<(), String> {
    let p = state_path(data_dir);
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Чистая логика (юнит-тесты внизу)
// ---------------------------------------------------------------------------

/// СВОИ события строго новее подтверждённого курсора — тело push.
/// `events` отсортированы (гарантия Journal::open) — выборка сохраняет
/// порядок, последний элемент = новый курсор после подтверждения.
fn events_to_push<'a>(events: &'a [Event], device: &str, pushed: &Option<Hlc>) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|e| e.id.device == device && pushed.as_ref().is_none_or(|p| e.id > *p))
        .collect()
}

/// Реакция на ответ lease-ручки: `(держали, granted) -> (держим, заметка)`.
/// Одна точка истины для claim и heartbeat: потеря — только у бывшего
/// держателя (заметка Lost -> «перебегает»), приобретение — только у
/// нового (заметка Gained -> возможен run-in).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseShift {
    None,
    Gained,
    Lost,
}

fn lease_transition(was_holding: bool, granted: bool) -> (bool, LeaseShift) {
    match (was_holding, granted) {
        (false, true) => (true, LeaseShift::Gained),
        (true, true) => (true, LeaseShift::None),
        (true, false) => (false, LeaseShift::Lost),
        (false, false) => (false, LeaseShift::None),
    }
}

/// Бэкофф цикла синка: 1x, 2x, 4x… интервала, с потолком.
fn backoff_delay(base: Duration, max: Duration, streak: u32) -> Duration {
    let factor = 1u32 << streak.min(4);
    (base * factor).min(max)
}

// ---------------------------------------------------------------------------
// Воркер
// ---------------------------------------------------------------------------

/// Запустить воркер режима «свой сервер». `data_dir` — локальные
/// pet.json/sync-state, `journal_dir` — каталог журналов (в серверном
/// режиме это тот же data_dir), `device` — id этого устройства,
/// `summoned` — питомец сейчас на экране (heartbeat с порога).
pub(crate) fn spawn(
    cfg: &SyncConfig,
    data_dir: &Path,
    journal_dir: &Path,
    device: &str,
    summoned: bool,
    tuning: Tuning,
) -> SyncHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (note_tx, notes) = mpsc::channel();
    let shared = Arc::new(Mutex::new(SyncShared::default()));
    let worker = Worker {
        address: cfg.address.trim().trim_end_matches('/').to_string(),
        token: cfg.token.trim().to_string(),
        data_dir: data_dir.to_path_buf(),
        journal_dir: journal_dir.to_path_buf(),
        device: device.to_string(),
        tuning,
        state: load_state(data_dir),
        summoned,
        holding: false,
        sync_streak: 0,
        lease_streak: 0,
        notes: note_tx,
        shared: Arc::clone(&shared),
    };
    std::thread::Builder::new()
        .name("driftling-sync".into())
        .spawn(move || worker.run(cmd_rx))
        .expect("поток синк-воркера");
    SyncHandle {
        cmd_tx,
        notes,
        shared,
    }
}

struct Worker {
    address: String,
    token: String,
    data_dir: PathBuf,
    journal_dir: PathBuf,
    device: String,
    tuning: Tuning,
    state: SyncState,
    summoned: bool,
    holding: bool,
    /// Подряд неудачных циклов синка (бэкофф + приглушение лога).
    sync_streak: u32,
    /// Подряд неудачных lease-запросов (приглушение лога).
    lease_streak: u32,
    notes: Sender<SyncNote>,
    shared: Arc<Mutex<SyncShared>>,
}

impl Worker {
    fn run(mut self, cmd_rx: Receiver<SyncCmd>) {
        log::info!(
            "синк: воркер запущен ({}), устройство {}",
            self.address,
            self.device
        );
        let mut next_sync = Instant::now(); // первый цикл — сразу
        let mut next_hb = Instant::now();
        loop {
            // Спим до ближайшего дедлайна, но просыпаемся на команды.
            let mut deadline = next_sync;
            if self.summoned && self.holding {
                deadline = deadline.min(next_hb);
            }
            let timeout = deadline.saturating_duration_since(Instant::now());
            match cmd_rx.recv_timeout(timeout) {
                Ok(cmd) => {
                    self.handle_cmd(cmd, &mut next_sync, &mut next_hb);
                    // Коалесценция: серия команд (шторм Wake от пачки
                    // append) сливается разом — один HTTP-цикл вместо N.
                    while let Ok(more) = cmd_rx.try_recv() {
                        self.handle_cmd(more, &mut next_sync, &mut next_hb);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // Демон дропнул ручку: reload или выход — завершаемся тихо.
                Err(RecvTimeoutError::Disconnected) => {
                    log::debug!("синк: воркер остановлен (канал закрыт)");
                    return;
                }
            }
            if Instant::now() >= next_sync {
                let ok = self.sync_once();
                let delay = if ok {
                    self.tuning.sync_every
                } else {
                    backoff_delay(
                        self.tuning.sync_every,
                        self.tuning.backoff_max,
                        self.sync_streak.saturating_sub(1),
                    )
                };
                next_sync = Instant::now() + delay;
            }
            if self.summoned && self.holding && Instant::now() >= next_hb {
                self.heartbeat();
                // При сетевой ошибке — реже: не молотим мёртвый сервер.
                let mult = if self.lease_streak > 0 { 3 } else { 1 };
                next_hb = Instant::now() + self.tuning.heartbeat_every * mult;
            }
        }
    }

    /// Обработать одну команду демона (планирование — через дедлайны).
    fn handle_cmd(&mut self, cmd: SyncCmd, next_sync: &mut Instant, next_hb: &mut Instant) {
        match cmd {
            SyncCmd::Wake => *next_sync = Instant::now(),
            SyncCmd::Claim => {
                self.claim();
                *next_hb = Instant::now() + self.tuning.heartbeat_every;
            }
            SyncCmd::SetSummoned(s) => {
                self.summoned = s;
                if s {
                    *next_hb = Instant::now() + self.tuning.heartbeat_every;
                }
            }
            SyncCmd::Release => self.release(),
        }
    }

    // -- HTTP ---------------------------------------------------------------

    fn post_json<B: Serialize>(&self, path: &str, body: &B) -> Result<(u16, String), String> {
        let body = serde_json::to_string(body).map_err(|e| e.to_string())?;
        let resp = minreq::post(format!("{}{}", self.address, path))
            .with_header("Authorization", format!("Bearer {}", self.token))
            .with_header("Content-Type", "application/json")
            .with_timeout(HTTP_TIMEOUT_S)
            .with_body(body)
            .send()
            .map_err(|e| format!("{path}: {e}"))?;
        let text = resp.as_str().unwrap_or_default().to_string();
        Ok((resp.status_code, text))
    }

    // -- Цикл push+pull -----------------------------------------------------

    fn sync_once(&mut self) -> bool {
        match self.try_sync() {
            Ok(()) => {
                if self.sync_streak > 0 {
                    log::info!("синк: связь с сервером восстановлена");
                }
                self.sync_streak = 0;
                self.shared.lock().unwrap().last_error = None;
                true
            }
            Err(e) => {
                // Первый сбой — warn, дальнейшие — debug (не засоряем лог
                // в оффлайне); бэкофф считает run().
                if self.sync_streak == 0 {
                    log::warn!("синк: {e} (продолжаю с бэкоффом; оффлайн — это штатно)");
                } else {
                    log::debug!("синк: {e} (сбой #{})", self.sync_streak + 1);
                }
                self.sync_streak = self.sync_streak.saturating_add(1);
                self.shared.lock().unwrap().last_error = Some(e);
                false
            }
        }
    }

    fn try_sync(&mut self) -> Result<(), String> {
        let (events, warnings) = Journal::open(&self.journal_dir)?;
        if warnings > 0 {
            log::debug!("синк: в журнале битых строк: {warnings}");
        }

        // PUSH: свои события новее подтверждённого курсора.
        let to_push = events_to_push(&events, &self.device, &self.state.pushed);
        if !to_push.is_empty() {
            #[derive(Serialize)]
            struct PushReq<'a> {
                device: &'a str,
                events: &'a [&'a Event],
            }
            let (status, text) = self.post_json(
                "/v1/push",
                &PushReq {
                    device: &self.device,
                    events: &to_push,
                },
            )?;
            if status != 200 {
                return Err(format!("push: HTTP {status}: {text}"));
            }
            let newest = to_push.last().expect("to_push не пуст").id.clone();
            log::info!("синк: отправлено событий: {}", to_push.len());
            self.state.pushed = Some(newest);
            if let Err(e) = save_state(&self.data_dir, &self.state) {
                // Не смертельно: потерянный курсор = повторный push,
                // сервер дедуплицирует по id.
                log::warn!("sync-state.json не сохранён: {e}");
            }
        }
        self.shared.lock().unwrap().last_push = Some(Instant::now());

        // PULL: курсоры всего локального знания -> недостающее с сервера.
        let cursors = cursors_of(&events);
        let (status, text) = self.post_json("/v1/pull", &cursors)?;
        if status != 200 {
            return Err(format!("pull: HTTP {status}: {text}"));
        }
        #[derive(Deserialize)]
        struct PullResp {
            events: Vec<Event>,
        }
        let resp: PullResp =
            serde_json::from_str(&text).map_err(|e| format!("pull: битый ответ: {e}"))?;
        // Защитный фильтр + сортировка (events_after заодно отбрасывает
        // то, что сервер прислал бы лишнего) — и на диск, файлами
        // устройств-авторов.
        let fresh = events_after(&resp.events, &cursors);
        if !fresh.is_empty() {
            if let Err(e) = Journal::append_remote(&self.journal_dir, &fresh) {
                log::warn!("синк: чужие события не легли на диск ({e}) — применяю только в памяти");
            }
            log::info!("синк: получено новых событий: {}", fresh.len());
            let _ = self.notes.send(SyncNote::Remote(fresh));
        }
        self.shared.lock().unwrap().last_pull = Some(Instant::now());
        Ok(())
    }

    // -- Lease присутствия ---------------------------------------------------

    fn lease_post(&self) -> Result<(bool, String), String> {
        #[derive(Serialize)]
        struct LeaseReq<'a> {
            device: &'a str,
            ttl_s: u64,
        }
        #[derive(Deserialize)]
        struct LeaseResp {
            granted: bool,
            holder: String,
        }
        let (status, text) = self.post_json(
            "/v1/lease",
            &LeaseReq {
                device: &self.device,
                ttl_s: self.tuning.lease_ttl_s,
            },
        )?;
        if status != 200 {
            return Err(format!("lease: HTTP {status}: {text}"));
        }
        let resp: LeaseResp =
            serde_json::from_str(&text).map_err(|e| format!("lease: битый ответ: {e}"))?;
        Ok((resp.granted, resp.holder))
    }

    /// Применить исход lease-запроса: перевести holding, обновить статус,
    /// отправить заметку демону.
    fn apply_lease(&mut self, granted: bool, holder: String) {
        let (holding, shift) = lease_transition(self.holding, granted);
        self.holding = holding;
        {
            let mut sh = self.shared.lock().unwrap();
            sh.holding = holding;
            sh.holder = Some(holder.clone());
        }
        match shift {
            LeaseShift::Gained => {
                log::info!("присутствие: lease наш");
                let _ = self.notes.send(SyncNote::LeaseGained);
            }
            LeaseShift::Lost => {
                log::info!("присутствие: lease забрал {holder} — питомец перебегает");
                let _ = self.notes.send(SyncNote::LeaseLost { holder });
            }
            LeaseShift::None => {}
        }
    }

    /// Забрать lease (пользовательское действие/призыв). Пользователь
    /// взаимодействует ЗДЕСЬ — granted:false тут не «питомец убегает», а
    /// снятый ousted-маркер сервера: он честно отвечает false ровно один
    /// раз, вторая попытка берёт слот. Потому до двух попыток и без
    /// Lost-заметки между ними.
    fn claim(&mut self) {
        for _ in 0..2 {
            match self.lease_post() {
                Ok((true, holder)) => {
                    self.lease_streak = 0;
                    self.apply_lease(true, holder);
                    return;
                }
                Ok((false, holder)) => {
                    self.lease_streak = 0;
                    self.shared.lock().unwrap().holder = Some(holder);
                    // Вторая попытка возьмёт слот (пометка ousted снята).
                }
                Err(e) => {
                    // Оффлайн: показываем питомца локально (ТЗ §3.5) и
                    // считаем себя держателем — первый же успешный
                    // heartbeat (claim-or-heartbeat) оформит это у сервера.
                    if self.lease_streak == 0 {
                        log::warn!("присутствие: {e} — оффлайн, питомец остаётся локально");
                    }
                    self.lease_streak = self.lease_streak.saturating_add(1);
                    self.holding = true;
                    self.shared.lock().unwrap().holding = true;
                    return;
                }
            }
        }
        // Обе попытки granted:false — сервер упрямо на стороне другого
        // устройства (гонка одновременных claim): уступаем.
        let holder = self.shared.lock().unwrap().holder.clone();
        log::info!("присутствие: claim проигран (держатель {holder:?})");
        self.holding = false;
        self.shared.lock().unwrap().holding = false;
        let _ = self.notes.send(SyncNote::LeaseLost {
            holder: holder.unwrap_or_default(),
        });
    }

    /// Heartbeat держателя. granted:false = lease забрали — заметка демону.
    fn heartbeat(&mut self) {
        match self.lease_post() {
            Ok((granted, holder)) => {
                self.lease_streak = 0;
                self.apply_lease(granted, holder);
            }
            Err(e) => {
                // Сеть упала: продолжаем показывать питомца (партиция —
                // штатный режим); holding не трогаем.
                if self.lease_streak == 0 {
                    log::warn!("присутствие: heartbeat не прошёл ({e}) — работаем оффлайн");
                } else {
                    log::debug!("присутствие: heartbeat не прошёл ({e})");
                }
                self.lease_streak = self.lease_streak.saturating_add(1);
            }
        }
    }

    /// Отпустить lease (явный dismiss пользователем). Best-effort.
    fn release(&mut self) {
        self.holding = false;
        {
            let mut sh = self.shared.lock().unwrap();
            sh.holding = false;
            // Кто держит lease дальше — неизвестно (пассивное устройство не
            // опрашивает сервер): стираем держателя, иначе `ctl sync status`
            // показывал бы устаревшего «держателя» — самого себя.
            sh.holder = None;
        }
        #[derive(Serialize)]
        struct ReleaseReq<'a> {
            device: &'a str,
        }
        let body = match serde_json::to_string(&ReleaseReq {
            device: &self.device,
        }) {
            Ok(b) => b,
            Err(_) => return,
        };
        let result = minreq::delete(format!("{}/v1/lease", self.address))
            .with_header("Authorization", format!("Bearer {}", self.token))
            .with_header("Content-Type", "application/json")
            .with_timeout(HTTP_TIMEOUT_S)
            .with_body(body)
            .send();
        match result {
            Ok(resp) if resp.status_code == 200 => log::debug!("присутствие: lease отпущен"),
            Ok(resp) => log::debug!("присутствие: release HTTP {}", resp.status_code),
            Err(e) => log::debug!("присутствие: release не прошёл ({e})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Тесты
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use driftling_core::EventKind;

    fn ev(device: &str, wall_ms: u64, counter: u16) -> Event {
        Event {
            id: Hlc {
                wall_ms,
                counter,
                device: device.into(),
            },
            kind: EventKind::Petted,
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("driftling-sync-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- Курсор push: машина состояний --------------------------------------

    #[test]
    fn events_to_push_takes_own_after_cursor_only() {
        let log = vec![
            ev("mine", 1, 0),
            ev("other", 2, 0),
            ev("mine", 3, 0),
            ev("mine", 3, 1),
        ];
        // Без курсора — все свои, порядок сохранён, чужие никогда.
        let all = events_to_push(&log, "mine", &None);
        let ids: Vec<u64> = all.iter().map(|e| e.id.wall_ms).collect();
        assert_eq!(ids, vec![1, 3, 3]);

        // Строго новее курсора: метка ровно на курсоре уже подтверждена.
        let cursor = Some(log[0].id.clone());
        let rest = events_to_push(&log, "mine", &cursor);
        assert_eq!(rest.len(), 2);
        assert!(rest.iter().all(|e| e.id > log[0].id));

        // Курсор в хвосте — пусто; неизвестное устройство — пусто.
        let tail = Some(log[3].id.clone());
        assert!(events_to_push(&log, "mine", &tail).is_empty());
        assert!(events_to_push(&log, "nobody", &None).is_empty());
    }

    /// Подтверждение push двигает курсор к последнему отправленному, и
    /// следующая выборка пуста, пока не появятся новые события, — точная
    /// машина курсора без пропусков и повторов.
    #[test]
    fn push_cursor_advances_exactly() {
        let mut log = vec![ev("mine", 1, 0), ev("mine", 2, 0)];
        let mut pushed: Option<Hlc> = None;

        let batch = events_to_push(&log, "mine", &pushed);
        pushed = Some(batch.last().unwrap().id.clone());
        assert!(
            events_to_push(&log, "mine", &pushed).is_empty(),
            "повторов нет"
        );

        log.push(ev("mine", 5, 0));
        let next = events_to_push(&log, "mine", &pushed);
        assert_eq!(next.len(), 1, "новое событие не пропущено");
        assert_eq!(next[0].id.wall_ms, 5);
    }

    #[test]
    fn sync_state_roundtrips_and_survives_garbage() {
        let dir = tmp_dir("state");
        assert_eq!(load_state(&dir), SyncState::default(), "нет файла — дефолт");

        let state = SyncState {
            pushed: Some(Hlc {
                wall_ms: 42,
                counter: 7,
                device: "mine".into(),
            }),
        };
        save_state(&dir, &state).unwrap();
        assert_eq!(load_state(&dir), state);

        std::fs::write(state_path(&dir), "{мусор").unwrap();
        assert_eq!(load_state(&dir), SyncState::default(), "битый — дефолт");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Lease: реакции ------------------------------------------------------

    #[test]
    fn lease_transition_matrix() {
        assert_eq!(lease_transition(false, true), (true, LeaseShift::Gained));
        assert_eq!(lease_transition(true, true), (true, LeaseShift::None));
        assert_eq!(lease_transition(true, false), (false, LeaseShift::Lost));
        assert_eq!(lease_transition(false, false), (false, LeaseShift::None));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let base = Duration::from_secs(30);
        let max = Duration::from_secs(300);
        assert_eq!(backoff_delay(base, max, 0), Duration::from_secs(30));
        assert_eq!(backoff_delay(base, max, 1), Duration::from_secs(60));
        assert_eq!(backoff_delay(base, max, 3), Duration::from_secs(240));
        assert_eq!(backoff_delay(base, max, 4), max, "потолок");
        assert_eq!(backoff_delay(base, max, 99), max, "без переполнения");
    }

    // -- Интеграция с настоящим driftling-server -----------------------------

    /// Путь собранного серверного бинаря; None — не собран (тест тихо
    /// пропускается: `cargo build -p driftling-server` его создаёт).
    fn server_bin() -> Option<PathBuf> {
        // target/ лежит на два уровня выше crates/driftling.
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            })
            .join("driftling-server");
        target.canonicalize().ok().filter(|p| p.is_file())
    }

    /// Убить серверный процесс при любом исходе теста.
    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn test_tuning() -> Tuning {
        Tuning {
            sync_every: Duration::from_millis(150),
            heartbeat_every: Duration::from_millis(100),
            lease_ttl_s: 2,
            backoff_max: Duration::from_secs(1),
        }
    }

    fn server_cfg(dir: &Path, device: &str, address: &str, token: &str) -> SyncConfig {
        let _ = dir;
        let _ = device;
        SyncConfig {
            mode: driftling_core::SyncMode::Server,
            address: address.to_string(),
            token: token.to_string(),
            folder: String::new(),
        }
    }

    /// Дождаться заметки нужного вида (остальные пропустить).
    fn wait_note<F: Fn(&SyncNote) -> bool>(
        handle: &SyncHandle,
        what: &str,
        pred: F,
    ) -> Option<SyncNote> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match handle.notes.recv_timeout(Duration::from_millis(200)) {
                Ok(note) if pred(&note) => return Some(note),
                Ok(_) => {}
                Err(_) => {}
            }
        }
        eprintln!("не дождались заметки: {what}");
        None
    }

    /// Полный обмен через НАСТОЯЩИЙ driftling-server: два «устройства»
    /// сходятся журналами, lease переходит по newest-wins с заметками
    /// Lost/Gained (критерий M3 ТЗ). Тест сам пропускается, если бинарь
    /// сервера не собран.
    #[test]
    fn two_devices_converge_via_real_server() {
        let Some(bin) = server_bin() else {
            eprintln!("SKIP: driftling-server не собран (cargo build -p driftling-server)");
            return;
        };
        let root = tmp_dir("e2e");
        let port = free_port();
        let address = format!("http://127.0.0.1:{port}");
        std::fs::write(
            root.join("server.toml"),
            format!("listen = \"127.0.0.1:{port}\"\ndb = \"server.sqlite3\"\n"),
        )
        .unwrap();

        // Аккаунт: токен печатается последней строкой stdout.
        let out = std::process::Command::new(&bin)
            .args(["--config", "server.toml", "account", "add", "тест"])
            .current_dir(&root)
            .output()
            .expect("driftling-server account add");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let token = stdout
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .expect("токен в stdout")
            .to_string();

        let child = std::process::Command::new(&bin)
            .args(["--config", "server.toml", "serve"])
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("driftling-server serve");
        let _guard = KillOnDrop(child);

        // Ждём готовности health.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let ok = minreq::get(format!("{address}/v1/health"))
                .with_timeout(2)
                .send()
                .map(|r| r.status_code == 200)
                .unwrap_or(false);
            if ok {
                break;
            }
            assert!(Instant::now() < deadline, "сервер не поднялся за 10 с");
            std::thread::sleep(Duration::from_millis(100));
        }

        let dir_a = root.join("dev-a");
        let dir_b = root.join("dev-b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        // Устройство A рождает событие и пушит.
        let ev_a = ev("dev-a", 1_000, 0);
        Journal::append(&dir_a, "dev-a", &ev_a).unwrap();
        let a = spawn(
            &server_cfg(&dir_a, "dev-a", &address, &token),
            &dir_a,
            &dir_a,
            "dev-a",
            false,
            test_tuning(),
        );
        let b = spawn(
            &server_cfg(&dir_b, "dev-b", &address, &token),
            &dir_b,
            &dir_b,
            "dev-b",
            false,
            test_tuning(),
        );

        // B получает событие A: заметка Remote + файл автора на диске.
        let note = wait_note(&b, "Remote у B", |n| matches!(n, SyncNote::Remote(_)))
            .expect("B должен получить события A");
        match note {
            SyncNote::Remote(events) => {
                assert!(events.iter().any(|e| e.id == ev_a.id), "{events:?}");
            }
            other => panic!("неожиданная заметка: {other:?}"),
        }
        let b_copy = driftling_core::device_journal_path_in(&dir_b, "dev-a");
        assert!(b_copy.is_file(), "pull материализован файлом автора");

        // Двусторонность: событие B доезжает до A (wake ускоряет).
        let ev_b = ev("dev-b", 2_000, 0);
        Journal::append(&dir_b, "dev-b", &ev_b).unwrap();
        b.send(SyncCmd::Wake);
        let note = wait_note(&a, "Remote у A", |n| matches!(n, SyncNote::Remote(_)))
            .expect("A должен получить события B");
        if let SyncNote::Remote(events) = note {
            assert!(events.iter().any(|e| e.id == ev_b.id));
        }

        // Повторный цикл ничего не дублирует: журналы обоих сошлись.
        std::thread::sleep(Duration::from_millis(500));
        let (merged_a, _) = Journal::open(&dir_a).unwrap();
        let (merged_b, _) = Journal::open(&dir_b).unwrap();
        assert_eq!(merged_a, merged_b, "журналы сошлись без диалогов");
        assert_eq!(merged_a.len(), 2);

        // Lease: A призван и клеймит; B клеймит следом — A получает Lost.
        a.send(SyncCmd::SetSummoned(true));
        a.send(SyncCmd::Claim);
        wait_note(&a, "Gained у A", |n| matches!(n, SyncNote::LeaseGained)).expect("A взял lease");
        b.send(SyncCmd::SetSummoned(true));
        b.send(SyncCmd::Claim);
        wait_note(&b, "Gained у B", |n| matches!(n, SyncNote::LeaseGained))
            .expect("B перехватил lease (newest-wins)");
        let lost = wait_note(&a, "Lost у A", |n| matches!(n, SyncNote::LeaseLost { .. }))
            .expect("heartbeat A узнаёт о потере");
        if let SyncNote::LeaseLost { holder } = lost {
            assert_eq!(holder, "dev-b");
        }
        // Возврат: пользователь на A взаимодействует — claim возвращает
        // lease (после ousted-отказа воркер повторяет попытку сам).
        a.send(SyncCmd::Claim);
        wait_note(&a, "Gained у A (возврат)", |n| {
            matches!(n, SyncNote::LeaseGained)
        })
        .expect("A вернул lease взаимодействием");

        // Release (dismiss): статус не должен запоминать устаревшего
        // держателя — пассивное устройство не знает, кто держит lease.
        a.send(SyncCmd::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let sh = a.shared.lock().unwrap();
            if !sh.holding && sh.holder.is_none() {
                break;
            }
            drop(sh);
            assert!(Instant::now() < deadline, "release не очистил держателя");
            std::thread::sleep(Duration::from_millis(20));
        }

        drop(a);
        drop(b);
        let _ = std::fs::remove_dir_all(&root);
    }
}
