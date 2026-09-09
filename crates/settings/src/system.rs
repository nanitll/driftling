//! Связь с системой: автозапуск (systemd user unit или .desktop), пути,
//! запуск демона и открытие файлов в файловом менеджере. Всё блокирующее
//! вызывается из фоновых потоков (см. actions).

use std::path::{Path, PathBuf};

use crate::i18n::fl;

pub fn exec_quote(arg: &str) -> String {
    let simple = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':' | '+'));
    if simple {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        match c {
            // Уровень Exec-разбора требует `\"`; чтобы `\` пережил общий
            // unescape, в файле это `\\"`.
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            // Литеральный бэкслеш: `\\` на уровне Exec = `\\\\` в файле.
            '\\' => out.push_str("\\\\\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Содержимое autostart-файла для KDE: запускаем демона после панели.
pub fn desktop_file_content(exec: &str) -> String {
    let exec = exec_quote(exec);
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Driftling\n\
         Exec={exec}\n\
         X-KDE-autostart-after=panel\n"
    )
}

/// Путь autostart-файла внутри каталога конфигов.
pub fn autostart_path_in(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join("driftling.desktop")
}

/// `$XDG_CONFIG_HOME` либо `~/.config` — как в driftling_core::config.
pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn autostart_path() -> PathBuf {
    autostart_path_in(&config_home())
}

/// Exec для autostart: бинарь демона рядом с текущим бинарём настроек,
/// если он там есть; иначе полагаемся на `driftling` в PATH.
pub fn daemon_exec_in(settings_dir: Option<&Path>) -> String {
    settings_dir
        .map(|d| d.join("driftling"))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "driftling".to_string())
}

pub fn daemon_exec() -> String {
    let exe = std::env::current_exe().ok();
    daemon_exec_in(exe.as_deref().and_then(Path::parent))
}

/// Каким механизмом управлять автозапуском (ТД-28): предпочитаем systemd
/// user unit (Restart=on-failure, journald), .desktop — фолбэк.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartBackend {
    /// Установлен dist/driftling.service — рулим через systemctl --user.
    SystemdUnit,
    /// Юнита нет (или нет systemd) — XDG autostart .desktop-файл.
    DesktopFile,
}

/// Определить бэкенд: юнит считается установленным, если systemd его видит.
pub fn autostart_backend() -> AutostartBackend {
    let installed = std::process::Command::new("systemctl")
        .args(["--user", "list-unit-files", "driftling.service"])
        .output()
        .map(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains("driftling.service")
        })
        .unwrap_or(false);
    if installed {
        AutostartBackend::SystemdUnit
    } else {
        AutostartBackend::DesktopFile
    }
}

/// systemctl --user enable/disable driftling.service.
pub fn set_autostart_systemd(enable: bool) -> Result<(), String> {
    let action = if enable { "enable" } else { "disable" };
    let out = std::process::Command::new("systemctl")
        .args(["--user", action, "driftling.service"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Текущее фактическое состояние автозапуска (для тумблера).
pub fn autostart_enabled() -> bool {
    match autostart_backend() {
        AutostartBackend::SystemdUnit => std::process::Command::new("systemctl")
            .args(["--user", "is-enabled", "--quiet", "driftling.service"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        AutostartBackend::DesktopFile => autostart_path().exists(),
    }
}

/// Включить/выключить автозапуск выбранным бэкендом.
pub fn set_autostart(enable: bool) -> Result<(), String> {
    match autostart_backend() {
        AutostartBackend::SystemdUnit => set_autostart_systemd(enable),
        AutostartBackend::DesktopFile => {
            set_autostart_at(&autostart_path(), enable, &daemon_exec())
        }
    }
}

/// Та же логика с явными путями — для юнит-тестов.
pub fn set_autostart_at(path: &Path, enable: bool, exec: &str) -> Result<(), String> {
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
// Фоновый опрос демона
// ---------------------------------------------------------------------------

/// Развернуть «~» в домашний каталог — путь синка вводится руками.
pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~") {
        Some(rest) => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(rest.trim_start_matches('/'))
        }
        None => PathBuf::from(path),
    }
}

/// Открыть файл или каталог системным способом. Молча: если открывать
/// нечем, окно всё равно показывает путь текстом.
pub fn open_path(path: &str) {
    let path = expand_tilde(path);
    std::thread::spawn(move || {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    });
}

/// Запустить демона (бинарь рядом с окном), не дожидаясь его.
pub fn start_daemon() -> String {
    match std::process::Command::new(daemon_exec()).spawn() {
        Ok(_) => fl!("msg-daemon-starting"),
        Err(e) => fl!("generic-error", error = e.to_string()),
    }
}

/// Прогнать `ctl doctor` в фоне и положить вывод в слот. С таймаутом:
/// доктор ходит в сеть, и окно не должно висеть из-за него.
pub fn run_doctor(
    slot: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    ctx: eframe::egui::Context,
) {
    *slot.lock().unwrap() = None;
    std::thread::spawn(move || {
        let out = std::process::Command::new(daemon_exec())
            .args(["ctl", "doctor"])
            .output();
        let text = match out {
            Ok(o) => {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&o.stderr));
                if text.trim().is_empty() {
                    fl!("service-doctor-empty")
                } else {
                    text
                }
            }
            Err(e) => fl!("generic-error", error = e.to_string()),
        };
        *slot.lock().unwrap() = Some(text);
        ctx.request_repaint();
    });
}
