//! Окно настроек Driftling (ТЗ §3.3, этап M0.5).
//!
//! eframe/egui-приложение: статус демона (фоновый опрос раз в секунду),
//! кнопки управления (Призвать / Убрать / Остановить демон), форма настроек
//! поверх `driftling_core::Config` с «Сохранить и применить» (save + Reload
//! по IPC) и «Сбросить», тумблер автозапуска через
//! `~/.config/autostart/driftling.desktop`. Все подписи — на русском;
//! кириллицу гарантируем подгрузкой системного шрифта (DejaVu/Noto),
//! при его отсутствии остаёмся на встроенных шрифтах egui.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use driftling_core::Config;
use driftling_ipc::{call, Request, Response};

// ---------------------------------------------------------------------------
// Чистая логика (тестируется юнит-тестами внизу файла)
// ---------------------------------------------------------------------------

/// Содержимое autostart-файла для KDE: запускаем демона после панели.
fn desktop_file_content(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Driftling\n\
         Exec={exec}\n\
         X-KDE-autostart-after=panel\n"
    )
}

/// Путь autostart-файла внутри каталога конфигов.
fn autostart_path_in(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join("driftling.desktop")
}

/// `$XDG_CONFIG_HOME` либо `~/.config` — как в driftling_core::config.
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn autostart_path() -> PathBuf {
    autostart_path_in(&config_home())
}

/// Exec для autostart: бинарь демона рядом с текущим бинарём настроек,
/// если он там есть; иначе полагаемся на `driftling` в PATH.
fn daemon_exec_in(settings_dir: Option<&Path>) -> String {
    settings_dir
        .map(|d| d.join("driftling"))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "driftling".to_string())
}

fn daemon_exec() -> String {
    let exe = std::env::current_exe().ok();
    daemon_exec_in(exe.as_deref().and_then(Path::parent))
}

/// Привести значения формы к допустимым диапазонам ТЗ; min сна никогда
/// не превышает max (правим max, т.к. min пользователь двигал последним
/// осознанно — оба поля всё равно остаются в 1..=3600).
fn clamp_form(cfg: &mut Config) {
    cfg.pet.size = cfg.pet.size.clamp(32, 256);
    let b = &mut cfg.behavior;
    b.walk_speed = b.walk_speed.clamp(5.0, 400.0);
    b.w_idle_to_walk = b.w_idle_to_walk.min(100);
    b.w_idle_to_sleep = b.w_idle_to_sleep.min(100);
    b.sleep_min = b.sleep_min.clamp(1.0, 3600.0);
    b.sleep_max = b.sleep_max.clamp(b.sleep_min, 3600.0);
}

/// Аптайм демона в человекочитаемом виде.
fn format_uptime(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h} ч {m} мин")
    } else if m > 0 {
        format!("{m} мин {s} с")
    } else {
        format!("{s} с")
    }
}

// ---------------------------------------------------------------------------
// Состояние и потоки
// ---------------------------------------------------------------------------

/// Последний известный статус демона (пишет фоновый поток, читает UI).
enum DaemonStatus {
    /// Ещё не опрашивали.
    Unknown,
    /// Сокет недоступен (демон не запущен) или ошибка обмена.
    Down,
    /// Демон ответил на Status.
    Up {
        pets: u32,
        state: String,
        uptime_secs: u64,
    },
}

/// Фоновый опрос статуса раз в секунду — блокирующий IPC не должен
/// выполняться в UI-потоке. Поток живёт до конца процесса.
fn spawn_status_poller(slot: Arc<Mutex<DaemonStatus>>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || loop {
        let status = match call(&Request::Status) {
            Ok(Response::Status {
                pets,
                state,
                uptime_secs,
            }) => DaemonStatus::Up {
                pets,
                state,
                uptime_secs,
            },
            Ok(_) => DaemonStatus::Down,
            Err(_) => DaemonStatus::Down,
        };
        *slot.lock().unwrap() = status;
        ctx.request_repaint();
        std::thread::sleep(Duration::from_secs(1));
    });
}

/// Разовая IPC-команда в короткоживущем потоке; результат — в строку статуса.
fn spawn_action(
    req: Request,
    ok_text: &'static str,
    slot: Arc<Mutex<Option<String>>>,
    ctx: eframe::egui::Context,
) {
    std::thread::spawn(move || {
        let text = match call(&req) {
            Ok(Response::Error(e)) => format!("Ошибка демона: {e}"),
            Ok(_) => ok_text.to_string(),
            Err(e) => format!("Ошибка: {e:#}"),
        };
        *slot.lock().unwrap() = Some(text);
        ctx.request_repaint();
    });
}

/// «Сохранить и применить»: запись конфига + Reload демону, в потоке.
fn spawn_save_and_apply(cfg: Config, slot: Arc<Mutex<Option<String>>>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let text = match cfg.save() {
            Err(e) => format!("Не удалось сохранить: {e}"),
            Ok(()) => match call(&Request::Reload) {
                Ok(Response::Error(e)) => format!("Сохранено, но демон ответил ошибкой: {e}"),
                Ok(_) => "Применено".to_string(),
                Err(_) => "Сохранено (демон не запущен — применится при старте)".to_string(),
            },
        };
        *slot.lock().unwrap() = Some(text);
        ctx.request_repaint();
    });
}

/// Включить/выключить автозапуск: создать или удалить .desktop-файл.
fn set_autostart(enable: bool) -> Result<(), String> {
    set_autostart_at(&autostart_path(), enable, &daemon_exec())
}

/// Та же логика с явными путями — для юнит-тестов.
fn set_autostart_at(path: &Path, enable: bool, exec: &str) -> Result<(), String> {
    if enable {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, desktop_file_content(exec)).map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Приложение eframe
// ---------------------------------------------------------------------------

/// Кандидаты системных шрифтов с кириллицей (Debian): встроенные шрифты
/// egui покрывают её не полностью, поэтому первым в Proportional ставим
/// системный. Ни один не нашёлся — молча остаёмся на дефолтных.
const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
];

fn install_cyrillic_font(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};
    for path in FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "system-cyrillic".into(),
                Arc::new(FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "system-cyrillic".into());
            ctx.set_fonts(fonts);
            return;
        }
    }
    log::warn!("системный шрифт с кириллицей не найден, остаёмся на встроенных");
}

struct SettingsApp {
    cfg: Config,
    /// Ошибка чтения config.toml при старте (битый TOML) — показываем в UI.
    load_error: Option<String>,
    status: Arc<Mutex<DaemonStatus>>,
    /// Результат последней команды/сохранения.
    action_result: Arc<Mutex<Option<String>>>,
    autostart: bool,
}

impl SettingsApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cyrillic_font(&cc.egui_ctx);

        let (cfg, load_error) = match Config::load() {
            Ok(cfg) => (cfg, None),
            Err(e) => (Config::default(), Some(e)),
        };

        let status = Arc::new(Mutex::new(DaemonStatus::Unknown));
        spawn_status_poller(Arc::clone(&status), cc.egui_ctx.clone());

        Self {
            cfg,
            load_error,
            status,
            action_result: Arc::new(Mutex::new(None)),
            autostart: autostart_path().exists(),
        }
    }

    fn status_panel(&self, ui: &mut eframe::egui::Ui) {
        use eframe::egui::{Color32, RichText};
        ui.heading("Демон");
        match &*self.status.lock().unwrap() {
            DaemonStatus::Unknown => {
                ui.label("Проверяем состояние…");
            }
            DaemonStatus::Down => {
                ui.label(RichText::new("Демон не запущен").color(Color32::LIGHT_RED));
            }
            DaemonStatus::Up {
                pets,
                state,
                uptime_secs,
            } => {
                ui.label(RichText::new("Демон работает").color(Color32::LIGHT_GREEN));
                ui.label(format!(
                    "Питомцев: {pets} · Состояние: {state} · Аптайм: {}",
                    format_uptime(*uptime_secs)
                ));
            }
        }
    }

    fn control_buttons(&mut self, ui: &mut eframe::egui::Ui) {
        ui.horizontal(|ui| {
            let actions: [(&str, Request, &'static str); 3] = [
                ("Призвать", Request::Summon, "Питомец призван"),
                ("Убрать", Request::Dismiss, "Питомец убран"),
                ("Остановить демон", Request::Quit, "Демон остановлен"),
            ];
            for (label, req, ok_text) in actions {
                if ui.button(label).clicked() {
                    spawn_action(
                        req,
                        ok_text,
                        Arc::clone(&self.action_result),
                        ui.ctx().clone(),
                    );
                }
            }
        });
    }

    fn settings_form(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui::{DragValue, Slider};
        ui.heading("Настройки");
        if let Some(err) = &self.load_error {
            ui.colored_label(
                eframe::egui::Color32::LIGHT_RED,
                format!("Ошибка чтения конфига (показаны дефолты): {err}"),
            );
        }

        let b = &mut self.cfg.behavior;
        ui.add(Slider::new(&mut self.cfg.pet.size, 32..=256).text("Размер питомца, px"));
        ui.add(Slider::new(&mut b.walk_speed, 5.0..=400.0).text("Скорость ходьбы, px/с"));
        ui.add(Slider::new(&mut b.w_idle_to_walk, 0..=100).text("Непоседливость"));
        ui.add(Slider::new(&mut b.w_idle_to_sleep, 0..=100).text("Сонливость"));
        ui.horizontal(|ui| {
            ui.label("Сон, сек:");
            ui.add(
                DragValue::new(&mut b.sleep_min)
                    .range(1.0..=3600.0)
                    .prefix("от "),
            );
            ui.add(
                DragValue::new(&mut b.sleep_max)
                    .range(1.0..=3600.0)
                    .prefix("до "),
            );
        });
        // Держим форму в допустимых пределах, включая min <= max сна.
        clamp_form(&mut self.cfg);
    }

    fn save_buttons(&mut self, ui: &mut eframe::egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Сохранить и применить").clicked() {
                spawn_save_and_apply(
                    self.cfg.clone(),
                    Arc::clone(&self.action_result),
                    ui.ctx().clone(),
                );
            }
            if ui.button("Сбросить").clicked() {
                // Только форма: на диск попадёт лишь после «применить».
                self.cfg = Config::default();
                *self.action_result.lock().unwrap() =
                    Some("Дефолты в форме (не сохранены)".to_string());
            }
        });
    }

    fn autostart_toggle(&mut self, ui: &mut eframe::egui::Ui) {
        if ui
            .checkbox(&mut self.autostart, "Автозапуск при входе в сессию")
            .changed()
        {
            let result = match set_autostart(self.autostart) {
                Ok(()) if self.autostart => "Автозапуск включён".to_string(),
                Ok(()) => "Автозапуск выключен".to_string(),
                Err(e) => {
                    // Не вышло — возвращаем галку к фактическому состоянию.
                    self.autostart = autostart_path().exists();
                    format!("Ошибка автозапуска: {e}")
                }
            };
            *self.action_result.lock().unwrap() = Some(result);
        }
    }
}

impl eframe::App for SettingsApp {
    // В eframe 0.36 App отдаёт готовый Ui центральной панели.
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.status_panel(ui);
        self.control_buttons(ui);
        ui.separator();
        self.settings_form(ui);
        ui.separator();
        self.save_buttons(ui);
        self.autostart_toggle(ui);
        if let Some(text) = &*self.action_result.lock().unwrap() {
            ui.separator();
            ui.label(text);
        }
    }
}

fn main() -> eframe::Result {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([420.0, 520.0])
            .with_min_inner_size([380.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Driftling — настройки",
        options,
        Box::new(|cc| Ok(Box::new(SettingsApp::new(cc)))),
    )
}

// ---------------------------------------------------------------------------
// Тесты чистой логики
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_file_is_well_formed() {
        let text = desktop_file_content("/usr/bin/driftling");
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("Type=Application\n"));
        assert!(text.contains("Name=Driftling\n"));
        assert!(text.contains("Exec=/usr/bin/driftling\n"));
        assert!(text.contains("X-KDE-autostart-after=panel\n"));
        assert!(text.ends_with('\n'));
        assert!(!text.contains("OnlyShowIn"));
    }

    #[test]
    fn autostart_path_layout() {
        let p = autostart_path_in(Path::new("/home/u/.config"));
        assert_eq!(p, Path::new("/home/u/.config/autostart/driftling.desktop"));
    }

    #[test]
    fn daemon_exec_prefers_sibling_binary() {
        // Каталога нет / бинаря рядом нет — фолбэк на PATH.
        assert_eq!(daemon_exec_in(None), "driftling");
        assert_eq!(
            daemon_exec_in(Some(Path::new("/nonexistent-dir"))),
            "driftling"
        );

        // Есть файл driftling рядом — берём абсолютный путь к нему.
        let dir =
            std::env::temp_dir().join(format!("driftling-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("driftling");
        std::fs::write(&bin, b"").unwrap();
        assert_eq!(daemon_exec_in(Some(&dir)), bin.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clamp_form_enforces_ranges_and_sleep_order() {
        let mut cfg = Config::default();
        cfg.pet.size = 9000;
        cfg.behavior.walk_speed = -5.0;
        cfg.behavior.w_idle_to_walk = 500;
        cfg.behavior.w_idle_to_sleep = 101;
        cfg.behavior.sleep_min = 100.0;
        cfg.behavior.sleep_max = 1.0;
        clamp_form(&mut cfg);
        assert_eq!(cfg.pet.size, 256);
        assert_eq!(cfg.behavior.walk_speed, 5.0);
        assert_eq!(cfg.behavior.w_idle_to_walk, 100);
        assert_eq!(cfg.behavior.w_idle_to_sleep, 100);
        // min <= max: max подтянут к min.
        assert_eq!(cfg.behavior.sleep_min, 100.0);
        assert_eq!(cfg.behavior.sleep_max, 100.0);

        // Корректные значения не трогаем.
        let mut ok = Config::default();
        let before = ok.clone();
        clamp_form(&mut ok);
        assert_eq!(ok, before);
    }

    #[test]
    fn set_autostart_writes_and_removes_desktop_file() {
        let dir =
            std::env::temp_dir().join(format!("driftling-autostart-test-{}", std::process::id()));
        let path = autostart_path_in(&dir);

        // Включение создаёт каталог и корректный .desktop.
        set_autostart_at(&path, true, "/usr/bin/driftling").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, desktop_file_content("/usr/bin/driftling"));

        // Выключение удаляет файл; повторное выключение — не ошибка.
        set_autostart_at(&path, false, "/usr/bin/driftling").unwrap();
        assert!(!path.exists());
        set_autostart_at(&path, false, "/usr/bin/driftling").unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uptime_formatting() {
        assert_eq!(format_uptime(7), "7 с");
        assert_eq!(format_uptime(75), "1 мин 15 с");
        assert_eq!(format_uptime(3700), "1 ч 1 мин");
    }
}
