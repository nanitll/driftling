#!/usr/bin/env bash
# Headless-смоук демона (ТД-27, ROADMAP A8): поднять композитор, запустить
# release-демон, убедиться, что он жив, получил геометрию выхода, отвечает
# по IPC и укладывается в энергобюджет в покое (ТЗ §7), затем штатно выйти.
#
# Запуск в CI (ubuntu-latest):
#   dbus-run-session -- ci/smoke.sh
# Локальная проверка на живой Wayland-сессии (без sway):
#   SMOKE_COMPOSITOR=session ci/smoke.sh
#
# Переменные:
#   DRIFTLING_BIN     — путь к бинарю демона (по умолчанию target/release/driftling)
#   SMOKE_COMPOSITOR  — sway (по умолчанию) | session (текущая Wayland-сессия)
#   SMOKE_CPU_SECONDS — окно замера CPU в покое, сек (по умолчанию 30)
#   SMOKE_CPU_LIMIT   — потолок среднего CPU в процентах (по умолчанию 2.0)
#   SMOKE_RSS_LIMIT   — потолок RSS демона в МБ (по умолчанию 60, ТЗ §7)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${DRIFTLING_BIN:-$REPO_ROOT/target/release/driftling}"
COMPOSITOR="${SMOKE_COMPOSITOR:-sway}"
CPU_SECONDS="${SMOKE_CPU_SECONDS:-30}"
CPU_LIMIT="${SMOKE_CPU_LIMIT:-2.0}"
RSS_LIMIT="${SMOKE_RSS_LIMIT:-60}"

[ -x "$BIN" ] || { echo "FAIL: демон не собран: $BIN (нужен cargo build --release)"; exit 1; }

WORK="$(mktemp -d /tmp/driftling-smoke.XXXXXX)"
DAEMON_LOG="$WORK/daemon.log"
SWAY_LOG="$WORK/sway.log"
DAEMON_PID=""
SWAY_PID=""

cleanup() {
    # Демона глушим всегда — даже при провале теста он не должен пережить скрипт.
    if [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in $(seq 1 50); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    if [ -n "$SWAY_PID" ] && kill -0 "$SWAY_PID" 2>/dev/null; then
        kill "$SWAY_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*"
    echo "--- лог демона ---"; cat "$DAEMON_LOG" 2>/dev/null || true
    [ -f "$SWAY_LOG" ] && { echo "--- лог sway ---"; cat "$SWAY_LOG"; }
    exit 1
}

# Изоляция от окружения пользователя: свои XDG-каталоги. Данные/конфиг —
# чистый «первый запуск» (summoned=true по умолчанию); рантайм-каталог свой,
# чтобы сокет/лок не столкнулись с реальным демоном.
export XDG_DATA_HOME="$WORK/data"
export XDG_CONFIG_HOME="$WORK/config"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME"
REAL_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}"
export XDG_RUNTIME_DIR="$WORK/runtime"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
# env_logger, а не journald, даже если stderr смотрит в журнал.
unset JOURNAL_STREAM
export RUST_LOG=info
# Детерминированная локаль: вывод ctl локализован (ТД-30), под C уходит
# в английский fallback — сверяемся с английскими строками.
export LC_ALL=C
unset LANG LANGUAGE

case "$COMPOSITOR" in
    sway)
        command -v sway >/dev/null || fail "sway не установлен"
        # Headless-wlroots: без GPU, без libinput, без seatd.
        WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
            sway --config /dev/null --verbose >"$SWAY_LOG" 2>&1 &
        SWAY_PID=$!
        # sway пишет имя своего дисплея в лог; ждём и подхватываем.
        WAYLAND_DISPLAY=""
        for _ in $(seq 1 100); do
            WAYLAND_DISPLAY="$(sed -n "s/.*Running compositor on wayland display '\([^']*\)'.*/\1/p" "$SWAY_LOG" | head -1)"
            [ -n "$WAYLAND_DISPLAY" ] && break
            kill -0 "$SWAY_PID" 2>/dev/null || fail "sway умер на старте"
            sleep 0.1
        done
        [ -n "$WAYLAND_DISPLAY" ] || fail "sway не сообщил имя wayland-дисплея за 10 с"
        export WAYLAND_DISPLAY
        echo "ok: sway (headless) на $WAYLAND_DISPLAY"
        ;;
    session)
        # Локальный прогон: цепляемся к живому композитору. Сокет дисплея
        # лежит в реальном рантайм-каталоге — линкуем его в наш изолированный.
        [ -n "${WAYLAND_DISPLAY:-}" ] || fail "SMOKE_COMPOSITOR=session требует WAYLAND_DISPLAY"
        [ -n "$REAL_RUNTIME_DIR" ] || fail "нет XDG_RUNTIME_DIR у сессии"
        case "$WAYLAND_DISPLAY" in
            /*) ;; # абсолютный путь — линк не нужен
            *) ln -s "$REAL_RUNTIME_DIR/$WAYLAND_DISPLAY" "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ;;
        esac
        echo "ok: текущая сессия ($WAYLAND_DISPLAY)"
        ;;
    *)
        fail "неизвестный SMOKE_COMPOSITOR=$COMPOSITOR (sway|session)"
        ;;
esac

# --- Запуск демона ------------------------------------------------------
"$BIN" >"$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!
START_TS=$(date +%s)

# Демон обязан получить геометрию выхода (строка «выход: WxH» в логе).
GEOMETRY=""
for _ in $(seq 1 150); do
    GEOMETRY="$(grep -o 'выход: [0-9]*x[0-9]*' "$DAEMON_LOG" | head -1 || true)"
    [ -n "$GEOMETRY" ] && break
    kill -0 "$DAEMON_PID" 2>/dev/null || fail "демон умер до геометрии выхода"
    sleep 0.1
done
[ -n "$GEOMETRY" ] || fail "за 15 с в логе не появилась геометрия выхода"
echo "ok: $GEOMETRY"

# 10 секунд непрерывной работы — демон всё ещё жив.
ELAPSED=$(( $(date +%s) - START_TS ))
[ "$ELAPSED" -lt 10 ] && sleep $(( 10 - ELAPSED ))
kill -0 "$DAEMON_PID" 2>/dev/null || fail "демон умер в первые 10 с"
echo "ok: жив после 10 с"

# IPC: status обязан ответить (питомец призван — «первый запуск»).
STATUS_OUT="$("$BIN" ctl status)" || fail "ctl status не ответил"
echo "ok: ctl status → $STATUS_OUT"
case "$STATUS_OUT" in
    *"pets: 1"*) ;;
    *) fail "неожиданный status (ждали «pets: 1»): $STATUS_OUT" ;;
esac

# --- Энергобюджет в покое (ТЗ §7, ТД-3) ----------------------------------
# Питомец убран → демон обязан почти не есть CPU. Замер по /proc/PID/stat
# (utime+stime всего процесса, включая потоки); порог намеренно щадящий
# для шумных CI-раннеров.
"$BIN" ctl dismiss >/dev/null || fail "ctl dismiss не ответил"
cpu_ticks() { awk '{print $14 + $15}' "/proc/$DAEMON_PID/stat"; }
TICKS_BEFORE=$(cpu_ticks)
sleep "$CPU_SECONDS"
kill -0 "$DAEMON_PID" 2>/dev/null || fail "демон умер во время замера CPU"
TICKS_AFTER=$(cpu_ticks)
CLK_TCK=$(getconf CLK_TCK)
CPU_PCT=$(awk -v d=$((TICKS_AFTER - TICKS_BEFORE)) -v hz="$CLK_TCK" -v s="$CPU_SECONDS" \
    'BEGIN { printf "%.2f", 100.0 * d / hz / s }')
echo "ok: CPU в покое за ${CPU_SECONDS} с = ${CPU_PCT}%"
awk -v p="$CPU_PCT" -v lim="$CPU_LIMIT" 'BEGIN { exit !(p <= lim) }' \
    || fail "энергобюджет превышен: ${CPU_PCT}% > ${CPU_LIMIT}%"

# Память (ТЗ §7: RSS < 60 МБ). Берём пик (VmHWM) — строже мгновенного VmRSS.
RSS_KB=$(awk '/^VmHWM:/ {print $2}' "/proc/$DAEMON_PID/status")
RSS_MB=$(awk -v kb="$RSS_KB" 'BEGIN { printf "%.1f", kb / 1024 }')
echo "ok: пиковый RSS = ${RSS_MB} МБ"
awk -v m="$RSS_MB" -v lim="$RSS_LIMIT" 'BEGIN { exit !(m <= lim) }' \
    || fail "бюджет памяти превышен: ${RSS_MB} МБ > ${RSS_LIMIT} МБ"

# --- Штатное завершение ---------------------------------------------------
"$BIN" ctl quit >/dev/null || fail "ctl quit не ответил"
for _ in $(seq 1 100); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
if kill -0 "$DAEMON_PID" 2>/dev/null; then
    fail "демон не завершился за 10 с после ctl quit"
fi
set +e
wait "$DAEMON_PID"
EXIT_CODE=$?
set -e
DAEMON_PID=""
[ "$EXIT_CODE" -eq 0 ] || fail "демон завершился с кодом $EXIT_CODE"
echo "ok: штатный выход (код 0)"

echo "SMOKE PASSED"
