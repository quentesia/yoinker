mod history;
mod socket;
mod watcher;

use history::ClipboardHistory;
use std::sync::Arc;
use tokio::sync::Mutex;
use yoinker_common::Config;

#[tokio::main]
async fn main() {
    let config = Config::load();

    // Ensure data directory exists
    if let Some(parent) = config.history_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    eprintln!("yoinkerd starting...");
    eprintln!("  history: {:?}", config.history_path);
    eprintln!("  socket:  {:?}", config.socket_path);
    eprintln!("  max history: {}", config.max_history);

    let history = ClipboardHistory::load(&config);
    eprintln!("  loaded {} entries", history.entries.len());

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

    tokio::select! {
        _ = watcher_handle => eprintln!("clipboard watcher exited"),
        _ = socket_handle => eprintln!("socket listener exited"),
        _ = tokio::signal::ctrl_c() => eprintln!("\nshutting down..."),
    }

    // Save on exit
    let history = history.lock().await;
    history.save();
    eprintln!("history saved ({} entries)", history.entries.len());
}
