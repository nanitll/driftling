# dist/ — файлы установки

## io.github.nanitll.driftling.desktop — идентичность приложения (F1)

`io.github.nanitll.driftling` — зафиксированный app-id проекта
(`driftling_core::APP_ID`, правила из docs/SHIPPING.md). Им подписаны все
окна и слои: namespace Wayland-оверлея, WM_CLASS X11-оверлея, app_id окна
настроек и Id трея. Чтобы KDE/GNOME показывали фирменную иконку (а не
generic), базовое имя .desktop-файла и имя иконки обязаны совпадать с
app-id — поэтому файл и иконки называются именно так и переименовывать их
нельзя.

Установка .desktop и иконок (иконки лежат в `assets/icons/`,
`-NN` в имени — размер, файл без суффикса — 128 px):

```sh
install -Dm644 dist/io.github.nanitll.driftling.desktop \
  ~/.local/share/applications/io.github.nanitll.driftling.desktop
for px in 32 48 64; do
  install -Dm644 assets/icons/io.github.nanitll.driftling-$px.png \
    ~/.local/share/icons/hicolor/${px}x${px}/apps/io.github.nanitll.driftling.png
done
install -Dm644 assets/icons/io.github.nanitll.driftling.png \
  ~/.local/share/icons/hicolor/128x128/apps/io.github.nanitll.driftling.png
```

`Exec=driftling` рассчитывает на бинарь в `PATH` (`~/.local/bin` — см.
установку демона ниже). Кэши меню/иконок обычно подхватывают файлы сами;
если иконка не появилась — `update-desktop-database ~/.local/share/applications`
и перелогин (в KDE — `kbuildsycoca6`).

Удаление:

```sh
rm ~/.local/share/applications/io.github.nanitll.driftling.desktop
rm ~/.local/share/icons/hicolor/{32x32,48x48,64x64,128x128}/apps/io.github.nanitll.driftling.png
```

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
