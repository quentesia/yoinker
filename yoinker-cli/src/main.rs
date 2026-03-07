use clap::{Parser, Subcommand};
use yoinker_common::Config;

#[derive(Parser)]
#[command(name = "yoinker", about = "Terminal clipboard manager for Wayland")]
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
        /// Index of the clipboard entry
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
    let _config = Config::load();

    // TODO: Connect to daemon via Unix socket and send commands
    match cli.command {
        Commands::List => {
            println!("TODO: launch TUI");
        }
        Commands::Get { index } => {
            println!("TODO: get entry {index}");
        }
        Commands::Pin { index } => {
            println!("TODO: pin entry {index}");
        }
        Commands::Unpin { index } => {
            println!("TODO: unpin entry {index}");
        }
        Commands::Clear => {
            println!("TODO: clear history");
        }
        Commands::Store { content, pin } => {
            println!("TODO: store '{content}' (pin={pin})");
        }
    }
}
