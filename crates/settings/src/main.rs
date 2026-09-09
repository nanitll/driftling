//! Окно настроек Driftling (ТЗ §3.3, редизайн M0.6).
//!
//! Пять страниц: «Питомец» (кто это существо и как оно себя чувствует),
//! «Мир» (правила мира вещей — секции `[game]`, `[world]`, `[comfort]`),
//! «Устройства» (синхронизация), «Приложение» (автозапуск, демон, окно) и
//! «Продвинутые» (техническое, включается настройкой или флагом --debug).
//!
//! Три правила, на которых всё держится:
//!
//! - **UI-поток не ходит в IPC и не пишет файлы.** Всё блокирующее — в
//!   фоновых потоках ([`state`]), окно читает готовый снимок.
//! - **Демон предпочтительный, но не единственный писатель настроек.**
//!   Окно чаще всего открывают как раз тогда, когда питомца нет на экране;
//!   при лежащем демоне настройки пишутся прямо в файл ([`config_io`]).
//! - **Что меняет питомца — идёт событиями журнала** (имя, цвет, размер,
//!   характер) и уезжает на другие устройства; что меняет правила мира —
//!   локальный конфиг этого компьютера.

mod app;
mod config_io;
mod i18n;
mod labels;
mod pages;
mod state;
mod system;
mod theme;
mod uiprefs;

use app::{Page, SettingsApp};
use driftling_core::{Stage, DEFAULT_PET_COLOR};
use eframe::egui;

/// Иконка окна: взрослый idle-кадр арт-пака.
fn app_icon() -> egui::IconData {
    let frames = theme::pack_idle_frames(Stage::Adult, 64, DEFAULT_PET_COLOR);
    let f = &frames[0];
    let rgba = f
        .argb
        .iter()
        .flat_map(|&p| {
            [
                ((p >> 16) & 0xff) as u8,
                ((p >> 8) & 0xff) as u8,
                (p & 0xff) as u8,
                (p >> 24) as u8,
            ]
        })
        .collect();
    egui::IconData {
        rgba,
        width: f.w,
        height: f.h,
    }
}

fn main() -> eframe::Result {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    let debug_flag = args.iter().any(|a| a == "--debug");
    // `--page world` — меню питомца открывает окно сразу на нужной
    // странице, когда внешнее кольцо переключателей не поместилось.
    let start = args
        .iter()
        .position(|a| a == "--page")
        .and_then(|i| args.get(i + 1))
        .and_then(|k| Page::from_key(k));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 620.0])
            .with_min_inner_size([660.0, 480.0])
            // app_id = имя .desktop-файла (SHIPPING.md, F1): без совпадения
            // KDE показывает generic-иконку в панели/alt-tab.
            .with_app_id(driftling_core::APP_ID)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Driftling",
        options,
        Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, debug_flag, start)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::fl;
    use crate::labels::*;
    use crate::system::*;
    use crate::theme::*;
    use driftling_core::PET_PRESETS;
    use eframe::egui::Color32;
    use std::collections::HashMap;
    use std::path::Path;

    /// Отображение всех состояний тотально и идёт через i18n (ТД-30):
    /// сравниваем с fl!-рендером — тест не зависит от локали машины.
    #[test]
    fn state_labels_are_localized_and_total() {
        assert_eq!(state_label(None), fl!("state-absent"));
        assert_eq!(state_label(Some("Idle")), fl!("state-idle"));
        assert_eq!(state_label(Some("Walk")), fl!("state-walk"));
        assert_eq!(state_label(Some("Sleep")), fl!("state-sleep"));
        assert_eq!(state_label(Some("Climb")), fl!("state-climb"));
        assert_eq!(state_label(Some("Bonk")), fl!("state-bonk"));
        assert_eq!(state_label(Some("Roll")), fl!("state-roll"));
        assert_eq!(state_label(Some("Falling")), fl!("state-falling"));
        assert_eq!(state_label(Some("Dragged")), fl!("state-dragged"));
        assert_eq!(state_label(Some("Landing")), fl!("state-landing"));
        assert_eq!(state_label(Some("???")), fl!("state-unknown"));
        // Известные состояния не проваливаются в «неизвестно».
        assert_ne!(state_label(Some("Idle")), state_label(Some("???")));
    }

    #[test]
    fn state_colors_match_design_system() {
        // «Гуляет» подсвечивается акцентом (цветом питомца), остальные
        // семантические цвета фиксированы.
        let al = Color32::from_rgb(1, 2, 3);
        assert_eq!(state_color(Some("Walk"), al), al);
        assert_eq!(state_color(Some("Sleep"), al), SLEEP_BLUE);
        assert_eq!(state_color(Some("Falling"), al), AMBER);
        assert_eq!(state_color(Some("Dragged"), al), AMBER);
        assert_eq!(state_color(None, al), MUTED);
    }

    /// Акцентная пара: сам цвет + строго осветлённый тон, альфа входа
    /// не важна.
    #[test]
    fn accent_pair_derives_light_tone() {
        let (a, l) = accent_pair(0x00_8a_63_d2);
        assert_eq!((a.r(), a.g(), a.b(), a.a()), (0x8a, 0x63, 0xd2, 0xff));
        assert!(l.r() >= a.r() && l.g() >= a.g() && l.b() >= a.b());
        assert_ne!(a, l, "светлый тон отличим от базового");
        // Дефолт даёт пару без паники и с непрозрачными цветами.
        let (a, l) = accent_pair(DEFAULT_PET_COLOR);
        assert_eq!((a.a(), l.a()), (0xff, 0xff));
    }

    /// Каждый пресет ядра имеет локализованную подпись (ключ существует),
    /// незнакомое имя не роняет UI.
    #[test]
    fn preset_labels_cover_all_presets() {
        for (_, name) in PET_PRESETS {
            assert_ne!(
                preset_label(name),
                *name,
                "нет ключа локализации для {name}"
            );
        }
        assert_eq!(preset_label("greige"), fl!("color-greige"));
        assert_eq!(preset_label("violet"), fl!("color-violet"));
        assert_eq!(preset_label("no-such-preset"), "no-such-preset");
    }

    #[test]
    fn rgb_triplet_roundtrips_to_opaque_argb() {
        assert_eq!(rgb_to_argb([0xe8, 0x94, 0x4a]), 0xff_e8_94_4a);
        assert_eq!(argb_to_rgb(0xff_e8_94_4a), [0xe8, 0x94, 0x4a]);
        assert_eq!(argb_to_rgb(rgb_to_argb([1, 2, 3])), [1, 2, 3]);
        // Альфа входа не протекает в триплет.
        assert_eq!(argb_to_rgb(0x00_b0_a2_94), [0xb0, 0xa2, 0x94]);
    }

    #[test]
    fn norm_clamps_to_unit_range() {
        assert_eq!(norm(5.0, 5.0, 400.0), 0.0);
        assert_eq!(norm(400.0, 5.0, 400.0), 1.0);
        assert_eq!(norm(-10.0, 5.0, 400.0), 0.0);
        assert_eq!(norm(999.0, 5.0, 400.0), 1.0);
        // Вырожденная шкала не даёт NaN/inf.
        assert_eq!(norm(1.0, 3.0, 3.0), 0.0);
        let mid = norm(202.5, 5.0, 400.0);
        assert!((mid - 0.5).abs() < 1e-6);
    }

    #[test]
    fn argb_pixel_converts_channelwise() {
        let c = argb_to_color32(0xff_8a_63_d2);
        assert_eq!((c.r(), c.g(), c.b(), c.a()), (0x8a, 0x63, 0xd2, 0xff));
        // Прозрачный пиксель остаётся прозрачным.
        assert_eq!(argb_to_color32(0).a(), 0);
    }

    /// Структура аптайма: часы+минуты / минуты+секунды / секунды.
    /// Сравниваем с fl!-рендером — не зависит от локали машины.
    #[test]
    fn uptime_formatting_structure() {
        assert_eq!(format_uptime(7), fl!("uptime-seconds", seconds = 7));
        assert_eq!(
            format_uptime(75),
            format!(
                "{} {}",
                fl!("uptime-minutes", minutes = 1),
                fl!("uptime-seconds", seconds = 15)
            )
        );
        assert_eq!(
            format_uptime(3700),
            format!(
                "{} {}",
                fl!("uptime-hours", hours = 1),
                fl!("uptime-minutes", minutes = 1)
            )
        );
        assert_eq!(
            format_uptime(7980),
            format!(
                "{} {}",
                fl!("uptime-hours", hours = 2),
                fl!("uptime-minutes", minutes = 13)
            )
        );
    }

    /// Русские плюральные формы CLDR (ТД-30): «1 минута / 2 минуты /
    /// 5 минут» — детерминированно, через явный ru-загрузчик.
    #[test]
    fn uptime_russian_plural_forms() {
        let ru = crate::i18n::loader_for("ru");
        let minutes = |n: u64| ru.get_args("uptime-minutes", HashMap::from([("minutes", n)]));
        assert_eq!(minutes(1), "1 минута");
        assert_eq!(minutes(2), "2 минуты");
        assert_eq!(minutes(5), "5 минут");
        assert_eq!(minutes(21), "21 минута");
        assert_eq!(minutes(64), "64 минуты");
        let hours = |n: u64| ru.get_args("uptime-hours", HashMap::from([("hours", n)]));
        assert_eq!(hours(1), "1 час");
        assert_eq!(hours(3), "3 часа");
        assert_eq!(hours(11), "11 часов");
        let seconds = |n: u64| ru.get_args("uptime-seconds", HashMap::from([("seconds", n)]));
        assert_eq!(seconds(1), "1 секунда");
        assert_eq!(seconds(15), "15 секунд");
    }

    /// Английские плюральные формы (фолбэк-язык обязан быть полным).
    #[test]
    fn uptime_english_plural_forms() {
        let en = crate::i18n::loader_for("en");
        let minutes = |n: u64| en.get_args("uptime-minutes", HashMap::from([("minutes", n)]));
        assert_eq!(minutes(1), "1 minute");
        assert_eq!(minutes(2), "2 minutes");
        let hours = |n: u64| en.get_args("uptime-hours", HashMap::from([("hours", n)]));
        assert_eq!(hours(1), "1 hour");
        assert_eq!(hours(5), "5 hours");
    }

    #[test]
    fn desktop_file_is_well_formed() {
        let text = desktop_file_content("/usr/bin/driftling");
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("Type=Application\n"));
        assert!(text.contains("Name=Driftling\n"));
        assert!(text.contains("Exec=/usr/bin/driftling\n"));
        assert!(text.contains("X-KDE-autostart-after=panel\n"));
        assert!(text.ends_with('\n'));
    }

    /// Exec экранируется по Desktop Entry spec: путь с пробелами/кавычками
    /// переживает оба уровня разбора (общий unescape + разбор аргументов).
    #[test]
    fn exec_quoting_survives_special_paths() {
        // Простой путь остаётся как есть.
        assert_eq!(exec_quote("/usr/bin/driftling"), "/usr/bin/driftling");
        assert_eq!(exec_quote("driftling"), "driftling");

        // Пробел: достаточно кавычек, внутри ничего не экранируется.
        assert_eq!(
            exec_quote("/opt/my apps/driftling"),
            "\"/opt/my apps/driftling\""
        );
        let text = desktop_file_content("/opt/my apps/driftling");
        assert!(text.contains("Exec=\"/opt/my apps/driftling\"\n"));

        // Кавычка: в файле `\\"` (общий unescape -> `\"` для Exec-разбора).
        assert_eq!(exec_quote(r#"/tmp/a"b"#), "\"/tmp/a\\\\\"b\"");
        // Доллар и бэктик — та же схема.
        assert_eq!(exec_quote("/tmp/a$b"), "\"/tmp/a\\\\$b\"");
        assert_eq!(exec_quote("/tmp/a`b"), "\"/tmp/a\\\\`b\"");
        // Литеральный бэкслеш: четыре в файле.
        assert_eq!(exec_quote(r"/tmp/a\b"), "\"/tmp/a\\\\\\\\b\"");
        // Пустая строка не даёт пустого Exec без кавычек.
        assert_eq!(exec_quote(""), "\"\"");
    }

    #[test]
    fn autostart_path_layout() {
        let p = autostart_path_in(Path::new("/home/u/.config"));
        assert_eq!(p, Path::new("/home/u/.config/autostart/driftling.desktop"));
    }

    #[test]
    fn daemon_exec_prefers_sibling_binary() {
        assert_eq!(daemon_exec_in(None), "driftling");
        assert_eq!(
            daemon_exec_in(Some(Path::new("/nonexistent-dir"))),
            "driftling"
        );

        let dir =
            std::env::temp_dir().join(format!("driftling-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("driftling");
        std::fs::write(&bin, b"").unwrap();
        assert_eq!(daemon_exec_in(Some(&dir)), bin.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Отображение стадий тотально и стабильно по именам ключей;
    /// локализованная подпись совпадает с fl!-рендером (не зависит от
    /// локали машины).
    #[test]
    fn stage_labels_map_to_expected_keys() {
        assert_eq!(stage_key(Stage::Egg), "stage-egg");
        assert_eq!(stage_key(Stage::Baby), "stage-baby");
        assert_eq!(stage_key(Stage::Child), "stage-child");
        assert_eq!(stage_key(Stage::Teen), "stage-teen");
        assert_eq!(stage_key(Stage::Adult), "stage-adult");

        assert_eq!(stage_label(Stage::Egg), fl!("stage-egg"));
        assert_eq!(stage_label(Stage::Baby), fl!("stage-baby"));
        assert_eq!(stage_label(Stage::Child), fl!("stage-child"));
        assert_eq!(stage_label(Stage::Teen), fl!("stage-teen"));
        assert_eq!(stage_label(Stage::Adult), fl!("stage-adult"));
        // Разные стадии не схлопываются в одну подпись.
        assert_ne!(stage_label(Stage::Egg), stage_label(Stage::Adult));
    }

    /// Пороги цвета баров ухода: >60 зелёный, 30..=60 янтарный, <30 красный.
    #[test]
    fn stat_bar_color_thresholds() {
        assert_eq!(stat_bar_color(100.0), SUCCESS);
        assert_eq!(stat_bar_color(60.1), SUCCESS);
        assert_eq!(stat_bar_color(60.0), AMBER);
        assert_eq!(stat_bar_color(45.0), AMBER);
        assert_eq!(stat_bar_color(30.0), AMBER);
        assert_eq!(stat_bar_color(29.9), DANGER);
        assert_eq!(stat_bar_color(0.0), DANGER);
    }

    /// Валидация имени: пустое/пробельное — отказ, валидное — trim.
    #[test]
    fn pet_name_validation() {
        assert_eq!(valid_pet_name(""), None);
        assert_eq!(valid_pet_name("   "), None);
        assert_eq!(valid_pet_name("\t\n"), None);
        assert_eq!(valid_pet_name("Дрифт"), Some("Дрифт"));
        assert_eq!(valid_pet_name("  Дрифт  "), Some("Дрифт"));
        assert_eq!(valid_pet_name("a"), Some("a"));
    }

    #[test]
    fn set_autostart_writes_and_removes_desktop_file() {
        let dir =
            std::env::temp_dir().join(format!("driftling-autostart-test-{}", std::process::id()));
        let path = autostart_path_in(&dir);

        set_autostart_at(&path, true, "/usr/bin/driftling").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, desktop_file_content("/usr/bin/driftling"));

        set_autostart_at(&path, false, "/usr/bin/driftling").unwrap();
        assert!(!path.exists());
        set_autostart_at(&path, false, "/usr/bin/driftling").unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }
}
