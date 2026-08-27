# driftling-settings: интерфейс окна настроек.

# --- Бейдж состояния питомца ---
state-absent = Убран с экрана
state-idle = Отдыхает
state-walk = Гуляет
state-sleep = Спит
state-falling = Падает
state-dragged = В руках
state-landing = Приземлился
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
