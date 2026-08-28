//! driftling-server — синк-сервер Driftling (фаза E, ТЗ §3.5).
//!
//! Один статический бинарь + SQLite: push/pull журнала событий и lease
//! присутствия («питомец один», newest-wins). Плоский HTTP; TLS — задача
//! reverse-proxy (Caddy/nginx), см. dist/server/README.md.
//!
//! Это ЕДИНСТВЕННЫЙ крейт workspace с tokio: отдельный бинарь, с демоном
//! и настройками не линкуется, фич zbus/ksni не трогает (см. Cargo.toml).
//!
//! Вывод CLI — операторский (администрирование своего сервера), поэтому
//! без Fluent-локализации демона; язык сообщений — английский, как у логов.

mod app;
mod config;
mod db;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};

use crate::app::AppState;
use crate::config::Config;
use crate::db::Db;

#[derive(Parser)]
#[command(
    name = "driftling-server",
    version,
    about = "Driftling sync server (journal push/pull + presence lease)"
)]
struct Cli {
    /// Путь к TOML-конфигу (listen, db).
    #[arg(long, global = true, default_value = "driftling-server.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Запустить HTTP-сервер.
    Serve,
    /// Управление аккаунтами (токены — Bearer для клиентов).
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Создать аккаунт и напечатать его токен (показывается ОДИН раз).
    Add { name: String },
    /// Список аккаунтов.
    List,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    let db = Db::open(&cfg.db)?;
    match cli.command {
        Command::Serve => serve(cfg, db),
        Command::Account { command } => account(command, &db),
    }
}

fn account(command: AccountCommand, db: &Db) -> anyhow::Result<()> {
    match command {
        AccountCommand::Add { name } => {
            let (id, token) = db.account_add(&name)?;
            println!("account #{id} \"{name}\" created");
            println!("token (shown ONCE, only its hash is stored):");
            println!("{token}");
        }
        AccountCommand::List => {
            let list = db.account_list()?;
            if list.is_empty() {
                println!("no accounts yet; create one: driftling-server account add <name>");
            }
            for (id, name, events, tokens) in list {
                println!("#{id}\t{name}\tevents: {events}\ttokens: {tokens}");
            }
        }
    }
    Ok(())
}

/// Tokio-рантайм поднимается только для serve: account-команды остаются
/// синхронными и мгновенными.
fn serve(cfg: Config, db: Db) -> anyhow::Result<()> {
    let state = Arc::new(AppState { db });
    let router = app::router(state);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?
        .block_on(async {
            let listener = tokio::net::TcpListener::bind(&cfg.listen)
                .await
                .with_context(|| format!("не удалось слушать {}", cfg.listen))?;
            log::info!(
                "driftling-server {} слушает http://{} (база: {})",
                env!("CARGO_PKG_VERSION"),
                cfg.listen,
                cfg.db.display()
            );
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal())
                .await
                .context("http server")
        })
}

/// SIGINT/SIGTERM → мягкое завершение (докер и systemd шлют SIGTERM).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    log::info!("получен сигнал завершения, останавливаюсь");
}
