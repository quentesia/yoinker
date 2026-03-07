use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// --- Configuration ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Maximum number of clipboard entries to keep (excluding pinned)
    pub max_history: usize,
    /// Path to persist history
    pub history_path: PathBuf,
    /// Socket path for daemon communication
    pub socket_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("yoinker");

        let runtime_dir = dirs::runtime_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("yoinker.sock");

        Self {
            max_history: 50,
            history_path: data_dir.join("history.json"),
            socket_path: runtime_dir,
        }
    }
}

impl Config {
    /// Load config from ~/.config/yoinker/config.toml, falling back to defaults.
    pub fn load() -> Self {
        let config_path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("yoinker")
            .join("config.toml");

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path).unwrap_or_default();
            toml::from_str(&contents).unwrap_or_default()
        } else {
            Self::default()
        }
    }
}

// --- Clipboard entry ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub id: u64,
    pub content: EntryContent,
    pub timestamp: u64,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EntryContent {
    Text(String),
    Image(Vec<u8>),
}

impl EntryContent {
    pub fn preview(&self, max_len: usize) -> String {
        match self {
            EntryContent::Text(s) => {
                if s.len() <= max_len {
                    s.clone()
                } else {
                    format!("{}...", &s[..max_len])
                }
            }
            EntryContent::Image(bytes) => {
                format!("[image: {} bytes]", bytes.len())
            }
        }
    }
}

// --- IPC protocol ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    List,
    Get { index: usize },
    Pin { index: usize },
    Unpin { index: usize },
    Clear,
    /// Store content directly (for Neovim integration)
    Store { content: String, pin: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Entries(Vec<ClipboardEntry>),
    Entry(ClipboardEntry),
    Ok,
    Error(String),
}
