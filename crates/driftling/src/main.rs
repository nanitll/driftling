//! driftling: без аргументов — демон (питомец на экране);
//! `driftling ctl <cmd>` — управление запущенным демоном.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod daemon;
mod i18n;
mod sync;
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
    // Прокатить питомца на транспорте.
    #[command(about = fl!("cli-about-ride"))]
    Ride {
        #[arg(help = fl!("cli-about-ride-kind"))]
        kind: Option<String>,
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
    // Перекрасить питомца: hex-цвет #rrggbb или rrggbb (= событие журнала).
    #[command(about = fl!("cli-about-recolor"))]
    Recolor { color: String },
    // Перечитать конфиг и применить на лету.
    #[command(about = fl!("cli-about-reload"))]
    Reload,
    // Синхронизация между устройствами (фаза E).
    #[command(about = fl!("cli-about-sync"))]
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },
    // Остановить демон.
    #[command(about = fl!("cli-about-quit"))]
    Quit,
    // Диагностика: окружение, сокет, файлы, автозапуск (работает без демона).
    #[command(about = fl!("cli-about-doctor"))]
    Doctor,
}

#[derive(Subcommand)]
enum SyncAction {
    // Статус синка: режим, последние push/pull, курсоры, lease.
    #[command(about = fl!("cli-about-sync-status"))]
    Status,
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
                CtlAction::Ride { kind } => driftling_ipc::Request::Ride { kind },
                CtlAction::Rename { name } => driftling_ipc::Request::Rename(name),
                CtlAction::Recolor { color } => match parse_hex_color(&color) {
                    Some(argb) => driftling_ipc::Request::Recolor(argb),
                    None => anyhow::bail!(fl!("ctl-recolor-bad-hex", value = color)),
                },
                CtlAction::Reload => driftling_ipc::Request::Reload,
                CtlAction::Sync {
                    action: SyncAction::Status,
                } => driftling_ipc::Request::SyncStatus,
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
                    color,
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
                            "ctl-petinfo-color",
                            color = format!("#{:06x}", color & 0x00ff_ffff)
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
                driftling_ipc::Response::SyncStatus {
                    mode,
                    address,
                    folder,
                    last_push_secs,
                    last_pull_secs,
                    devices,
                    events,
                    holding,
                    holder,
                    last_error,
                } => print_sync_status(
                    &mode,
                    address.as_deref(),
                    folder.as_deref(),
                    last_push_secs,
                    last_pull_secs,
                    devices,
                    events,
                    holding,
                    holder.as_deref(),
                    last_error.as_deref(),
                ),
                driftling_ipc::Response::Error(e) => anyhow::bail!(e),
            }
            Ok(())
        }
    }
}

/// Человекочитаемый вывод `ctl sync status` (машинные значения демона
/// локализуются здесь, ТД-30).
#[allow(clippy::too_many_arguments)]
fn print_sync_status(
    mode: &str,
    address: Option<&str>,
    folder: Option<&str>,
    last_push_secs: Option<u64>,
    last_pull_secs: Option<u64>,
    devices: u32,
    events: u64,
    holding: bool,
    holder: Option<&str>,
    last_error: Option<&str>,
) {
    let mode_name = match mode {
        "off" => fl!("sync-mode-off"),
        "server" => fl!("sync-mode-server"),
        "folder" => fl!("sync-mode-folder"),
        other => other.to_string(),
    };
    let target = address.or(folder).unwrap_or_default();
    if target.is_empty() {
        println!("{}", fl!("ctl-sync-mode", mode = mode_name));
    } else {
        println!(
            "{}",
            fl!("ctl-sync-mode-target", mode = mode_name, target = target)
        );
    }
    if mode == "off" {
        return;
    }
    let ago = |secs: Option<u64>| match secs {
        Some(s) => fl!("ctl-sync-ago", secs = s),
        None => fl!("ctl-sync-never"),
    };
    println!(
        "{}",
        fl!(
            "ctl-sync-transfers",
            push = ago(last_push_secs),
            pull = ago(last_pull_secs)
        )
    );
    println!(
        "{}",
        fl!("ctl-sync-journal", events = events, devices = devices)
    );
    if mode == "server" {
        let line = match (holding, holder) {
            (true, _) => fl!("ctl-sync-lease-ours"),
            (false, Some(h)) if !h.is_empty() => fl!("ctl-sync-lease-holder", holder = h),
            _ => fl!("ctl-sync-lease-unknown"),
        };
        println!("{line}");
    }
    if let Some(e) = last_error {
        println!("{}", fl!("ctl-sync-error", error = e));
    }
}

/// Разобрать пользовательский hex-цвет `#rrggbb`/`rrggbb` в ARGB
/// (альфа ff). Кривой ввод — None, дальше внятная ошибка ctl.
fn parse_hex_color(s: &str) -> Option<u32> {
    let hex = s.trim();
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16)
        .ok()
        .map(|rgb| 0xff00_0000 | rgb)
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

    // Конфиг читается один раз: печать в п.4 и секция [sync] для
    // каталога журналов (режим «папки») и проверок синка в п.5.
    let cfg_path = driftling_core::config::path();
    let cfg_loaded = driftling_core::Config::load();
    let sync_cfg = cfg_loaded
        .as_ref()
        .map(|c| c.sync.clone())
        .unwrap_or_default();

    // 3. pet.json (v3: только device_id) + журнал событий — источник
    // истины о питомце. Имя берём свёрткой журнала. load() при v1/v2
    // сам мигрирует содержимое в журнал — то же сделал бы демон при
    // старте, операция идемпотентна (ТД-15).
    let pet_path = driftling_core::attributes::record_path();
    let data_dir = driftling_core::attributes::data_dir();
    // Каталог журналов — как его увидит демон (sync.folder в режиме папки).
    let folder = sync_cfg.folder.trim();
    let journal_dir = if sync_cfg.mode == driftling_core::SyncMode::Folder && !folder.is_empty() {
        std::path::PathBuf::from(folder)
    } else {
        data_dir.clone()
    };
    match driftling_core::PetRecord::load() {
        Ok(Some(rec)) => match driftling_core::Journal::open(&journal_dir) {
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
                        path = driftling_core::device_journal_path_in(&journal_dir, &rec.device_id)
                            .display()
                            .to_string()
                    ),
                );
            }
            Err(e) => check(false, &fl!("doctor-journal-unreadable", error = e)),
        },
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
    if cfg_path.exists() {
        match &cfg_loaded {
            Ok(_) => check(
                true,
                &fl!("doctor-config-ok", path = cfg_path.display().to_string()),
            ),
            Err(e) => check(
                false,
                &fl!(
                    "doctor-config-broken",
                    path = cfg_path.display().to_string(),
                    error = e.clone()
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

    // 5. Синхронизация (фаза E): достижимость сервера и токен либо
    // доступность папки — по конфигу, без демона.
    doctor_sync(&sync_cfg, &journal_dir);

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

/// Секция синка в `ctl doctor` (фаза E): сервер — health + проверка токена
/// пустым push (ничего не пишет), папка — существование и права записи.
fn doctor_sync(sync: &driftling_core::SyncConfig, journal_dir: &std::path::Path) {
    use driftling_core::SyncMode;
    match sync.mode {
        SyncMode::Off => check(true, &fl!("doctor-sync-off")),
        SyncMode::Server => {
            for w in sync.warnings() {
                check(false, &w);
            }
            let address = sync.address.trim().trim_end_matches('/');
            if address.is_empty() {
                return;
            }
            match minreq::get(format!("{address}/v1/health"))
                .with_timeout(5)
                .send()
            {
                Ok(resp) if resp.status_code == 200 => {
                    check(true, &fl!("doctor-sync-server-ok", address = address));
                    if sync.token.trim().is_empty() {
                        return;
                    }
                    // Пустой push — валидная и «сухая» проверка токена:
                    // сервер ничего не сохраняет, отвечает accepted: 0.
                    let auth = minreq::post(format!("{address}/v1/push"))
                        .with_header("Authorization", format!("Bearer {}", sync.token.trim()))
                        .with_header("Content-Type", "application/json")
                        .with_timeout(5)
                        .with_body(r#"{"device":"doctor","events":[]}"#)
                        .send();
                    match auth {
                        Ok(resp) if resp.status_code == 200 => {
                            check(true, &fl!("doctor-sync-token-ok"))
                        }
                        Ok(resp) if resp.status_code == 401 => {
                            check(false, &fl!("doctor-sync-token-bad"))
                        }
                        Ok(resp) => check(
                            false,
                            &fl!(
                                "doctor-sync-server-odd",
                                status = i64::from(resp.status_code)
                            ),
                        ),
                        Err(e) => check(
                            false,
                            &fl!(
                                "doctor-sync-server-unreachable",
                                address = address,
                                error = e.to_string()
                            ),
                        ),
                    }
                }
                Ok(resp) => check(
                    false,
                    &fl!(
                        "doctor-sync-server-odd",
                        status = i64::from(resp.status_code)
                    ),
                ),
                Err(e) => check(
                    false,
                    &fl!(
                        "doctor-sync-server-unreachable",
                        address = address,
                        error = e.to_string()
                    ),
                ),
            }
        }
        SyncMode::Folder => {
            for w in sync.warnings() {
                check(true, &w); // рекомендация, не провал
            }
            if !journal_dir.is_dir() {
                check(
                    false,
                    &fl!(
                        "doctor-sync-folder-missing",
                        path = journal_dir.display().to_string()
                    ),
                );
                return;
            }
            let writable = rustix::fs::access(journal_dir, rustix::fs::Access::WRITE_OK).is_ok();
            let files = std::fs::read_dir(journal_dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| {
                            e.file_name()
                                .to_str()
                                .is_some_and(|n| n.starts_with("journal.") && n.ends_with(".jsonl"))
                        })
                        .count() as u64
                })
                .unwrap_or(0);
            check(
                writable,
                &fl!(
                    "doctor-sync-folder-ok",
                    path = journal_dir.display().to_string(),
                    files = files
                ),
            );
            if !writable {
                check(false, &fl!("doctor-sync-folder-readonly"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_hex_color;

    #[test]
    fn hex_color_parses_with_and_without_hash() {
        assert_eq!(parse_hex_color("e8944a"), Some(0xff_e8_94_4a));
        assert_eq!(parse_hex_color("#e8944a"), Some(0xff_e8_94_4a));
        assert_eq!(parse_hex_color("#E8944A"), Some(0xff_e8_94_4a));
        assert_eq!(parse_hex_color("  b0a294 "), Some(0xff_b0_a2_94));
        assert_eq!(parse_hex_color("000000"), Some(0xff_00_00_00));
    }

    #[test]
    fn hex_color_rejects_garbage() {
        for bad in [
            "", "#", "e8944", "e8944a0", "e8944a00", "zzzzzz", "##e8944a", "#e894 4a",
        ] {
            assert_eq!(parse_hex_color(bad), None, "{bad:?} не должен парситься");
        }
    }
}
