# driftling: вывод CLI (ctl/doctor) и IPC-ошибки демона.

# --- Справка CLI ---
cli-about = Настольный тамагочи, который дрейфует между вашими устройствами
cli-about-ctl = Управление запущенным демоном
cli-about-settings = Открыть окно настроек (запускает driftling-settings)
cli-about-summon = Позвать питомца на экран
cli-about-dismiss = Убрать питомца с экрана (демон продолжает работать)
cli-about-status = Показать состояние
cli-about-info = Карточка питомца: имя, стадия, статы, характеристики
cli-about-feed = Покормить питомца (обычная еда)
cli-about-feed-treat = Дать вкусняшку (настроение выше; злоупотребление бьёт по здоровью)
cli-about-play = Поиграть с питомцем
cli-about-sleep = Уложить питомца спать
cli-about-rename = Переименовать питомца
cli-about-recolor = Перекрасить питомца (hex-цвет #rrggbb, например e8944a)
cli-about-reload = Перечитать конфиг и применить на лету
cli-about-quit = Остановить демон
cli-about-doctor = Диагностика: окружение, сокет, файлы, автозапуск (работает без демона)

# --- Вывод ctl ---
ctl-ok = ок
ctl-status = питомцев: { $pets }, состояние: { $state }, аптайм: { $uptime }s
ctl-petinfo = { $name } ({ $stage }): { $state }, аптайм { $uptime }s
ctl-petinfo-stats = сытость { $satiety } · энергия { $energy } · настроение { $mood } · здоровье { $health }
ctl-petinfo-color = цвет: { $color }
ctl-petinfo-attributes = характеристики: { $attributes }
ctl-recolor-bad-hex = «{ $value }» не похоже на цвет — нужен hex #rrggbb, например e8944a
state-dismissed = убран с экрана

# --- Стадии роста (машинное имя шлёт демон) ---
stage-egg = яйцо
stage-baby = малыш
stage-child = ребёнок
stage-teen = подросток
stage-adult = взрослый
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
doctor-journal = журнал: событий { $events }, битых строк пропущено { $warnings } ({ $path })
doctor-journal-unreadable = журнал событий не читается: { $error }
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
daemon-journal-append-failed = событие ухода не записано в журнал: { $error }
daemon-rename-empty = имя не может быть пустым
daemon-config-unreadable = config.toml не прочитан: { $error }

# --- Трей (B7) ---
tray-tooltip = Дрифтлинг
tray-summon = Призвать
tray-dismiss = Убрать с экрана
tray-settings = Настройки
tray-quit = Остановить демона

# --- Контекстное меню питомца (B3, ПКМ по питомцу) ---
menu-feed = Покормить
menu-treat = Вкусняшка
menu-play = Поиграть
menu-sleep = Уложить спать
menu-settings = Настройки
menu-dismiss = Убрать с экрана

# --- Речевые пузыри (B5/B6) ---
bubble-hello = Привет!

# --- Питомец ---
# Дефолтное имя записывается в Genesis-событие журнала ровно один раз,
# при рождении питомца (это данные — смену локали они переживают).
default-pet-name = Дрифтлинг
