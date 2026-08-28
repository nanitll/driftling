//! Синхронизация журналов (фаза E, ТЗ §3.5): чистая логика курсоров и
//! слияния, общая для клиента (демон) и сервера (`driftling-server`).
//!
//! Модель: у каждого устройства свой append-only лог ([`crate::journal`]),
//! курсор устройства — максимальная виденная HLC-метка. Раз пер-девайс
//! логи append-only, у получателя всегда есть ровно ПРЕФИКС лога каждого
//! устройства — поэтому курсоры дают точную выборку недостающего
//! ([`events_after`]), и merkle-структуры для поиска точки расхождения не
//! нужны.
//!
//! Протокол поверх этого (HTTP push/pull, lease) — транспорт фазы E;
//! здесь только чистые функции над `Vec<Event>`.
//!
//! Правила чистоты (ТЗ §3.7): модуль собирается под wasm32 — только
//! std-коллекции и serde; ни часов, ни файлов, ни сети.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::journal::{merge, Event, Hlc};

/// Курсоры синка: `устройство -> максимальная виденная метка`.
///
/// Сериализация прозрачная — JSON-карта `{device_id: Hlc}` (формат
/// курсоров `GET /v1/pull` серверного протокола). `BTreeMap` держит
/// ключи отсортированными — сериализация детерминирована.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursors(pub BTreeMap<String, Hlc>);

impl Cursors {
    /// Учесть виденную метку: курсор её устройства только растёт.
    pub fn observe(&mut self, id: &Hlc) {
        match self.0.get_mut(&id.device) {
            Some(cur) => {
                if *id > *cur {
                    *cur = id.clone();
                }
            }
            None => {
                self.0.insert(id.device.clone(), id.clone());
            }
        }
    }

    /// Метка уже покрыта курсорами (не новее курсора своего устройства)?
    /// Метки неизвестных устройств не покрыты ничем.
    pub fn covers(&self, id: &Hlc) -> bool {
        self.0.get(&id.device).is_some_and(|cur| *cur >= *id)
    }
}

/// Курсоры журнала: максимум меток по каждому устройству.
pub fn cursors_of(events: &[Event]) -> Cursors {
    let mut cursors = Cursors::default();
    for ev in events {
        cursors.observe(&ev.id);
    }
    cursors
}

/// События, которых нет у владельца `cursors`: строго новее курсора
/// своего устройства (события неизвестных устройств — целиком).
/// Результат отсортирован по id — готовое тело ответа `GET /v1/pull`.
///
/// Точность (без лишнего и без пропусков) гарантирована контрактом
/// append-only: чужой лог у получателя — всегда префикс по HLC-порядку,
/// значит «всё выше максимума» и «всё недостающее» — одно множество.
pub fn events_after(events: &[Event], cursors: &Cursors) -> Vec<Event> {
    let mut out: Vec<Event> = events
        .iter()
        .filter(|ev| !cursors.covers(&ev.id))
        .cloned()
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Влить удалённые события в локальный журнал ([`merge`]: множество по
/// id + сортировка) и вернуть, сколько НОВЫХ событий добавилось.
/// 0 — ничего нового (повторный push/pull идемпотентен).
pub fn apply_remote(events: &mut Vec<Event>, remote: &[Event]) -> usize {
    let before = events.len();
    merge(events, remote);
    // merge заодно схлопнул бы дубли в самом `events` — не уходим в минус.
    events.len().saturating_sub(before)
}

// ---------------------------------------------------------------------------
// Тесты
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::EventKind;

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

    #[test]
    fn cursors_of_keeps_max_per_device() {
        let log = vec![ev("a", 5, 0), ev("a", 5, 3), ev("b", 9, 0), ev("a", 2, 7)];
        let cursors = cursors_of(&log);
        assert_eq!(cursors.0.len(), 2);
        assert_eq!(
            cursors.0["a"],
            Hlc {
                wall_ms: 5,
                counter: 3,
                device: "a".into()
            },
            "максимум по (wall_ms, counter), а не по порядку появления"
        );
        assert_eq!((cursors.0["b"].wall_ms, cursors.0["b"].counter), (9, 0));
    }

    #[test]
    fn cursors_serialize_as_plain_device_map() {
        let mut cursors = Cursors::default();
        cursors.observe(&Hlc {
            wall_ms: 7,
            counter: 1,
            device: "aa".into(),
        });
        let json = serde_json::to_string(&cursors).unwrap();
        // Контракт протокола: прозрачная карта {device: Hlc}.
        assert_eq!(json, r#"{"aa":{"wall_ms":7,"counter":1,"device":"aa"}}"#);
        let back: Cursors = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cursors);
    }

    /// Ключевое свойство: для append-only пер-девайс логов курсоры дают
    /// ТОЧНУЮ выборку недостающего — ни пропусков, ни повторов.
    #[test]
    fn events_after_is_exact_complement_of_prefixes() {
        let d1: Vec<Event> = (0..5u64).map(|i| ev("d1", 10 + i, 0)).collect();
        let d2: Vec<Event> = (0..4u64).map(|i| ev("d2", 8 + 3 * i, 1)).collect();
        let mut full = Vec::new();
        merge(&mut full, &d1);
        merge(&mut full, &d2);
        // У получателя — префиксы логов (гарантия append-only).
        let mut have = Vec::new();
        merge(&mut have, &d1[..2]);
        merge(&mut have, &d2[..3]);

        let missing = events_after(&full, &cursors_of(&have));
        assert_eq!(missing.len(), full.len() - have.len(), "без пропусков");
        assert_eq!(
            apply_remote(&mut have, &missing),
            missing.len(),
            "без повторов: каждое присланное — новое"
        );
        assert_eq!(have, full, "получатель сошёлся с полным журналом");
    }

    #[test]
    fn events_after_is_strictly_greater_than_cursor() {
        // Метка ровно на курсоре уже есть у получателя; следующая по
        // счётчику в той же миллисекунде — уже нет.
        let log = vec![ev("a", 5, 0), ev("a", 5, 1)];
        let mut cursors = Cursors::default();
        cursors.observe(&log[0].id);
        assert_eq!(events_after(&log, &cursors), vec![log[1].clone()]);
    }

    #[test]
    fn unknown_device_is_sent_whole() {
        let log = vec![ev("new", 2, 0), ev("new", 1, 0)];
        let sent = events_after(&log, &Cursors::default());
        assert_eq!(sent.len(), 2);
        assert!(sent[0].id < sent[1].id, "выдача отсортирована по id");
    }

    #[test]
    fn own_cursors_cover_everything() {
        let log = vec![ev("a", 1, 0), ev("b", 2, 0)];
        assert!(events_after(&log, &cursors_of(&log)).is_empty());
        assert!(
            cursors_of(&[]).0.is_empty(),
            "пустой журнал — пустые курсоры"
        );
    }

    #[test]
    fn apply_remote_counts_new_and_is_idempotent() {
        let mut local = vec![ev("a", 1, 0)];
        let remote = vec![ev("a", 1, 0), ev("b", 2, 0)];
        assert_eq!(apply_remote(&mut local, &remote), 1, "дубль не считается");
        assert_eq!(apply_remote(&mut local, &remote), 0, "повтор — ноль новых");
        assert_eq!(local.len(), 2);
        assert!(
            local.windows(2).all(|w| w[0].id < w[1].id),
            "журнал отсортирован"
        );
    }

    /// Двусторонний обмен по курсорам: два офлайн-устройства сходятся к
    /// одинаковому журналу без диалогов (критерий M3 ТЗ).
    #[test]
    fn two_way_exchange_converges() {
        let mut a: Vec<Event> = (0..3u64).map(|i| ev("a", 10 + i, 0)).collect();
        let mut b: Vec<Event> = (0..2u64).map(|i| ev("b", 11 + i, 0)).collect();
        let to_b = events_after(&a, &cursors_of(&b));
        let to_a = events_after(&b, &cursors_of(&a));
        assert_eq!(apply_remote(&mut b, &to_b), 3);
        assert_eq!(apply_remote(&mut a, &to_a), 2);
        assert_eq!(a, b, "журналы сошлись");
        assert_eq!(a.len(), 5);
    }
}
