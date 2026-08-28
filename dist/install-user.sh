#!/usr/bin/env bash
# Установка Driftling для текущего пользователя (~/.local): бинари, иконки,
# ярлык меню приложений. Запускать из корня репозитория после
# `cargo build --release`. Удаление — install-user.sh --uninstall.
set -euo pipefail

APP_ID="io.github.nanitll.driftling"
BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$BIN/driftling" "$BIN/driftling-settings" "$APPS/$APP_ID.desktop"
    for s in 128 64 48 32; do rm -f "$ICONS/${s}x${s}/apps/$APP_ID.png"; done
    update-desktop-database "$APPS" 2>/dev/null || true
    echo "Driftling удалён из ~/.local (данные питомца в ~/.local/share/driftling не тронуты)"
    exit 0
fi

install -Dm755 target/release/driftling "$BIN/driftling"
install -Dm755 target/release/driftling-settings "$BIN/driftling-settings"
for s in 128 64 48 32; do
    suffix=""; [[ $s != 128 ]] && suffix="-$s"
    install -Dm644 "assets/icons/$APP_ID$suffix.png" "$ICONS/${s}x${s}/apps/$APP_ID.png"
done

mkdir -p "$APPS"
sed -e "s|^Exec=driftling$|Exec=$BIN/driftling|" "dist/$APP_ID.desktop" > "$APPS/$APP_ID.desktop"
cat >> "$APPS/$APP_ID.desktop" <<EOF
Actions=settings;dismiss;

[Desktop Action settings]
Name=Settings
Name[ru]=Настройки
Exec=$BIN/driftling-settings

[Desktop Action dismiss]
Name=Hide the pet
Name[ru]=Убрать питомца
Exec=$BIN/driftling ctl dismiss
EOF

update-desktop-database "$APPS" 2>/dev/null || true
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
echo "Driftling установлен: ищите «Driftling» в меню приложений"
