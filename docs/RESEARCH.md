# Обзор существующих проектов (август 2026)

Исследование проведено 2026-08-07. Все репозитории проверены по живым ссылкам (звёзды, активность, лицензии — по GitHub API на дату проверки).

## Вывод

Ниша **«тамагочи + ходьба по окнам + Linux/Wayland-first»** пуста:

- Семейство **shimeji** умеет ходить по окнам, но ни один форк не имеет тамагочи-механик (все — «анимационные игрушки»).
- **VPet-Simulator** — самый полный тамагочи (6.6k★), но Windows-only (WPF) и не взаимодействует с окнами.
- Wayland-нативных проектов единицы (wl_shimeji, wayland-vpets), и оба без механик ухода.
- Самый близкий прототип нужной архитектуры — **wl_shimeji** (оверлей layer-shell + KWin-плагин для геометрии окон), но это чистый движок без геймплея.

## Семейство Shimeji

| Проект | ★ | Активность | Стек | Платформы | Окна | Тамагочи |
|---|---|---|---|---|---|---|
| [Shimeji-ee](https://github.com/xxhiddenchaos/shimeji-ee) (оригинальная линия) | — | мертв (2016); линию продолжает Kilkakon | Java 6 + JNA | Windows | да (Win32) | нет |
| [Kilkakon Shimeji](https://kilkakon.com/shimeji/) | не на GitHub | v1.0.22, авг 2025 | Java + JNA | Windows | да, с блэклистом окон | нет |
| [Shimeji-Desktop](https://github.com/DalekCraft2/Shimeji-Desktop) (DalekCraft2) | 43 | активен (июль 2026) | Java 25, Maven | Win / macOS / Linux X11 | да (JNA/Xlib) | нет |
| [wl_shimeji](https://github.com/CluelessCatBurger/wl_shimeji) | 192 | активен (март 2026) | C, Wayland | Linux Wayland (KDE/wlroots; без GNOME, Hyprland, Gamescope) | только через KWin-плагин (D-Bus) | нет |
| [Shijima-Qt](https://github.com/pixelomer/Shijima-Qt) + libshijima | 196 | **архивирован 2026-04-29**; libshijima жив | C++17 + Qt6 | Win / macOS / Linux (KDE, GNOME) | да — активное окно, через shell-плагины | нет |
| [NeurolingsCE](https://github.com/qingchenyouforcc/NeurolingsCE) | 33 | очень активен (авг 2026) | C++17 + Qt6 (форк Shijima-Qt) | Win / Linux / macOS | да (унаследовано) | нет |
| [ShimejiEE-cross-platform](https://github.com/LavenderSnek/ShimejiEE-cross-platform) | 89 | июнь 2025 | Java 17 | macOS (Linux WIP) | да на macOS | нет |
| [linux-shimeji](https://github.com/estenv/linux-shimeji) | 166 | **архивирован 2025-08** («Wayland убил доступ к дереву окон») | Java 6-эры | Linux X11 | да (Xlib) | нет |
| [a1098832322/shimeji](https://github.com/a1098832322/shimeji) | 120 | май 2026 | Java 11 | Win / macOS | да | нет |
| [webmeji](https://github.com/lars-rooij/webmeji) | 40 | янв 2026 | JS (браузер) | любой браузер | DOM-границы | нет |

Ключевые уроки семейства:
- **Shijima-Qt — предсмертная записка Qt-подхода**: автор архивировал проект с вердиктом «Qt was not the right framework for this project»; нативный Wayland-бэкенд на layer-shell-qt так и не вылечили от исчезающих маскотов ([issue #75](https://github.com/pixelomer/Shijima-Qt/issues/75)).
- **linux-shimeji архивирован** именно из-за Wayland.
- **wl_shimeji — выживший референс** нативного Wayland-оверлея: один fullscreen layer-shell surface на выход + wl_subsurface на питомца; ~0.9% CPU / ~56 МиБ на 7 маскотов против ~19% / ~880 МиБ у Java-оригинала.

## Прочие десктоп-питомцы и тамагочи

| Проект | ★ | Активность | Стек | Платформы | Окна | Тамагочи |
|---|---|---|---|---|---|---|
| [VPet-Simulator](https://github.com/LorisYounger/VPet) | 6.6k | очень активен (авг 2026) | C# WPF | **Windows only** | нет | **полный**: голод/жажда/настроение/здоровье/уровень/деньги/работа/болезнь |
| [BongoCat](https://github.com/ayangweb/BongoCat) (ayangweb) | 22.4k | апр 2026 | Tauri 2 + Rust | Win / macOS / Linux **X11 only** | нет | нет (реагирует на клавиатуру) |
| [clawd-on-desk](https://github.com/rullerzhou-afk/clawd-on-desk) | 5.9k | очень активен (авг 2026) | Electron | Win / macOS / Linux | нет | нет (монитор AI-агентов) |
| [OpenPets](https://github.com/alvinunreal/openpets) | 1k | активен (авг 2026) | Electron + TS | Win / macOS / Linux (AppImage) | нет | лёгкий (плагин Virtual Pet: голод/энергия/привязанность) |
| [DyberPet](https://github.com/ChaozhongLiu/DyberPet) | 911 | июль 2026 | Python + PySide6 | Windows (Linux частично) | нет | **развитый**: сытость/привязанность/инвентарь/магазин/квесты/помодоро |
| [Mate-Engine](https://github.com/shinyflvre/Mate-Engine) (+ [Linux-порт](https://github.com/Marksonthegamer/Mate-Engine-Linux-Port)) | 3.5k / 285 | янв / июль 2026 | Unity 6, VRM | Windows; Linux **X11 only** | частично (прилипает к окнам/докам) | нет |
| [CppGoose](https://github.com/jeffthepineapple/desktop-goose-linux-port) (Desktop Goose для Linux) | 38 | июль 2026 | C++17, GTK4 + gtk4-layer-shell | Linux X11 **и Wayland** | частично | нет (хаос-игрушка) |
| [wayland-vpets](https://github.com/furudbat/wayland-vpets) | 97 | июль 2026 | C++, layer-shell | Linux **Wayland only** | нет | частично (эволюция дигимонов, настроение от скорости печати) |
| [oneko](https://github.com/tie/oneko) / [oneko.js](https://github.com/adryd325/oneko.js) | 149 / 1.3k | заморожен / окт 2025 | C+Xlib / JS | X11 / браузер | нет | нет |
| [XPenguins](https://github.com/spartrekus/xpenguins) | — | не поддерживается | C + Xlib | Linux X11 | **да — лучшая механика окон-как-рельефа** | нет |
| [KDE AMOR](https://github.com/KDE/amor) | 29 | мертв (KDE4) | C++/Qt | Linux X11 | да (сидит на активном окне) | нет |
| [Gnomelets](https://extensions.gnome.org/extension/9035/gnomelets/) | e.g.o | активен (GNOME 45–50) | GJS-расширение | **GNOME Wayland+X11** | **да — ходит по кромкам окон изнутри Shell** | нет |
| [RunCat GNOME](https://github.com/win0err/gnome-runcat) | 544 | май 2026 | GJS | GNOME | нет (панель) | нет |
| [Agentic-Desktop-Pet](https://github.com/jihe520/Agentic-Desktop-Pet) | 327 | янв 2026 | Godot 4 + Python | Windows | нет | частично (эмоции/RPG через LLM) |
| [tama96](https://github.com/siegerts/tama96) | — | 2026 | — | десктоп/терминал | нет | да (классический цикл) |

## Технические первоисточники

- Протоколы: [wlr-layer-shell](https://wayland.app/protocols/wlr-layer-shell-unstable-v1), [ext-foreign-toplevel-list](https://wayland.app/protocols/ext-foreign-toplevel-list-v1) (геометрии НЕ даёт), [cosmic-toplevel-info](https://wayland.app/protocols/cosmic-toplevel-info-unstable-v1) (единственный с событием geometry), [xdg-output](https://wayland.app/protocols/xdg-output-unstable-v1).
- KWin scripting: [API](https://develop.kde.org/docs/plasma/kwin/api/) — `workspace.stackingOrder`, `frameGeometry`, `frameGeometryChanged`, `callDBus`; загрузка скриптов через D-Bus `org.kde.kwin.Scripting` `/Scripting` (loadScript/unloadScript — подтверждено по исходникам KWin).
- Hyprland: [IPC](https://wiki.hypr.land/IPC/) — `.socket.sock` (запросы, `hyprctl clients -j` → `at`/`size`) + `.socket2.sock` (события).
- sway: [sway-ipc(7)](https://man.archlinux.org/man/sway-ipc.7.en) — `get_tree` → `rect`/`window_rect`/`deco_rect`, подписка на `window`-события.
- GNOME: расширение [window-calls](https://github.com/ickyicky/window-calls) (List/Details c x/y/w/h по D-Bus) как образец; [Gnomelets](https://extensions.gnome.org/extension/9035/gnomelets/) как доказательство «полного» пути.
- Референс KWin-моста: [wl_shimeji.kwinsupport](https://github.com/CluelessCatBurger/wl_shimeji.kwinsupport) (GPL-2.0 — только как образец идеи, код не копируем).
- winit `set_cursor_hittest`: Windows/macOS/Wayland с 0.27, X11 починен в 0.30.13 ([changelog](https://rust-windowing.github.io/winit/winit/changelog/v0_30/index.html)); layer-shell в winit нет ([#2582](https://github.com/rust-windowing/winit/issues/2582)) → smithay-client-toolkit.
- Godot 4 (важно — почему НЕ он): `mouse_passthrough` и `always_on_top` **не реализованы на Wayland** ([Window docs](https://docs.godotengine.org/en/stable/classes/class_window.html)).
- Электрон/Tauri: click-through на Linux худший из всех вариантов ([electron#16777](https://github.com/electron/electron/issues/16777), [tauri#6164](https://github.com/tauri-apps/tauri/issues/6164)).
- Формат shimeji-паков: [структура и порядок поиска conf](https://github.com/DalekCraft2/Shimeji-Desktop), условия в `actions.xml` — это JavaScript-выражения (`${mascot.environment.activeIE.topBorder.isOn(...)}`); готовый парсер/симулятор — [libshijima](https://github.com/pixelomer/libshijima) (GPL-3.0).

## Мультиустройство: есть ли «один питомец на всех устройствах» у кого-то?

Проверено 2026-08-07 (отдельное исследование, факты верифицированы по первоисточникам). **Короткий ответ: нет ни у кого — это реальный дифференциатор.** Всё, что существует:

| Кто | Что умеет на самом деле |
|---|---|
| [VPet-Simulator](https://store.steampowered.com/app/1920960/VPetSimulator/) | Синк **файла сейва** через Steam Cloud — настолько конфликтный, что автор в обновлении 06.10.2025 добавил локальные бэкапы + сверку «для предотвращения потери сейвов из-за Steam Cloud», а сообщество сделало [сторонний Cloud Save мод](https://steamcommunity.com/sharedfiles/filedetails/?id=3307842583) с внешним (в т.ч. self-hosted) сервером. Мультиплеер VPet — «сходить в гости к другу», не кросс-девайс |
| Tamagotchi On (Bandai) | **Перенос** (check-out/check-in) питомца в приложение по Bluetooth и обратно — питомец существует ровно в одном месте; Tamagotchi Uni — питомец живёт только на устройстве |
| DyberPet, OpenPets и остальные | Никакого синка вообще (локальные JSON) |
| [mwambanatanga/tama](https://github.com/mwambanatanga/tama) | Единственный «pet-сервер» в истории — Net Tamagotchi по **telnet** из 90-х, текстовый, мёртвый |
| Steam Cloud как базовый UX | Диалог «выбери файл: локальный или облачный», last-writer-wins целым файлом — ровно то, чего event-журнал позволяет не иметь в принципе |

### Прецеденты архитектуры селфхост-синка

- **[atuin](https://github.com/atuinsh/atuin)** (Rust, синк истории shell) — ближайший образец целиком: приложение полноценно офлайн; официальный хост и селфхост различаются одной настройкой `sync_address` (дефолт `api.atuin.sh`); сервер — отдельный бинарь `atuin-server` (+Docker), Postgres 14+ **или SQLite**, TLS отдан reverse-proxy; синк v2 — append-only журнал записей, шифрованных на клиенте (PASETO v4.local + PASERK, ключ per-record, обёрнутый мастер-ключом) — сервер слепой: «I couldn't access your data even if I wanted to».
- **[Joplin](https://joplinapp.org/help/dev/spec/sync/)** — абстракция «цель синка»: «тупые» хранилища (WebDAV/Nextcloud/S3/файловая система) через generic file API vs «живой» Joplin Server с delta-эндпоинтом; E2E — цель видит только шифротекст. **Анти-урок лицензии**: клиенты AGPL, но [Joplin Server — несвободная Personal Use License](https://github.com/laurent22/joplin/blob/dev/packages/server/LICENSE.md) → многолетние трения с сообществом.
- **[vaultwarden](https://github.com/dani-garcia/vaultwarden)** — доказательство, что маленький (SQLite, ~50 МБ RAM) протокол-совместимый сервер побеждает тяжёлый официальный стек у селфхостеров → наш протокол должен быть простым и документированным.
- Ожидания selfhost-сообщества 2026: Docker compose — лингва франка, но идеал — один статический бинарь (эталон — PocketBase: один бинарь + встроенный SQLite/WAL).

### Алгоритм слияния (проверенные источники)

- **Журнал событий = G-Set CRDT**: объединение per-device append-only множеств событий с уникальными ID коммутативно/ассоциативно/идемпотентно → все реплики сходятся без конфликтов ([Wikipedia CRDT](https://en.wikipedia.org/wiki/Conflict-free_replicated_data_type)); статы — чистая функция от слитого журнала + времени, их синкать не надо. Automerge/Yjs для этой формы данных избыточны.
- **Референсная реализация — [jlongster/crdt-example-app](https://github.com/jlongster/crdt-example-app)** (автор Actual Budget): HLC-метки `ISO-millis + hex-счётчик + node-id` (лексикографически сортируемы), per-field LWW как tie-break, merkle-trie по HLC для дешёвого поиска точки расхождения при реконнекте.
- **HLC устойчивы к сдвигу часов** (сдвиг портит близость к wall time, не корректность порядка — [Demirbas, соавтор HLC](http://muratbuffalo.blogspot.com/2014/07/hybrid-logical-clocks.html)); guard дрейфа (~60 с, `ClockDriftError`) у библиотек **по умолчанию выключен** — конфигурировать самим.
- **Анти-чит не нужен**: Animal Crossing NH — канонический прецедент терпимости к переводу часов (только мягкие диегетические последствия).
- **Синк через папку (Syncthing/Nextcloud)**: паттерн [tonsky «Local, first, forever»](https://tonsky.me/blog/crdt-filesync/) — один append-only файл на устройство (single-writer → конфликты файлового синка недостижимы), чужие файлы read-only; Syncthing пишет получателю атомарно (temp+rename), но «рваный хвост» на отправителе — наша забота (чексуммы записей); [живой SQLite файлами не синкать](https://www.sqlite.org/howtocorrupt.html). Компакция журнала — нерешённая в источнике домашка.
- **Присутствие**: lease с heartbeat + fencing ([Fowler, Lease pattern](https://martinfowler.com/articles/patterns-of-distributed-systems/lease.html)); UX-модель — Steam «запущено на другом компьютере» (newest-wins), при партиции — забота локально и слияние потом.

## Проверка имён (2026-08-07, два раунда)

Раунд 2: 16 кандидатов, проверка GitHub (`in:name` + ник), crates.io, продукты/сторы; финалисты — ещё домены (RDAP: .app/.dev/.io), Steam/itch/App Store, товарные знаки, произношение RU/EN.

**Финальный рейтинг:**

| Место | Имя | GitHub-ник | crates.io | Домены .app/.dev/.io | Коллизии |
|---|---|---|---|---|---|
| **1. Driftling** | «малыш, который дрейфует» — единственное имя, рассказывающее про наш главный дифференциатор (питомец перетекает между устройствами) | **свободен** (единственный из 16!) | свободно | **все три свободны** | одна: инди-аркада «Driftling» в App Store (без ТЗ-регистрации, другая категория) |
| 2. Ledgeling | ledge+-ling (существо с карнизов окон, каламбур с fledgling) | свободен | свободно | все три свободны | ноль вообще; минусы: не говорит про кросс-девайс, для RU читается коряво («леджлинг»), путается с ledger |
| 3. Domovoi | лучшая история (домовой = дух, живущий в доме и переезжающий с семьёй), но: domovoi.app и .dev заняты живым продуктом 2026 г., игры «The Domovoi»/«Domovoy», Дворецки из Артемиса Фаула, три варианта транслитерации | сквот | свободно | занято/занято/свободно | много |
| 4. Gotchling | чисто везде (0 репо, ник/домены свободны), но вся ценность имени — намёк на Tamagotchi, а Bandai держит марки в 56 странах и активно их защищает | свободен | свободно | все три свободны | юридическая тень |

Выбывшие с жёсткими коллизиями: Poppet (питомец Moshi Monsters + одноимённое приложение), Windowsill (живое Windows-приложение + care-игра на itch с точным именем), Eggling (активная Steam-игра про выращивание питомцев «Egglings»), Perchling (свежий desktop-pet на GitHub + птичка в Pocket Bird), Wanderling/Mosey/Amble/Skitter/Scamper (crowded), Moppet (путаница с Muppet), Spriteling (JS-библиотека 107★ владеет поиском), Pippet (занято).

Против базовых Deskgotchi/Wandergotchi: оба содержат строку «-gotchi» внутри активно защищаемого семейства марок Bandai и оба хуже как имя бинаря; Driftling/Ledgeling так же бесколлизионны, но без юридической тени. Рекомендация отчёта: **Driftling** как имя проекта, «Домовой» — как имя дефолтного персонажа/скина (вся история без цены коллизий). Свободный ник GitHub — скоропортящееся преимущество.

Раунд 1 (для истории): deskgotchi/wandergotchi свободны, tamapet (10 репо) и screenpets (macOS-питомец 2026) заняты. Соседние занятые имена: VPet, DPET, OpenPets, Shimeji*, PawPause, tama96.
