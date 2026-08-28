//! Хранилище сервера: SQLite (WAL) под мьютексом.
//!
//! Сервер намеренно прост (ТЗ §3.5: «эталон селфхоста»): одно соединение,
//! короткие транзакции, корректность важнее пропускной способности.
//! События хранятся как непрозрачный JSON (`payload`) с ключом
//! `(account_id, device, wall_ms, counter)` — сервер НЕ разбирает виды
//! событий и потому совместим вперёд с любыми новыми `EventKind` клиентов.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// Результат claim-or-heartbeat по lease (семантика newest-wins, ТЗ §3.5).
#[derive(Debug, PartialEq, Eq)]
pub struct LeaseAnswer {
    pub granted: bool,
    /// Текущий держатель ПОСЛЕ обработки запроса.
    pub holder: String,
}

/// Событие на выдачу: ключ + непрозрачный payload (исходный JSON события).
pub struct StoredEvent {
    pub device: String,
    pub wall_ms: u64,
    pub counter: u16,
    pub payload: String,
}

/// Курсор «максимальная виденная метка устройства» (пара из Hlc без device).
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub wall_ms: u64,
    pub counter: u16,
}

pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS tokens (
    token_hash TEXT PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id)
);
CREATE TABLE IF NOT EXISTS events (
    account_id INTEGER NOT NULL,
    device     TEXT    NOT NULL,
    wall_ms    INTEGER NOT NULL,
    counter    INTEGER NOT NULL,
    payload    TEXT    NOT NULL,
    PRIMARY KEY (account_id, device, wall_ms, counter)
);
CREATE TABLE IF NOT EXISTS leases (
    account_id    INTEGER PRIMARY KEY,
    device        TEXT    NOT NULL,
    expires_at    INTEGER NOT NULL,
    -- Устройство, у которого lease только что отобрали (newest-wins):
    -- его следующий heartbeat получит granted:false — «питомец убежал».
    ousted_device TEXT
);
";

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("не удалось создать каталог {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("не удалось открыть базу {}", path.display()))?;
        Self::init(conn)
    }

    /// База в памяти — для тестов.
    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        // WAL: читатели не блокируют писателя; busy_timeout — страховка,
        // хотя писатель у нас один (мьютекс).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5_000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA).context("миграция схемы")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // Отравленный мьютекс = паника другого обработчика; продолжать
        // с целой SQLite-транзакцией безопасно.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // -- Аккаунты и токены --------------------------------------------------

    /// Создать аккаунт и первый токен. Возвращает `(id, токен)` — токен
    /// показывается один раз, в базе остаётся только sha256-хэш.
    pub fn account_add(&self, name: &str) -> anyhow::Result<(i64, String)> {
        let token = generate_token()?;
        let conn = self.lock();
        conn.execute("INSERT INTO accounts(name) VALUES (?1)", params![name])
            .with_context(|| format!("аккаунт «{name}» не создан (имя уже занято?)"))?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO tokens(token_hash, account_id) VALUES (?1, ?2)",
            params![token_hash(&token), id],
        )?;
        Ok((id, token))
    }

    /// Список аккаунтов: `(id, имя, событий, токенов)`.
    pub fn account_list(&self) -> anyhow::Result<Vec<(i64, String, u64, u64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT a.id, a.name,
                    (SELECT COUNT(*) FROM events e WHERE e.account_id = a.id),
                    (SELECT COUNT(*) FROM tokens t WHERE t.account_id = a.id)
             FROM accounts a ORDER BY a.id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, u64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Проверить Bearer-токен; `None` = 401.
    pub fn auth(&self, token: &str) -> anyhow::Result<Option<i64>> {
        let conn = self.lock();
        let id = conn
            .query_row(
                "SELECT account_id FROM tokens WHERE token_hash = ?1",
                params![token_hash(token)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id)
    }

    // -- События ------------------------------------------------------------

    /// Идемпотентный upsert события. Повторный push того же id безвреден
    /// (перезаписывает payload тем же содержимым).
    pub fn upsert_event(
        &self,
        account: i64,
        device: &str,
        wall_ms: u64,
        counter: u16,
        payload: &str,
    ) -> anyhow::Result<()> {
        self.lock().execute(
            "INSERT INTO events(account_id, device, wall_ms, counter, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id, device, wall_ms, counter)
             DO UPDATE SET payload = excluded.payload",
            params![account, device, wall_ms as i64, counter, payload],
        )?;
        Ok(())
    }

    /// Все события аккаунта новее курсоров, отсортированные по
    /// `(wall_ms, counter, device)` — порядку HLC из core.
    ///
    /// `cursor_of(device)` — максимальная виденная клиентом метка этого
    /// устройства (нет курсора = отдать всё). Append-only журналы на
    /// устройство делают такой курсорный синк точным без merkle (контракт E).
    pub fn events_after(
        &self,
        account: i64,
        cursor_of: impl Fn(&str) -> Option<Cursor>,
    ) -> anyhow::Result<Vec<StoredEvent>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT device, wall_ms, counter, payload FROM events
             WHERE account_id = ?1 ORDER BY wall_ms, counter, device",
        )?;
        let rows = stmt.query_map(params![account], |r| {
            Ok(StoredEvent {
                device: r.get(0)?,
                wall_ms: r.get::<_, i64>(1)? as u64,
                counter: r.get(2)?,
                payload: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let ev = row?;
            let newer = match cursor_of(&ev.device) {
                None => true,
                Some(c) => (ev.wall_ms, ev.counter) > (c.wall_ms, c.counter),
            };
            if newer {
                out.push(ev);
            }
        }
        Ok(out)
    }

    // -- Lease (присутствие: питомец один, ТЗ §3.5) ---------------------------

    /// Claim-or-heartbeat. Семантика newest-wins:
    /// - lease свободен/истёк → выдать;
    /// - держатель шлёт сам → heartbeat, продлить;
    /// - чужой claim → отобрать НЕМЕДЛЕННО (granted:true), старого держателя
    ///   запомнить в `ousted_device`;
    /// - heartbeat только что смещённого держателя → granted:false + кто
    ///   держит («питомец убежал»); пометка снимается, так что его
    ///   СЛЕДУЮЩИЙ запрос — уже свежий claim и снова отберёт lease
    ///   (пользователь вернулся к этому устройству).
    pub fn lease_claim(
        &self,
        account: i64,
        device: &str,
        ttl_s: u64,
        now_s: u64,
    ) -> anyhow::Result<LeaseAnswer> {
        let expires = now_s.saturating_add(ttl_s) as i64;
        let conn = self.lock();
        let row: Option<(String, i64, Option<String>)> = conn
            .query_row(
                "SELECT device, expires_at, ousted_device FROM leases WHERE account_id = ?1",
                params![account],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let grant = |conn: &Connection, ousted: Option<&str>| -> anyhow::Result<LeaseAnswer> {
            conn.execute(
                "INSERT INTO leases(account_id, device, expires_at, ousted_device)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id) DO UPDATE
                 SET device = excluded.device,
                     expires_at = excluded.expires_at,
                     ousted_device = excluded.ousted_device",
                params![account, device, expires, ousted],
            )?;
            Ok(LeaseAnswer {
                granted: true,
                holder: device.to_string(),
            })
        };
        match row {
            None => grant(&conn, None),
            Some((_, exp, _)) if exp <= now_s as i64 => grant(&conn, None),
            Some((holder, _, ousted)) => {
                if holder == device {
                    // Heartbeat держателя: продлеваем, пометку ousted не трогаем.
                    conn.execute(
                        "UPDATE leases SET expires_at = ?2 WHERE account_id = ?1",
                        params![account, expires],
                    )?;
                    Ok(LeaseAnswer {
                        granted: true,
                        holder,
                    })
                } else if ousted.as_deref() == Some(device) {
                    // Первый запрос смещённого устройства после захвата:
                    // сообщить о потере, дальше оно вправе снова claim-ить.
                    conn.execute(
                        "UPDATE leases SET ousted_device = NULL WHERE account_id = ?1",
                        params![account],
                    )?;
                    Ok(LeaseAnswer {
                        granted: false,
                        holder,
                    })
                } else {
                    // Новый claim побеждает (newest-wins).
                    grant(&conn, Some(&holder))
                }
            }
        }
    }

    /// Отпустить lease добровольно; `true`, если держали именно мы.
    pub fn lease_release(&self, account: i64, device: &str) -> anyhow::Result<bool> {
        let n = self.lock().execute(
            "DELETE FROM leases WHERE account_id = ?1 AND device = ?2",
            params![account, device],
        )?;
        Ok(n > 0)
    }
}

fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Токен аккаунта: 32 криптослучайных байта в hex (64 символа).
fn generate_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("нет источника случайности: {e}"))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip_and_unknown_token() {
        let db = Db::open_in_memory().unwrap();
        let (id, token) = db.account_add("home").unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(db.auth(&token).unwrap(), Some(id));
        assert_eq!(db.auth("deadbeef").unwrap(), None);
        // В базе токена в открытом виде нет.
        let listed = db.account_list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].1, "home");
    }

    #[test]
    fn duplicate_account_name_is_error() {
        let db = Db::open_in_memory().unwrap();
        db.account_add("x").unwrap();
        assert!(db.account_add("x").is_err());
    }

    #[test]
    fn expired_lease_is_free_for_anyone() {
        let db = Db::open_in_memory().unwrap();
        let (id, _) = db.account_add("a").unwrap();
        let first = db.lease_claim(id, "laptop", 30, 1_000).unwrap();
        assert!(first.granted);
        // TTL вышел: чужой запрос берёт lease как свободный, ousted не ставится…
        let second = db.lease_claim(id, "desktop", 30, 1_031).unwrap();
        assert!(second.granted);
        // …поэтому старый держатель после истечения тоже просто claim-ит
        // (и по newest-wins отбирает обратно).
        let third = db.lease_claim(id, "laptop", 30, 1_032).unwrap();
        assert!(third.granted);
        assert_eq!(third.holder, "laptop");
    }
}
