# driftling-server — свой синк-сервер за 5 минут

Один и тот же питомец на всех устройствах (ТЗ §3.5): сервер хранит
append-only журнал событий ухода (push/pull) и решает, на каком устройстве
питомец виден сейчас (lease присутствия, newest-wins). Один статический
бинарь + SQLite; TLS вешает reverse-proxy.

## Вариант 1: Docker (рекомендуется)

```bash
git clone https://github.com/nanitll/driftling && cd driftling/dist/server
docker compose -f docker-compose.example.yml up -d --build

# Создать аккаунт — токен печатается ОДИН раз, сохраните его:
docker compose -f docker-compose.example.yml exec server \
    driftling-server --config /etc/driftling-server.toml account add home
```

Проверка: `curl http://127.0.0.1:8787/v1/health` → `{"status":"ok",…}`.

## Вариант 2: голый бинарь + systemd

```bash
cargo build --release -p driftling-server
sudo install -m755 target/release/driftling-server /usr/local/bin/

# Конфиг: база в /var/lib/driftling-server (её создаст systemd StateDirectory).
printf 'listen = "127.0.0.1:8787"\ndb = "/var/lib/driftling-server/driftling-server.sqlite3"\n' \
    | sudo tee /etc/driftling-server.toml

sudo cp dist/server/driftling-server.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now driftling-server

# Аккаунт (команде нужны права на базу — под systemd-юзером):
sudo systemd-run -qtP -p DynamicUser=yes -p StateDirectory=driftling-server \
    /usr/local/bin/driftling-server --config /etc/driftling-server.toml account add home
```

## TLS: reverse-proxy

Сервер намеренно говорит только плоским HTTP на localhost. Наружу его
выставляет прокси; для Caddy достаточно двух строк в `Caddyfile`:

```
sync.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

## Настройка клиента

На каждом устройстве в конфиге Driftling указываются адрес и токен
(один аккаунт = один питомец на все ваши устройства):

```toml
[sync]
address = "https://sync.example.com"
token = "<токен из account add>"
```

## Администрирование

```bash
driftling-server --config <toml> account add <имя>   # новый аккаунт + токен (один раз)
driftling-server --config <toml> account list        # id, имя, счётчики событий/токенов
```

Токены в базе не хранятся — только их sha256-хэши; потерянный токен не
восстанавливается (создайте новый аккаунт или добавьте строку в `tokens`
вручную). Бэкап сервера = бэкап одного файла SQLite (снимайте копию через
`sqlite3 … ".backup"` или при остановленном сервере — не копируйте живой
WAL-файл простым `cp`).

## Протокол (HTTP API v1)

Все ручки, кроме `health`, требуют `Authorization: Bearer <токен>`;
неизвестный токен → 401. События — самоописываемый JSON журнала
(`{"id":{"wall_ms":…,"counter":…,"device":"…"},"kind":{…}}`); сервер
проверяет только `id` и хранит события как есть, поэтому совместим вперёд
с новыми видами событий.

| Метод и путь | Тело | Ответ |
|---|---|---|
| `GET /v1/health` | — | `{"status":"ok","version":"…"}` |
| `POST /v1/push` | `{"device":"…","events":[Event…]}` | `{"accepted":N}` — идемпотентный upsert по `id`, повторная отправка безвредна; битый батч → 400 целиком |
| `GET /v1/pull` | курсоры `{device_id: Hlc}` телом или `?cursors=<JSON>` | `{"events":[…]}` — все события аккаунта новее курсоров, отсортированы по `(wall_ms, counter, device)`; без курсоров — всё |
| `POST /v1/lease` | `{"device":"…","ttl_s":N}` | `{"granted":bool,"holder":"…"}` |
| `DELETE /v1/lease` | `{"device":"…"}` | `{"released":bool}` |

Семантика lease (питомец виден на одном устройстве, newest-wins):

- свободный/истёкший lease выдаётся сразу; держатель продлевает его тем же
  запросом (heartbeat);
- claim другого устройства **забирает lease немедленно** (`granted:true`);
- смещённый держатель узнаёт о потере на своём следующем heartbeat:
  `granted:false` + кто держит («питомец убежал» на этом экране), а его
  последующий запрос уже считается свежим claim и снова заберёт lease.
