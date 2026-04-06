mod bot;
mod config;
mod format;
mod notify;
mod qb;
mod server;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "qbitbot", version, about = "qBittorrent Telegram bot")]
struct Cli {
    /// Path to config.json
    #[arg(long, short, default_value = "config.json")]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Send a notification to the running bot (called by qBittorrent hooks)
    Notify {
        /// The notification message (qBittorrent expands format tokens before calling)
        message: String,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("qbitbot=info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::load(&cli.config)?;

    // Use an 8 MB stack to prevent stack overflow from dptree's deeply nested
    // handler type tree in the teloxide dispatcher.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()?
        .block_on(async move {
            match cli.command {
                None => {
                    tracing::info!("Starting qbittorrent telegram bot");
                    tracing::info!("Allowed user IDs: {:?}", cfg.bot_allowed_users);
                    bot::run(cfg).await
                }
                Some(Commands::Notify { message }) => notify::run(cfg, message).await,
            }
        })
}
