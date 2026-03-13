mod ipc;
mod tui;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::path::PathBuf;
use std::process::Stdio;
use yoinker_common::{Config, EntryContent, Request, Response};

#[derive(Parser)]
#[command(
    name = "yoinker",
    about = "Terminal clipboard manager for Linux",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show clipboard history in a TUI with fuzzy search
    List {
        /// Output as JSON instead of launching TUI
        #[arg(long)]
        json: bool,
    },
    /// Print nth clipboard item to stdout
    Get {
        /// Index of the clipboard entry (0 = most recent)
        index: usize,
    },
    /// Pin a clipboard entry so it never expires
    Pin {
        /// Index of the clipboard entry
        index: usize,
    },
    /// Unpin a clipboard entry
    Unpin {
        /// Index of the clipboard entry
        index: usize,
    },
    /// Clear all unpinned clipboard history
    Clear,
    /// Store text directly (for Neovim integration)
    Store {
        /// Text to store
        content: String,
        /// Pin this entry
        #[arg(long)]
        pin: bool,
    },
    /// Manage the yoinker daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon in the background
    Start,
    /// Stop a running daemon
    Stop,
    /// Check if the daemon is running
    Status,
}

fn find_yoinkerd() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("yoinkerd");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("yoinkerd")
}

fn pid_path(config: &Config) -> PathBuf {
    config.socket_path.with_extension("pid")
}

fn read_pid(config: &Config) -> Option<u32> {
    std::fs::read_to_string(pid_path(config))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn daemon_start(config: &Config) -> Result<(), String> {
    // Check if already running
    if let Some(pid) = read_pid(config) {
        if process_alive(pid) {
            return Err(format!("daemon already running (PID {})", pid));
        }
    }

    let yoinkerd = find_yoinkerd();

    // Create log file next to history
    let log_path = config
        .history_path
        .parent()
        .map(|p| p.join("yoinkerd.log"))
        .unwrap_or_else(|| PathBuf::from("/tmp/yoinkerd.log"));

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| format!("cannot create log file {:?}: {}", log_path, e))?;

    let child = std::process::Command::new(&yoinkerd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .spawn()
        .map_err(|e| format!("cannot start {:?}: {}", yoinkerd, e))?;

    eprintln!("daemon started (PID {})", child.id());
    eprintln!("log: {:?}", log_path);
    Ok(())
}

fn daemon_stop(config: &Config) -> Result<(), String> {
    let pid = read_pid(config).ok_or("no PID file found (daemon not running?)")?;

    if !process_alive(pid) {
        // Clean up stale PID file
        std::fs::remove_file(pid_path(config)).ok();
        return Err(format!("daemon not running (stale PID {})", pid));
    }

    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    eprintln!("sent SIGTERM to daemon (PID {})", pid);
    Ok(())
}

async fn daemon_status(config: &Config) {
    match read_pid(config) {
        Some(pid) if process_alive(pid) => {
            eprintln!("daemon running (PID {})", pid);
            match ipc::send(config, Request::List).await {
                Ok(Response::Entries(entries)) => {
                    eprintln!("socket: connected ({} entries)", entries.len());
                }
                Ok(_) => eprintln!("socket: connected"),
                Err(e) => eprintln!("socket: {}", e),
            }
        }
        Some(pid) => {
            eprintln!("daemon not running (stale PID {})", pid);
            std::fs::remove_file(pid_path(config)).ok();
        }
        None => eprintln!("daemon not running (no PID file)"),
    }
}

/// Try to send a request; if connection fails, auto-start daemon and retry.
async fn send_with_autostart(config: &Config, request: Request) -> Result<Response, String> {
    match ipc::send(config, request.clone()).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            // Try auto-starting the daemon
            eprintln!("daemon not running, starting...");
            if let Err(start_err) = daemon_start(config) {
                return Err(format!(
                    "{}\nFailed to auto-start: {}\nHint: Start the daemon with: yoinker daemon start",
                    e, start_err
                ));
            }
            // Wait for daemon to be ready
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Ok(resp) = ipc::send(config, request.clone()).await {
                    return Ok(resp);
                }
            }
            Err(format!(
                "{}\nHint: Start the daemon with: yoinker daemon start",
                e
            ))
        }
    }
}

fn print_connection_error(e: &str) {
    eprintln!("connection error: {}", e);
    eprintln!("Hint: Start the daemon with: yoinker daemon start");
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = Config::load();

    match cli.command {
        Commands::List { json } => {
            let resp = send_with_autostart(&config, Request::List).await;
            match resp {
                Ok(Response::Entries(entries)) => {
                    if json {
                        let json_str = serde_json::to_string_pretty(&entries)
                            .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
                        println!("{}", json_str);
                        return;
                    }
                    if entries.is_empty() {
                        eprintln!("clipboard history is empty");
                        return;
                    }
                    match tui::run(entries, &config).await {
                        Ok(Some(tui::TuiAction::Select(index))) => {
                            match ipc::send(&config, Request::Copy { index }).await {
                                Ok(Response::Ok) => {}
                                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                                Err(e) => print_connection_error(&e),
                                _ => eprintln!("unexpected response"),
                            }
                        }
                        Ok(None) => {}
                        Err(e) => eprintln!("TUI error: {}", e),
                    }
                }
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => print_connection_error(&e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Get { index } => {
            match send_with_autostart(&config, Request::Get { index }).await {
                Ok(Response::Entry(entry)) => match &entry.content {
                    EntryContent::Text { text } => print!("{}", text),
                    EntryContent::Image { .. } => {
                        eprintln!("[image data - use 'list' to select]")
                    }
                },
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => print_connection_error(&e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Pin { index } => {
            match send_with_autostart(&config, Request::Pin { index }).await {
                Ok(Response::Ok) => eprintln!("pinned entry {}", index),
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => print_connection_error(&e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Unpin { index } => {
            match send_with_autostart(&config, Request::Unpin { index }).await {
                Ok(Response::Ok) => eprintln!("unpinned entry {}", index),
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => print_connection_error(&e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Clear => match send_with_autostart(&config, Request::Clear).await {
            Ok(Response::Ok) => eprintln!("cleared unpinned history"),
            Ok(Response::Error(e)) => eprintln!("error: {}", e),
            Err(e) => print_connection_error(&e),
            _ => eprintln!("unexpected response"),
        },
        Commands::Store { content, pin } => {
            match send_with_autostart(&config, Request::Store { content, pin }).await {
                Ok(Response::Ok) => eprintln!("stored"),
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => print_connection_error(&e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Daemon { action } => match action {
            DaemonAction::Start => {
                if let Err(e) = daemon_start(&config) {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
            DaemonAction::Stop => {
                if let Err(e) = daemon_stop(&config) {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
            DaemonAction::Status => daemon_status(&config).await,
        },
        Commands::Completions { shell } => {
            generate(
                shell,
                &mut Cli::command(),
                "yoinker",
                &mut std::io::stdout(),
            );
        }
    }
}
