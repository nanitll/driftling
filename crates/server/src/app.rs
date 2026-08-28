//! HTTP API v1 (контракт фазы E, ТЗ §3.5).
//!
//! Все ручки, кроме `/v1/health`, требуют `Authorization: Bearer <токен>`;
//! неизвестный токен = 401. События — непрозрачный JSON: сервер проверяет
//! только ключ `id` (HLC) и хранит/отдаёт исходные объекты байт-в-байт по
//! смыслу — новые виды событий будущих клиентов проходят насквозь.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{Cursor, Db};

/// Потолок TTL lease: heartbeat раз в ~10 с, час — уже явно кривой клиент.
const MAX_LEASE_TTL_S: u64 = 3_600;

pub struct AppState {
    pub db: Db,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/push", post(push))
        .route("/v1/pull", get(pull).post(pull))
        .route("/v1/lease", post(lease_claim).delete(lease_release))
        .with_state(state)
}

/// Ошибка API: статус + человекочитаемая причина простым текстом.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn internal(err: anyhow::Error) -> ApiError {
    log::error!("внутренняя ошибка: {err:#}");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_string(),
    )
}

/// `Authorization: Bearer <token>` → id аккаунта; иначе 401.
fn auth(state: &AppState, headers: &HeaderMap) -> Result<i64, ApiError> {
    let unauthorized = || ApiError(StatusCode::UNAUTHORIZED, "invalid token".to_string());
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)?;
    state
        .db
        .auth(token.trim())
        .map_err(internal)?
        .ok_or_else(unauthorized)
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// -- /v1/health --------------------------------------------------------------

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

// -- /v1/push ----------------------------------------------------------------

#[derive(Deserialize)]
struct PushReq {
    /// Устройство-отправитель (информационное: ключом события служит
    /// `id.device` самого события — после merge клиент может переслать
    /// и чужие записи, это безвредно и идемпотентно).
    #[allow(dead_code)]
    device: String,
    events: Vec<Value>,
}

#[derive(Serialize)]
struct PushResp {
    accepted: u32,
}

/// Ключ события из его `id` (HLC): `(device, wall_ms, counter)`.
fn event_key(ev: &Value) -> Result<(String, u64, u16), String> {
    let id = ev.get("id").ok_or("событие без id")?;
    let wall_ms = id
        .get("wall_ms")
        .and_then(Value::as_u64)
        .ok_or("id.wall_ms не целое неотрицательное")?;
    if wall_ms > i64::MAX as u64 {
        return Err("id.wall_ms за пределами разумного".to_string());
    }
    let counter = id
        .get("counter")
        .and_then(Value::as_u64)
        .filter(|&c| c <= u16::MAX as u64)
        .ok_or("id.counter не в диапазоне u16")?;
    let device = id
        .get("device")
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty())
        .ok_or("id.device пуст")?;
    Ok((device.to_string(), wall_ms, counter as u16))
}

async fn push(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<PushResp>, ApiError> {
    let account = auth(&state, &headers)?;
    let req: PushReq =
        serde_json::from_slice(&body).map_err(|e| bad_request(format!("bad push body: {e}")))?;
    // Сначала валидируем ВЕСЬ батч, потом пишем: частично принятый push
    // заставил бы клиента гадать, что именно легло.
    let mut batch = Vec::with_capacity(req.events.len());
    for (i, ev) in req.events.iter().enumerate() {
        let key = event_key(ev).map_err(|why| bad_request(format!("event #{i}: {why}")))?;
        let payload = serde_json::to_string(ev).map_err(|e| bad_request(e.to_string()))?;
        batch.push((key, payload));
    }
    let accepted = batch.len() as u32;
    for ((device, wall_ms, counter), payload) in batch {
        state
            .db
            .upsert_event(account, &device, wall_ms, counter, &payload)
            .map_err(internal)?;
    }
    Ok(Json(PushResp { accepted }))
}

// -- /v1/pull ----------------------------------------------------------------

/// Курсор в проводе — Hlc целиком; внутреннее поле `device` избыточно
/// (ключ карты главнее) и потому необязательно.
#[derive(Deserialize)]
struct WireCursor {
    wall_ms: u64,
    counter: u16,
    #[allow(dead_code)]
    #[serde(default)]
    device: Option<String>,
}

#[derive(Deserialize)]
struct PullBody {
    cursors: HashMap<String, WireCursor>,
}

#[derive(Serialize)]
struct PullResp {
    events: Vec<Value>,
}

/// Курсоры принимаются телом ИЛИ query (`?cursors=<urlencoded JSON>`) —
/// GET с телом не всякий клиент умеет. Тело — карта `{device: Hlc}`,
/// голая или завёрнутая в `{"cursors": …}`; пустое тело = отдать всё.
fn parse_cursors(
    query: &HashMap<String, String>,
    body: &Bytes,
) -> Result<HashMap<String, WireCursor>, ApiError> {
    let text: &str = match query.get("cursors") {
        Some(q) => q,
        None if body.is_empty() => return Ok(HashMap::new()),
        None => std::str::from_utf8(body).map_err(|_| bad_request("body is not utf-8"))?,
    };
    if text.trim().is_empty() {
        return Ok(HashMap::new());
    }
    if let Ok(wrapped) = serde_json::from_str::<PullBody>(text) {
        return Ok(wrapped.cursors);
    }
    serde_json::from_str(text).map_err(|e| bad_request(format!("bad cursors: {e}")))
}

async fn pull(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Json<PullResp>, ApiError> {
    let account = auth(&state, &headers)?;
    let cursors = parse_cursors(&query, &body)?;
    let stored = state
        .db
        .events_after(account, |device| {
            cursors.get(device).map(|c| Cursor {
                wall_ms: c.wall_ms,
                counter: c.counter,
            })
        })
        .map_err(internal)?;
    let mut events = Vec::with_capacity(stored.len());
    for ev in stored {
        // Payload писали мы же из валидного JSON; ошибка = порча базы.
        let value = serde_json::from_str(&ev.payload)
            .map_err(|e| internal(anyhow::anyhow!("порченый payload в базе: {e}")))?;
        events.push(value);
    }
    Ok(Json(PullResp { events }))
}

// -- /v1/lease ---------------------------------------------------------------

#[derive(Deserialize)]
struct LeaseReq {
    device: String,
    ttl_s: u64,
}

#[derive(Serialize)]
struct LeaseResp {
    granted: bool,
    holder: String,
}

async fn lease_claim(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<LeaseResp>, ApiError> {
    let account = auth(&state, &headers)?;
    let req: LeaseReq =
        serde_json::from_slice(&body).map_err(|e| bad_request(format!("bad lease body: {e}")))?;
    if req.device.is_empty() {
        return Err(bad_request("device пуст"));
    }
    let ttl = req.ttl_s.clamp(1, MAX_LEASE_TTL_S);
    let answer = state
        .db
        .lease_claim(account, &req.device, ttl, now_s())
        .map_err(internal)?;
    Ok(Json(LeaseResp {
        granted: answer.granted,
        holder: answer.holder,
    }))
}

#[derive(Deserialize)]
struct LeaseReleaseReq {
    device: String,
}

#[derive(Serialize)]
struct LeaseReleaseResp {
    released: bool,
}

async fn lease_release(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<LeaseReleaseResp>, ApiError> {
    let account = auth(&state, &headers)?;
    let req: LeaseReleaseReq =
        serde_json::from_slice(&body).map_err(|e| bad_request(format!("bad lease body: {e}")))?;
    let released = state
        .db
        .lease_release(account, &req.device)
        .map_err(internal)?;
    Ok(Json(LeaseReleaseResp { released }))
}

// ---------------------------------------------------------------------------
// Тесты уровня обработчиков (tower::ServiceExt::oneshot, без сети)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    /// Приложение с базой в памяти + токен единственного аккаунта.
    fn app() -> (Router, String) {
        let db = Db::open_in_memory().unwrap();
        let (_, token) = db.account_add("test").unwrap();
        (router(Arc::new(AppState { db })), token)
    }

    async fn call(
        app: &Router,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(v) => req
                .header("content-type", "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, value)
    }

    fn event(device: &str, wall_ms: u64, counter: u16, kind: Value) -> Value {
        json!({
            "id": { "wall_ms": wall_ms, "counter": counter, "device": device },
            "kind": kind,
        })
    }

    fn fed(device: &str, wall_ms: u64) -> Value {
        event(device, wall_ms, 0, json!({ "type": "Fed", "treat": false }))
    }

    #[tokio::test]
    async fn health_needs_no_auth() {
        let (app, _) = app();
        let (status, body) = call(&app, Method::GET, "/v1/health", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn unknown_or_missing_token_is_401() {
        let (app, _) = app();
        for (method, uri) in [
            (Method::POST, "/v1/push"),
            (Method::GET, "/v1/pull"),
            (Method::POST, "/v1/lease"),
            (Method::DELETE, "/v1/lease"),
        ] {
            let (status, _) = call(&app, method.clone(), uri, None, None).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} без токена"
            );
            let (status, _) = call(&app, method, uri, Some("wrong"), None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} с левым токеном");
        }
    }

    #[tokio::test]
    async fn push_is_idempotent_and_pull_returns_sorted() {
        let (app, token) = app();
        let t = token.as_str();
        let batch = json!({
            "device": "laptop",
            "events": [fed("laptop", 200), fed("laptop", 100)],
        });
        let (status, body) =
            call(&app, Method::POST, "/v1/push", Some(t), Some(batch.clone())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["accepted"], 2);
        // Повторный push того же батча — тот же ответ, дублей нет.
        let (status, body) = call(&app, Method::POST, "/v1/push", Some(t), Some(batch)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["accepted"], 2);

        let (status, body) = call(&app, Method::GET, "/v1/pull", Some(t), None).await;
        assert_eq!(status, StatusCode::OK);
        let events = body["events"].as_array().unwrap();
        assert_eq!(events.len(), 2, "идемпотентность: {body}");
        // Отсортировано по HLC и payload сохранён как есть.
        assert_eq!(events[0]["id"]["wall_ms"], 100);
        assert_eq!(events[1]["id"]["wall_ms"], 200);
        assert_eq!(events[0]["kind"]["type"], "Fed");
    }

    #[tokio::test]
    async fn unknown_event_kinds_pass_through_untouched() {
        // Совместимость вперёд: сервер не разбирает kind.
        let (app, token) = app();
        let t = token.as_str();
        let exotic = event(
            "laptop",
            5,
            1,
            json!({ "type": "Teleported", "to": "лунная база", "extra": [1, 2] }),
        );
        let batch = json!({ "device": "laptop", "events": [exotic.clone()] });
        let (status, _) = call(&app, Method::POST, "/v1/push", Some(t), Some(batch)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = call(&app, Method::GET, "/v1/pull", Some(t), None).await;
        assert_eq!(body["events"][0], exotic);
    }

    #[tokio::test]
    async fn pull_filters_by_cursors_query_and_body() {
        let (app, token) = app();
        let t = token.as_str();
        let batch = json!({
            "device": "laptop",
            "events": [
                fed("laptop", 100),
                event("laptop", 100, 1, json!({ "type": "Petted" })),
                fed("laptop", 300),
                fed("desktop", 200),
            ],
        });
        let (status, _) = call(&app, Method::POST, "/v1/push", Some(t), Some(batch)).await;
        assert_eq!(status, StatusCode::OK);

        // Курсор laptop=(100,1): его событие (300,0) новее, (100,0) и
        // (100,1) — нет; desktop без курсора отдаётся целиком.
        let cursors = json!({
            "laptop": { "wall_ms": 100, "counter": 1, "device": "laptop" },
        });
        // Вариант 1: тело (голая карта).
        let (status, body) = call(
            &app,
            Method::GET,
            "/v1/pull",
            Some(t),
            Some(cursors.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let got: Vec<(u64, &str)> = body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["id"]["wall_ms"].as_u64().unwrap(),
                    e["id"]["device"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(got, vec![(200, "desktop"), (300, "laptop")], "{body}");

        // Вариант 2: тело в обёртке {"cursors": …}.
        let wrapped = json!({ "cursors": cursors });
        let (_, body2) = call(&app, Method::GET, "/v1/pull", Some(t), Some(wrapped)).await;
        assert_eq!(body2, body);

        // Вариант 3: query-параметр.
        let uri = format!("/v1/pull?cursors={}", urlencode(&cursors.to_string()));
        let (_, body3) = call(&app, Method::GET, &uri, Some(t), None).await;
        assert_eq!(body3, body);
    }

    /// Мини-urlencode для тестов (только то, что встречается в JSON).
    fn urlencode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    #[tokio::test]
    async fn malformed_event_is_400_and_nothing_is_stored() {
        let (app, token) = app();
        let t = token.as_str();
        let batch = json!({
            "device": "laptop",
            "events": [fed("laptop", 1), json!({ "kind": { "type": "Fed" } })],
        });
        let (status, body) = call(&app, Method::POST, "/v1/push", Some(t), Some(batch)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        // Батч атомарен: валидное событие из битого батча тоже не легло.
        let (_, body) = call(&app, Method::GET, "/v1/pull", Some(t), None).await;
        assert_eq!(body["events"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn accounts_are_isolated() {
        let db = Db::open_in_memory().unwrap();
        let (_, token_a) = db.account_add("a").unwrap();
        let (_, token_b) = db.account_add("b").unwrap();
        let app = router(Arc::new(AppState { db }));

        let batch = json!({ "device": "laptop", "events": [fed("laptop", 1)] });
        let (status, _) = call(&app, Method::POST, "/v1/push", Some(&token_a), Some(batch)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = call(&app, Method::GET, "/v1/pull", Some(&token_b), None).await;
        assert_eq!(
            body["events"].as_array().unwrap().len(),
            0,
            "чужого не видно"
        );
        // И lease у аккаунтов свои.
        let claim = json!({ "device": "laptop", "ttl_s": 30 });
        let (_, a) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(&token_a),
            Some(claim.clone()),
        )
        .await;
        assert_eq!(a["granted"], true);
        let (_, b) = call(&app, Method::POST, "/v1/lease", Some(&token_b), Some(claim)).await;
        assert_eq!(b["granted"], true, "lease аккаунта b независим");
    }

    #[tokio::test]
    async fn lease_takeover_newest_claim_wins() {
        let (app, token) = app();
        let t = token.as_str();
        let claim = |device: &str| json!({ "device": device, "ttl_s": 60 });

        // Ноутбук берёт lease и спокойно heartbeat-ит.
        let (_, body) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(t),
            Some(claim("laptop")),
        )
        .await;
        assert_eq!(
            (body["granted"].clone(), body["holder"].clone()),
            (json!(true), json!("laptop"))
        );
        let (_, body) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(t),
            Some(claim("laptop")),
        )
        .await;
        assert_eq!(body["granted"], true, "heartbeat держателя");

        // Десктоп claim-ит: newest-wins, забирает СРАЗУ.
        let (_, body) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(t),
            Some(claim("desktop")),
        )
        .await;
        assert_eq!(
            (body["granted"].clone(), body["holder"].clone()),
            (json!(true), json!("desktop"))
        );

        // Следующий heartbeat ноутбука: узнаёт о потере («питомец убежал»).
        let (_, body) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(t),
            Some(claim("laptop")),
        )
        .await;
        assert_eq!(
            (body["granted"].clone(), body["holder"].clone()),
            (json!(false), json!("desktop"))
        );

        // А вот повторный запрос ноутбука — уже свежий claim: снова его.
        let (_, body) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(t),
            Some(claim("laptop")),
        )
        .await;
        assert_eq!(
            (body["granted"].clone(), body["holder"].clone()),
            (json!(true), json!("laptop"))
        );
    }

    #[tokio::test]
    async fn lease_release_frees_the_slot() {
        let (app, token) = app();
        let t = token.as_str();
        let (_, body) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(t),
            Some(json!({ "device": "laptop", "ttl_s": 60 })),
        )
        .await;
        assert_eq!(body["granted"], true);

        // Отпустить может только держатель.
        let (_, body) = call(
            &app,
            Method::DELETE,
            "/v1/lease",
            Some(t),
            Some(json!({ "device": "desktop" })),
        )
        .await;
        assert_eq!(body["released"], false);
        let (_, body) = call(
            &app,
            Method::DELETE,
            "/v1/lease",
            Some(t),
            Some(json!({ "device": "laptop" })),
        )
        .await;
        assert_eq!(body["released"], true);

        // Слот свободен: другой берёт без пометки ousted.
        let (_, body) = call(
            &app,
            Method::POST,
            "/v1/lease",
            Some(t),
            Some(json!({ "device": "desktop", "ttl_s": 60 })),
        )
        .await;
        assert_eq!(body["granted"], true);
    }
}
