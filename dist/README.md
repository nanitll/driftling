# dist/ — файлы установки

## driftling.service — автозапуск демона (systemd user unit)

Основной способ автозапуска: systemd перезапустит демон после краша
(`Restart=on-failure`) и корректно остановит его при выходе из сессии
(демон сохраняет pet.json по SIGTERM). Логи при этом идут в journald
с идентификатором `driftling`.

Установка:

```sh
# 1. Собрать и положить бинарь в ~/.local/bin (путь из ExecStart)
cargo build --release
install -Dm755 target/release/driftling ~/.local/bin/driftling

# 2. Установить и включить юнит
install -Dm644 dist/driftling.service ~/.config/systemd/user/driftling.service
systemctl --user daemon-reload
systemctl --user enable --now driftling.service
```

Проверка и логи:

```sh
systemctl --user status driftling.service
journalctl --user -t driftling -f
```

Удаление:

```sh
systemctl --user disable --now driftling.service
rm ~/.config/systemd/user/driftling.service ~/.local/bin/driftling
```

На системах без systemd окно настроек создаёт XDG autostart
(`~/.config/autostart/driftling.desktop`) как фолбэк; тумблер «Автозапуск»
сам выбирает systemd-юнит, если тот установлен.
