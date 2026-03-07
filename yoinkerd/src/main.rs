use yoinker_common::Config;

#[tokio::main]
async fn main() {
    let config = Config::load();

    println!("yoinkerd starting...");
    println!("  history: {:?}", config.history_path);
    println!("  socket:  {:?}", config.socket_path);
    println!("  max history: {}", config.max_history);

    // TODO: Load persisted history
    // TODO: Start clipboard watcher (wl-clipboard-rs)
    // TODO: Listen on Unix socket for client commands
}
