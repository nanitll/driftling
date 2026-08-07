//! driftling: без аргументов — демон (питомец на экране);
//! `driftling ctl <cmd>` — управление запущенным демоном.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod daemon;

#[derive(Parser)]
#[command(
    name = "driftling",
    version,
    about = "Desktop tamagotchi that drifts between your devices"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Управление запущенным демоном.
    Ctl {
        #[command(subcommand)]
        action: CtlAction,
    },
    /// Открыть окно настроек (запускает driftling-settings).
    Settings,
}

#[derive(Subcommand)]
enum CtlAction {
    /// Позвать питомца на экран.
    Summon,
    /// Убрать питомца с экрана (демон продолжает работать).
    Dismiss,
    /// Показать состояние.
    Status,
    /// Остановить демон.
    Quit,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
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
            let status = std::process::Command::new(&program)
                .status()
                .map_err(|e| anyhow::anyhow!("не удалось запустить {program:?}: {e}"))?;
            anyhow::ensure!(status.success(), "driftling-settings завершился с ошибкой");
            Ok(())
        }
        Some(Command::Ctl { action }) => {
            let req = match action {
                CtlAction::Summon => driftling_ipc::Request::Summon,
                CtlAction::Dismiss => driftling_ipc::Request::Dismiss,
                CtlAction::Status => driftling_ipc::Request::Status,
                CtlAction::Quit => driftling_ipc::Request::Quit,
            };
            match driftling_ipc::call(&req)? {
                driftling_ipc::Response::Ok => println!("ok"),
                driftling_ipc::Response::Status {
                    pets,
                    state,
                    uptime_secs,
                } => {
                    println!("питомцев: {pets}, состояние: {state}, аптайм: {uptime_secs}s");
                }
                driftling_ipc::Response::Error(e) => anyhow::bail!(e),
            }
            Ok(())
        }
    }
}
