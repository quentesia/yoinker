mod history;
mod socket;
mod watcher;

use history::ClipboardHistory;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};
use yoinker_common::Config;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    // Handle --version
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("yoinkerd {}", VERSION);
        return;
    }

    tracing_subscriber::fmt()
        .with_target(false)
        .with_timer(tracing_subscriber::fmt::time::uptime())
        .init();

    let config = Config::load();

    if let Some(parent) = config.history_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Write PID file
    let pid_path = config.socket_path.with_extension("pid");
    let pid = std::process::id();
    if let Err(e) = std::fs::write(&pid_path, pid.to_string()) {
        error!("failed to write PID file {:?}: {}", pid_path, e);
    }

    info!("yoinkerd {} starting (PID {})", VERSION, pid);
    info!("history: {:?}", config.history_path);
    info!("socket: {:?}", config.socket_path);
    info!("max history: {}", config.max_history);

    let history = ClipboardHistory::load(&config);
    info!("loaded {} entries", history.entries.len());

    let history = Arc::new(Mutex::new(history));

    // Clean up stale socket
    if config.socket_path.exists() {
        std::fs::remove_file(&config.socket_path).ok();
    }

    let watcher_history = Arc::clone(&history);
    let watcher_config = config.clone();
    let watcher_handle = tokio::spawn(async move {
        watcher::run(watcher_history, watcher_config).await;
    });

    let socket_history = Arc::clone(&history);
    let socket_config = config.clone();
    let socket_handle = tokio::spawn(async move {
        socket::run(socket_history, socket_config).await;
    });

    // Periodic save (every 5 seconds)
    let save_history = Arc::clone(&history);
    let save_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            save_history.lock().await.save_if_dirty();
        }
    });

    tokio::select! {
        _ = watcher_handle => error!("clipboard watcher exited"),
        _ = socket_handle => error!("socket listener exited"),
        _ = save_handle => error!("save task exited"),
        _ = tokio::signal::ctrl_c() => info!("shutting down..."),
    }

    // Save on exit
    let mut history = history.lock().await;
    history.save();
    info!("history saved ({} entries)", history.entries.len());

    // Clean up PID file
    std::fs::remove_file(&pid_path).ok();
}
