mod ipc;
mod tui;

use clap::{Parser, Subcommand};
use yoinker_common::{Config, EntryContent, Request, Response};

#[derive(Parser)]
#[command(name = "yoinker", about = "Terminal clipboard manager for Linux")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show clipboard history in a TUI with fuzzy search
    List,
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
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = Config::load();

    match cli.command {
        Commands::List => {
            let resp = ipc::send(&config, Request::List).await;
            match resp {
                Ok(Response::Entries(entries)) => {
                    if entries.is_empty() {
                        eprintln!("clipboard history is empty");
                        return;
                    }
                    match tui::run(entries, &config).await {
                        Ok(Some(index)) => {
                            // Tell the daemon to copy it to clipboard (daemon holds it alive)
                            match ipc::send(&config, Request::Copy { index }).await {
                                Ok(Response::Ok) => {}
                                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                                Err(e) => eprintln!("connection error: {}", e),
                                _ => eprintln!("unexpected response"),
                            }
                        }
                        Ok(None) => {}
                        Err(e) => eprintln!("TUI error: {}", e),
                    }
                }
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => eprintln!("connection error: {}", e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Get { index } => {
            match ipc::send(&config, Request::Get { index }).await {
                Ok(Response::Entry(entry)) => match &entry.content {
                    EntryContent::Text { text } => print!("{}", text),
                    EntryContent::Image { .. } => {
                        eprintln!("[image data - use 'list' to select]")
                    }
                },
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => eprintln!("connection error: {}", e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Pin { index } => {
            match ipc::send(&config, Request::Pin { index }).await {
                Ok(Response::Ok) => eprintln!("pinned entry {}", index),
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => eprintln!("connection error: {}", e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Unpin { index } => {
            match ipc::send(&config, Request::Unpin { index }).await {
                Ok(Response::Ok) => eprintln!("unpinned entry {}", index),
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => eprintln!("connection error: {}", e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Clear => {
            match ipc::send(&config, Request::Clear).await {
                Ok(Response::Ok) => eprintln!("cleared unpinned history"),
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => eprintln!("connection error: {}", e),
                _ => eprintln!("unexpected response"),
            }
        }
        Commands::Store { content, pin } => {
            match ipc::send(&config, Request::Store { content, pin }).await {
                Ok(Response::Ok) => eprintln!("stored"),
                Ok(Response::Error(e)) => eprintln!("error: {}", e),
                Err(e) => eprintln!("connection error: {}", e),
                _ => eprintln!("unexpected response"),
            }
        }
    }
}
