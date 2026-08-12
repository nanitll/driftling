# driftling: вывод CLI (ctl/doctor) и IPC-ошибки демона.

# --- Справка CLI ---
cli-about = Настольный тамагочи, который дрейфует между вашими устройствами
cli-about-ctl = Управление запущенным демоном
cli-about-settings = Открыть окно настроек (запускает driftling-settings)
cli-about-summon = Позвать питомца на экран
cli-about-dismiss = Убрать питомца с экрана (демон продолжает работать)
cli-about-status = Показать состояние
cli-about-info = Карточка питомца: имя, состояние, характеристики
cli-about-reload = Перечитать конфиг и применить на лету
cli-about-quit = Остановить демон
cli-about-doctor = Диагностика: окружение, сокет, файлы, автозапуск (работает без демона)

# --- Вывод ctl ---
ctl-ok = ок
ctl-status = питомцев: { $pets }, состояние: { $state }, аптайм: { $uptime }s
ctl-petinfo = { $name }: { $state }, аптайм { $uptime }s, { $attributes }
state-dismissed = убран с экрана
settings-launch-failed = не удалось запустить { $program }: { $error }
settings-exited-with-error = driftling-settings завершился с ошибкой

# --- doctor ---
doctor-title = driftling doctor — диагностика окружения
doctor-wayland-ok = WAYLAND_DISPLAY = { $value }
doctor-wayland-missing = WAYLAND_DISPLAY не задан — вне Wayland-сессии оверлей не поднимется
doctor-runtime-dir = XDG_RUNTIME_DIR = { $path }
doctor-daemon-answering = демон отвечает: протокол v{ $version } совпадает, питомцев { $pets }, состояние { $state }, аптайм { $uptime }s
doctor-daemon-unexpected-reply = демон отвечает (неожиданный ответ на Status: { $reply })
doctor-daemon-not-answering = сокет { $path } есть, но демон не отвечает: { $error }
doctor-socket-missing = сокета { $path } нет — демон не запущен
doctor-socket-uncheckable = { $error } — сокет проверить нельзя
doctor-pet-ok = pet.json: питомец «{ $name }» ({ $path })
doctor-pet-missing = pet.json ещё не создан ({ $path }) — появится после первого запуска демона
doctor-pet-unreadable = pet.json не читается ({ $path }): { $error }
doctor-config-ok = config.toml разбирается ({ $path })
doctor-config-broken = config.toml битый ({ $path }): { $error }
doctor-config-missing = config.toml отсутствует ({ $path }) — действуют настройки по умолчанию
doctor-autostart-systemd = автозапуск: systemd-юнит driftling.service включён
doctor-autostart-desktop = автозапуск: найден { $path }
doctor-autostart-none = автозапуск не настроен (ни systemd-юнита, ни autostart/driftling.desktop)
doctor-verdict-ok = Вердикт: всё в порядке — демон работает и отвечает.
doctor-verdict-should-run = Вердикт: демон должен работать, но не отвечает. Запустите `driftling` (или `systemctl --user restart driftling`).
doctor-verdict-not-running = Вердикт: демон не запущен, автозапуск не настроен. Запустите `driftling`.

# --- IPC-ошибки демона (видны в ctl и окне настроек) ---
daemon-shutting-down = демон завершается
daemon-reply-timeout = демон не ответил вовремя
daemon-output-not-ready = выход ещё не готов (нет геометрии)
daemon-summoned-not-saved = питомец призван, но pet.json не сохранён: { $error }
daemon-dismissed-not-saved = питомец убран, но pet.json не сохранён: { $error }
daemon-applied-not-saved = применено, но pet.json не сохранён: { $error }
daemon-config-unreadable = config.toml не прочитан: { $error }

# --- Питомец ---
# Дефолтное имя записывается в pet.json при первом создании (данные).
default-pet-name = Дрифтлинг
