# driftling-settings: интерфейс окна настроек.

# --- Бейдж состояния питомца ---
state-absent = Убран с экрана
state-idle = Отдыхает
state-walk = Гуляет
state-sleep = Спит
state-falling = Падает
state-dragged = В руках
state-landing = Приземлился
state-climb = Лезет по стене
state-bonk = Набил шишку
state-roll = Катится кувырком
state-unknown = Неизвестно

# --- Аптайм (плюральные формы CLDR: one/few/many) ---
uptime-hours =
    { $hours ->
        [one] { $hours } час
        [few] { $hours } часа
        [many] { $hours } часов
       *[other] { $hours } часа
    }
uptime-minutes =
    { $minutes ->
        [one] { $minutes } минута
        [few] { $minutes } минуты
        [many] { $minutes } минут
       *[other] { $minutes } минуты
    }
uptime-seconds =
    { $seconds ->
        [one] { $seconds } секунда
        [few] { $seconds } секунды
        [many] { $seconds } секунд
       *[other] { $seconds } секунды
    }

# --- Сайдбар ---
nav-pet = Питомец
nav-app = Приложение
nav-debug = Отладка
badge-checking = Проверяем…
daemon-running = Демон работает
daemon-not-running = Демон не запущен

# --- Страница «Питомец» ---
default-pet-name = Дрифтлинг
badge-no-data = Нет данных
online-for = в сети { $uptime }
btn-summon = Призвать
btn-dismiss = Убрать с экрана
msg-pet-summoned = Питомец призван
msg-pet-dismissed = Питомец убран
daemon-error = Ошибка демона: { $error }
generic-error = Ошибка: { $error }
rename-hint = Переименовать
msg-renamed = Питомец переименован
msg-rename-empty = Имя не может быть пустым
btn-feed = Покормить
btn-treat = Вкусняшка
btn-play = Поиграть
btn-sleep = Уложить
msg-fed = Питомец покормлен
msg-treat-given = Вкусняшка выдана
msg-played = Поиграли с питомцем
msg-put-to-sleep = Питомец уложен спать
daemon-down-hint = Демон не запущен — запустите его, чтобы ухаживать за питомцем.
btn-start-daemon = Запустить демона
msg-daemon-starting = Запускаем демона…

# --- Карточка «Внешний вид» (цвет питомца) ---
section-appearance = Внешний вид
appearance-hint = Цвет питомца задаёт и акцент всего приложения.
color-custom = Свой цвет
msg-recolored = Питомец перекрашен
color-greige = Бежевый
color-amber = Янтарный
color-mint = Мятный
color-sky = Голубой
color-rose = Розовый
color-slate = Графитовый
color-sand = Песочный
color-violet = Фиолетовый

# --- Чип стадии роста ---
stage-egg = Яйцо
stage-baby = Малыш
stage-child = Ребёнок
stage-teen = Подросток
stage-adult = Взрослый

# --- Карточка «Состояние» ---
section-condition = Состояние
stat-satiety = Сытость
stat-energy = Энергия
stat-mood = Настроение
stat-health = Здоровье
condition-unavailable = Демон не запущен — состояние питомца недоступно.

section-stats = Характеристики
stat-speed = Скорость
stat-speed-value = { $value } px/с
stat-curiosity = Непоседливость
stat-sleepiness = Сонливость
stat-size = Размер
stat-size-value = { $value } px
stat-sleep = Сон
stat-sleep-value = { $min }–{ $max } сек
stats-unavailable = Демон не запущен — характеристики недоступны.
stats-grow-note = Характеристики растут вместе с питомцем

# --- Страница «Приложение» ---
section-launch = Запуск
autostart-title = Автозапуск при входе в систему
autostart-desc = Демон запустится вместе с рабочим столом
msg-autostart-on = Автозапуск включён
msg-autostart-off = Автозапуск выключен
section-daemon = Демон
uptime-label = · аптайм { $uptime }
btn-stop-daemon = Остановить демона
btn-stop-confirm = Точно остановить?
msg-daemon-stopped = Демон остановлен
section-about = О программе
about-desc = Питомец для рабочего стола Wayland. Живёт на нижней кромке экрана, гуляет, спит и падает в руки.

# --- Карточка «Синхронизация» (фаза E) ---
section-sync = Синхронизация
sync-mode-off = Выкл
sync-mode-server = Свой сервер
sync-mode-folder = Папка
sync-off-hint = Питомец живёт только на этом устройстве.
sync-address-label = Адрес сервера
sync-address-hint = driftling-server по plain http — держите его в LAN/VPN или за локальным прокси; токен выдаёт `driftling-server account add`.
sync-token-label = Токен
sync-folder-label = Папка журналов
sync-folder-hint = Папка, которую синкает Syncthing/Nextcloud: журналы переезжают туда, по файлу на устройство. pet.json всегда остаётся локальным.
msg-sync-applied = Сохранено — демон перечитал настройки синка
msg-sync-saved-daemon-down = Сохранено; применится при запуске демона
sync-status-line = push: { $push } · pull: { $pull }
sync-status-ago = { $secs } с назад
sync-status-never = ещё не было
sync-status-journal = событий: { $events } · устройств: { $devices }
sync-status-lease-ours = питомец здесь
sync-status-lease-other = питомец на «{ $holder }»
sync-status-error = ошибка синка: { $error }

# --- Страница «Отладка» ---
debug-warning = Админ-панель. Прямое редактирование характеристик — для отладки; в игре они будут расти через уход за питомцем.
section-pet-attrs = Характеристики питомца
slider-size = Размер, px
slider-speed = Скорость, px/с
slider-curiosity = Непоседливость
slider-sleepiness = Сонливость
sleep-secs-label = Сон, сек:
sleep-from = от
sleep-to = до
debug-clamp-note = Демон клампит значения: непоседливость + сонливость ≤ 100, мин. сна ≤ макс.
btn-apply = Применить
msg-applied = Применено и сохранено
btn-reset = Сбросить к дефолту
section-raw-stats = Сырые статы
stat-stage = Стадия
debug-growth-hint = Ускоренный тест роста: запустите демона с DRIFTLING_GROWTH_SCALE=1440 (минута реального времени = игровые сутки).
section-raw = Сырой ответ демона
raw-none = Нет ответа от демона.

# --- Навигация (редизайн M0.6) ---
nav-world = Мир
nav-devices = Устройства
nav-advanced = Продвинутые
nav-hotkey = Ctrl+{ $n }
details = Подробнее

# --- Общие сообщения ---
msg-settings-applied = Настройки применены
msg-settings-saved-offline = Сохранено — применится при запуске демона
msg-config-rescued = Старый config.toml не читался, он сохранён как { $file }
msg-nothing-changed = Ничего не изменилось
msg-needs-restart = Применилось не всё: { $items } потребует перезапуска демона
msg-ui-saved = Настройки окна сохранены
msg-attrs-applied = Характер применён
msg-treated = Вкусняшка выдана
msg-sleeping = Питомец уложен спать
msg-guest-called = Гость позван
msg-ride-called = Транспорт подан
msg-ride-stopped = Питомец высажен
msg-ball-out = Мяч на экране
msg-ball-away = Мяч убран
msg-daemon-stopping = Демон останавливается

# --- Страница «Питомец» ---
btn-dismiss-hint = Питомец помашет лапкой и убежит за край экрана
btn-take-current = Взять текущие
pet-size = Размер
pet-size-hint = Влияет и на масштаб мира: от размера считаются вес, гравитация и размеры вещей
pet-journal-note = Всё на этой странице меняет самого питомца: события уходят в журнал и на другие устройства
section-temper = Характер
temper-preset = Темперамент
temper-hint = Готовые наборы; ручная правка — в тонкой настройке
temper-calm = Спокойный
temper-normal = Обычный
temper-lively = Непоседа
temper-custom = Свой
temper-fine = Тонкая настройка
attr-walk-speed = Скорость шага
attr-curiosity = Непоседливость
attr-sleepiness = Сонливость
attr-sleep-range = Сон длится, с

# --- Страница «Мир» ---
section-peace = Покой
section-fun = Развлечения
unit-secs = с
unit-mins = мин
unit-days = сут
section-guests = Гости
section-rides = Транспорт
section-things = Вещи
section-mess = Беспорядок
world-local-note = Эти настройки — правила этого компьютера: на другое устройство они не уезжают
world-offline-note = Демон не запущен: настройки сохранятся в файл и применятся при следующем запуске
world-muted-by-quiet = Сейчас выключено тихим режимом
world-quiet = Тихий режим
world-quiet-hint = Питомец ничего не затевает сам: ни гостей, ни транспорта, ни походов домой
world-hide = Прятаться при полноэкранном окне
world-hide-hint = Если есть домик — уходит в него, а потом выходит здороваться
world-grace = Ждать перед уходом
world-grace-hint = Короткие вспышки полноэкранного окна игнорируются
world-sick = Укачивается на руках
world-sick-hint = Долгая тряска мышью доводит до тошноты; выключено — трясти можно сколько угодно
world-surprises = Частота сюрпризов
world-surprises-hint = Множитель для всего, что случается само: 0 — только явные действия человека
world-war = Незваные гости
world-war-hint = Изредка заходит пылевой комок, таракан или жук-баг; питомец их гоняет. Урона нет
world-mob-every = Заходят примерно раз в
world-mob-kinds = Кто может зайти
world-kinds-hint = Ничего не выбрано — годятся все
world-guest-manual-hint = Позвать можно и при выключенном режиме
btn-summon-guest = Позвать гостя
world-rides = Приезжает сам
world-rides-hint = Изредка питомцу подаётся транспорт, и он катается
world-ride-every = Подаётся примерно раз в
world-ride-kinds = Какой транспорт
world-riding-now = Сейчас катается: { $kind }
btn-ride-now = Прокатить сейчас
btn-stop-ride = Высадить
world-bowl = Заводить миску
world-bowl-hint = Первая кормёжка ставит миску, дальше питомец ходит есть к ней
world-bed = Заводить лежанку
world-bed-hint = «Уложить спать» ведёт питомца в лежанку, а не на голый пол
world-house = Домик
world-house-hint = Свой угол: питомец прячется туда от полноэкранного окна и выходит махать
world-house-age = Появляется с возраста
world-ball = Мяч
world-ball-hint = Не настройка, а вещь: мяч временный, в журнал не пишется и на другое устройство не уезжает
world-scene = Сейчас в мире
world-scene-empty = Пока пусто
world-tag-kept = постоянная
world-tag-temporary = временная
world-tag-guest = гость
world-puddles = Лужи и уборка
world-puddles-hint = Выключено — новых луж не будет; стоящую питомец домоет
world-sneezes = Чихает
world-sneezes-hint = Редкий чих в покое
world-litter = Пылевой комок сорит
world-litter-hint = Гость оставляет за собой пыль
world-shadow = Тень под питомцем
world-shadow-hint = Без тени питомец визуально отлипает от пола
world-chore = Берётся за уборку через
world-chore-hint = Сколько питомца не трогают, прежде чем он достанет швабру

# --- Страница «Устройства» ---
section-where-lives = Где живёт питомец
section-sync-status = Что происходит
sync-mode = Режим
sync-mode-hint = Один компьютер, свой сервер или общая папка
sync-address = Адрес сервера
sync-address-empty = Без адреса синк не заработает
sync-address-scheme = Адрес должен начинаться с http:// или https://
sync-token = Токен
sync-token-hint = Пусто — оставить прежний; демон токен наружу не отдаёт
sync-token-kept = задан, оставить как есть
sync-token-empty = не задан
sync-folder = Папка
sync-folder-empty = Укажите папку
sync-folder-missing = Такой папки нет
sync-folder-not-dir = Это не папка
sync-status-unavailable = Демон не рассказал о синке
sync-not-applied = Настройки ещё не применены
sync-last-push = Отправлено
sync-last-pull = Получено
sync-events = Событий / устройств
sync-presence = Питомец сейчас
sync-here = здесь
sync-elsewhere = на «{ $device }»
sync-nobody = ничей
sync-never = ещё ни разу
sync-seconds-ago = { $secs } с назад
sync-reload-hint = Если правили config.toml руками
btn-discard = Отменить
btn-open = Открыть
btn-reload = Перечитать

# --- Страница «Приложение» ---
section-window = Окно
section-service = Служебное
ui-language = Язык интерфейса
ui-language-hint = Применится после перезапуска окна; язык пузырей питомца берётся из системы
ui-start-page = Открывать на странице
ui-advanced = Показывать «Продвинутые»
ui-advanced-hint = То же, что запуск с флагом --debug
ui-text-scale = Размер подписей у питомца
ui-text-scale-hint = Пузыри и подписи меню; помогает на экранах с высокой плотностью
lang-system = Как в системе
lang-ru = Русский
lang-en = English
service-config = Файл настроек
service-doctor = Проверить установку
service-doctor-hint = Прогоняет driftling ctl doctor и показывает отчёт
service-doctor-running = Проверяю…
service-doctor-empty = Доктор ничего не сказал
about-version = Версия
btn-run = Прогнать

# --- Страница «Продвинутые» ---
advanced-warning = Технические ручки. Здесь легко сделать питомцу странно
section-physics = Физика мира
physics-height = Рост питомца «в жизни»
physics-height-hint = Меньше рост — мельче мир: то же падение выглядит быстрее
physics-room = Комната примерно { $w } × { $h } м
stat-health-hint = Скрытый стат: на обычных страницах не показывается
world-screen = Рабочая область
world-screen-unknown = экран ещё не известен
world-hidden-now = Питомец сейчас спрятан: полноэкранное окно
raw-petinfo = PetInfo (JSON)
section-dangerous = Опасное
dangerous-ride = Подать конкретный транспорт
dangerous-ride-hint = В обход списка допущенных видов
dangerous-mob = Позвать конкретного гостя
dangerous-mob-hint = В обход режима войны и списка видов
btn-reset-default = Вернуть по умолчанию

# --- Имена вещей мира ---
prop-mop = Швабра
prop-bowl = Миска
prop-bed = Лежанка
prop-ball = Мяч
prop-house = Домик
prop-skate = Скейт
prop-bike = Велосипед
prop-moped = Мопед
prop-car = Автомобиль
prop-copter = Вертолёт
prop-plane = Самолёт
prop-dustball = Пылевой комок
prop-roach = Таракан
prop-bug = Жук-баг
