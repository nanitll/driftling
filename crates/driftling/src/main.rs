//! driftling: без аргументов — демон (питомец на экране);
//! `driftling ctl <cmd>` — управление запущенным демоном.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod daemon;
mod i18n;
mod tray;

use i18n::fl;

#[derive(Parser)]
#[command(name = "driftling", version, about = fl!("cli-about"))]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    // Управление запущенным демоном.
    #[command(about = fl!("cli-about-ctl"))]
    Ctl {
        #[command(subcommand)]
        action: CtlAction,
    },
    // Открыть окно настроек (запускает driftling-settings).
    #[command(about = fl!("cli-about-settings"))]
    Settings,
}

#[derive(Subcommand)]
enum CtlAction {
    // Позвать питомца на экран.
    #[command(about = fl!("cli-about-summon"))]
    Summon,
    // Убрать питомца с экрана (демон продолжает работать).
    #[command(about = fl!("cli-about-dismiss"))]
    Dismiss,
    // Показать состояние.
    #[command(about = fl!("cli-about-status"))]
    Status,
    // Карточка питомца: имя, стадия, статы, характеристики.
    #[command(about = fl!("cli-about-info"))]
    Info,
    // Покормить питомца (--treat — вкусняшка).
    #[command(about = fl!("cli-about-feed"))]
    Feed {
        #[arg(long, help = fl!("cli-about-feed-treat"))]
        treat: bool,
    },
    // Поиграть с питомцем.
    #[command(about = fl!("cli-about-play"))]
    Play,
    // Уложить питомца спать.
    #[command(about = fl!("cli-about-sleep"))]
    Sleep,
    // Переименовать питомца (= событие журнала).
    #[command(about = fl!("cli-about-rename"))]
    Rename { name: String },
    // Перечитать конфиг и применить на лету.
    #[command(about = fl!("cli-about-reload"))]
    Reload,
    // Остановить демон.
    #[command(about = fl!("cli-about-quit"))]
    Quit,
    // Диагностика: окружение, сокет, файлы, автозапуск (работает без демона).
    #[command(about = fl!("cli-about-doctor"))]
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Логирование демона настраивает daemon::run() сам (journald под
    // systemd, ТД-19) — env_logger здесь перехватил бы его первым.
    if cli.command.is_some() {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }
    match cli.command {
        None => daemon::run(),
        Some(Command::Settings) => {
            // Ищем driftling-settings рядом с собой (обычная раскладка
            // установки), затем в PATH.
            let sibling = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("driftling-settings")))
                .filter(|p| p.exists());
            let program = sibling.unwrap_or_else(|| "driftling-settings".into());
            let status = std::process::Command::new(&program).status().map_err(|e| {
                anyhow::anyhow!(fl!(
                    "settings-launch-failed",
                    program = format!("{program:?}"),
                    error = e.to_string()
                ))
            })?;
            anyhow::ensure!(status.success(), fl!("settings-exited-with-error"));
            Ok(())
        }
        Some(Command::Ctl { action }) => {
            let req = match action {
                CtlAction::Summon => driftling_ipc::Request::Summon,
                CtlAction::Dismiss => driftling_ipc::Request::Dismiss,
                CtlAction::Status => driftling_ipc::Request::Status,
                CtlAction::Info => driftling_ipc::Request::PetInfo,
                CtlAction::Feed { treat } => driftling_ipc::Request::Feed { treat },
                CtlAction::Play => driftling_ipc::Request::Play,
                CtlAction::Sleep => driftling_ipc::Request::PutToSleep,
                CtlAction::Rename { name } => driftling_ipc::Request::Rename(name),
                CtlAction::Reload => driftling_ipc::Request::Reload,
                CtlAction::Quit => driftling_ipc::Request::Quit,
                CtlAction::Doctor => return doctor(),
            };
            match driftling_ipc::call(&req)? {
                driftling_ipc::Response::Ok => println!("{}", fl!("ctl-ok")),
                driftling_ipc::Response::Status {
                    pets,
                    state,
                    uptime_secs,
                } => {
                    println!(
                        "{}",
                        fl!(
                            "ctl-status",
                            pets = pets,
                            state = state,
                            uptime = uptime_secs
                        )
                    );
                }
                driftling_ipc::Response::PetInfo {
                    name,
                    state,
                    attributes,
                    stats,
                    stage,
                    uptime_secs,
                } => {
                    let state = state.unwrap_or_else(|| fl!("state-dismissed"));
                    println!(
                        "{}",
                        fl!(
                            "ctl-petinfo",
                            name = name,
                            stage = stage_name(stage),
                            state = state,
                            uptime = uptime_secs
                        )
                    );
                    println!(
                        "{}",
                        fl!(
                            "ctl-petinfo-stats",
                            satiety = format!("{:.0}", stats.satiety),
                            energy = format!("{:.0}", stats.energy),
                            mood = format!("{:.0}", stats.mood),
                            health = format!("{:.0}", stats.health)
                        )
                    );
                    println!(
                        "{}",
                        fl!(
                            "ctl-petinfo-attributes",
                            attributes = format!("{attributes:?}")
                        )
                    );
                }
                driftling_ipc::Response::Error(e) => anyhow::bail!(e),
            }
            Ok(())
        }
    }
}

/// Локализованное имя стадии роста (демон шлёт машинное значение, ТД-30).
fn stage_name(stage: driftling_core::Stage) -> String {
    use driftling_core::Stage;
    match stage {
        Stage::Egg => fl!("stage-egg"),
        Stage::Baby => fl!("stage-baby"),
        Stage::Child => fl!("stage-child"),
        Stage::Teen => fl!("stage-teen"),
        Stage::Adult => fl!("stage-adult"),
    }
}

/// Одна строка чек-листа доктора.
fn check(ok: bool, msg: &str) {
    println!("  {} {msg}", if ok { "✓" } else { "✗" });
}

/// `driftling ctl doctor`: человекочитаемый чек-лист окружения (П-12, ТД-19).
/// Работает без демона. Выход с кодом 1, если демон должен работать
/// (автозапуск настроен или сокет существует), но не отвечает.
fn doctor() -> Result<()> {
    use std::path::PathBuf;

    println!("{}\n", fl!("doctor-title"));

    // 1. Wayland-сессия.
    match std::env::var("WAYLAND_DISPLAY") {
        Ok(v) => check(true, &fl!("doctor-wayland-ok", value = v)),
        Err(_) => check(false, &fl!("doctor-wayland-missing")),
    }

    // 2. XDG_RUNTIME_DIR + управляющий сокет.
    let mut socket_exists = false;
    let mut daemon_answering = false;
    match driftling_ipc::runtime_dir() {
        Ok(dir) => {
            check(
                true,
                &fl!("doctor-runtime-dir", path = dir.display().to_string()),
            );
            let sock = driftling_ipc::socket_path_in(&dir);
            socket_exists = sock.exists();
            if socket_exists {
                match driftling_ipc::call(&driftling_ipc::Request::Status) {
                    Ok(driftling_ipc::Response::Status {
                        pets,
                        state,
                        uptime_secs,
                    }) => {
                        daemon_answering = true;
                        check(
                            true,
                            &fl!(
                                "doctor-daemon-answering",
                                version = driftling_ipc::PROTOCOL_VERSION,
                                pets = pets,
                                state = state,
                                uptime = uptime_secs
                            ),
                        );
                    }
                    Ok(other) => {
                        daemon_answering = true;
                        check(
                            true,
                            &fl!(
                                "doctor-daemon-unexpected-reply",
                                reply = format!("{other:?}")
                            ),
                        );
                    }
                    // Сюда же попадает несовпадение версии протокола —
                    // call() сам объясняет обе версии и советует рестарт.
                    Err(e) => check(
                        false,
                        &fl!(
                            "doctor-daemon-not-answering",
                            path = sock.display().to_string(),
                            error = format!("{e:#}")
                        ),
                    ),
                }
            } else {
                check(
                    false,
                    &fl!("doctor-socket-missing", path = sock.display().to_string()),
                );
            }
        }
        Err(e) => check(
            false,
            &fl!("doctor-socket-uncheckable", error = format!("{e:#}")),
        ),
    }

    // 3. pet.json (v3: только device_id) + журнал событий — источник
    // истины о питомце. Имя берём свёрткой журнала. load() при v1/v2
    // сам мигрирует содержимое в журнал — то же сделал бы демон при
    // старте, операция идемпотентна (ТД-15).
    let pet_path = driftling_core::attributes::record_path();
    match driftling_core::PetRecord::load() {
        Ok(Some(_rec)) => {
            let data_dir = driftling_core::attributes::data_dir();
            match driftling_core::Journal::open(&data_dir) {
                Ok((events, warnings)) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let pet =
                        driftling_core::fold(&events, now_ms, &driftling_core::FoldCfg::default());
                    check(
                        true,
                        &fl!(
                            "doctor-pet-ok",
                            name = pet.name,
                            path = pet_path.display().to_string()
                        ),
                    );
                    check(
                        warnings == 0,
                        &fl!(
                            "doctor-journal",
                            events = (events.len() as u64),
                            warnings = warnings,
                            path = driftling_core::journal_path_in(&data_dir)
                                .display()
                                .to_string()
                        ),
                    );
                }
                Err(e) => check(false, &fl!("doctor-journal-unreadable", error = e)),
            }
        }
        Ok(None) => check(
            false,
            &fl!("doctor-pet-missing", path = pet_path.display().to_string()),
        ),
        Err(e) => check(
            false,
            &fl!(
                "doctor-pet-unreadable",
                path = pet_path.display().to_string(),
                error = e
            ),
        ),
    }

    // 4. config.toml.
    let cfg_path = driftling_core::config::path();
    if cfg_path.exists() {
        match driftling_core::Config::load() {
            Ok(_) => check(
                true,
                &fl!("doctor-config-ok", path = cfg_path.display().to_string()),
            ),
            Err(e) => check(
                false,
                &fl!(
                    "doctor-config-broken",
                    path = cfg_path.display().to_string(),
                    error = e
                ),
            ),
        }
    } else {
        check(
            true,
            &fl!(
                "doctor-config-missing",
                path = cfg_path.display().to_string()
            ),
        );
    }

    // 5. Автозапуск: systemd user unit или .desktop в autostart.
    let unit_enabled = std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "driftling.service"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let desktop_path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("autostart").join("driftling.desktop"));
    let desktop_present = desktop_path.as_deref().is_some_and(|p| p.exists());
    let autostart = unit_enabled || desktop_present;
    if unit_enabled {
        check(true, &fl!("doctor-autostart-systemd"));
    } else if desktop_present {
        check(
            true,
            &fl!(
                "doctor-autostart-desktop",
                path = desktop_path
                    .expect("desktop_present ⇒ путь есть")
                    .display()
                    .to_string()
            ),
        );
    } else {
        check(false, &fl!("doctor-autostart-none"));
    }

    // Вердикт.
    println!();
    if daemon_answering {
        println!("{}", fl!("doctor-verdict-ok"));
    } else if autostart || socket_exists {
        println!("{}", fl!("doctor-verdict-should-run"));
        std::process::exit(1);
    } else {
        println!("{}", fl!("doctor-verdict-not-running"));
    }
    Ok(())
}
