use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

// --- Configuration ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub max_history: usize,
    pub history_path: PathBuf,
    pub socket_path: PathBuf,
    /// Polling interval in milliseconds for clipboard changes
    pub poll_interval_ms: u64,
    /// Maximum size in bytes for a single clipboard entry (0 = unlimited)
    pub max_entry_bytes: usize,
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
            poll_interval_ms: 500,
            max_entry_bytes: 10 * 1024 * 1024, // 10 MB
        }
    }
}

impl Config {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipboardEntry {
    pub id: u64,
    pub content: EntryContent,
    pub timestamp: u64,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum EntryContent {
    Text { text: String },
    Image { width: usize, height: usize, bytes: Vec<u8> },
}

impl EntryContent {
    pub fn preview(&self, max_len: usize) -> String {
        match self {
            EntryContent::Text { text } => {
                let s = text.replace('\n', "\\n");
                if s.len() <= max_len {
                    s
                } else {
                    format!("{}...", &s[..max_len])
                }
            }
            EntryContent::Image { width, height, bytes } => {
                format!("[image: {}x{}, {} bytes]", width, height, bytes.len())
            }
        }
    }

    pub fn content_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        match self {
            EntryContent::Text { text } => {
                0u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            EntryContent::Image { bytes, .. } => {
                1u8.hash(&mut hasher);
                bytes.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    pub fn byte_len(&self) -> usize {
        match self {
            EntryContent::Text { text } => text.len(),
            EntryContent::Image { bytes, .. } => bytes.len(),
        }
    }
}

// --- IPC protocol (newline-delimited JSON over Unix socket) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    List,
    Get { index: usize },
    Pin { index: usize },
    Unpin { index: usize },
    Clear,
    Store { content: String, pin: bool },
    /// Copy entry at index to the system clipboard (daemon holds it alive)
    Copy { index: usize },
    /// Delete entry at index
    Delete { index: usize },
    /// Set or clear a tag on an entry (None to remove tag)
    Tag { index: usize, tag: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Response {
    Entries(Vec<ClipboardEntry>),
    Entry(ClipboardEntry),
    Ok,
    Error(String),
}

/// Create a test config pointing at a temp directory. Useful for tests.
pub fn test_config(tmp_dir: &std::path::Path) -> Config {
    Config {
        max_history: 5,
        history_path: tmp_dir.join("history.json"),
        socket_path: tmp_dir.join("yoinker-test.sock"),
        poll_interval_ms: 100,
        max_entry_bytes: 1024,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- EntryContent tests ---

    #[test]
    fn text_preview_short() {
        let c = EntryContent::Text { text: "hello".into() };
        assert_eq!(c.preview(10), "hello");
    }

    #[test]
    fn text_preview_truncated() {
        let c = EntryContent::Text { text: "hello world this is long".into() };
        let p = c.preview(10);
        assert!(p.ends_with("..."));
        assert!(p.len() <= 13); // 10 + "..."
    }

    #[test]
    fn text_preview_newlines_escaped() {
        let c = EntryContent::Text { text: "line1\nline2\nline3".into() };
        let p = c.preview(100);
        assert!(!p.contains('\n'));
        assert!(p.contains("\\n"));
    }

    #[test]
    fn image_preview() {
        let c = EntryContent::Image { width: 100, height: 200, bytes: vec![0; 5000] };
        let p = c.preview(100);
        assert!(p.contains("100x200"));
        assert!(p.contains("5000"));
    }

    #[test]
    fn text_content_hash_deterministic() {
        let a = EntryContent::Text { text: "same".into() };
        let b = EntryContent::Text { text: "same".into() };
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn text_content_hash_differs() {
        let a = EntryContent::Text { text: "hello".into() };
        let b = EntryContent::Text { text: "world".into() };
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn text_vs_image_hash_differs() {
        let t = EntryContent::Text { text: "".into() };
        let i = EntryContent::Image { width: 0, height: 0, bytes: vec![] };
        assert_ne!(t.content_hash(), i.content_hash());
    }

    #[test]
    fn image_content_hash_deterministic() {
        let a = EntryContent::Image { width: 10, height: 10, bytes: vec![1, 2, 3] };
        let b = EntryContent::Image { width: 10, height: 10, bytes: vec![1, 2, 3] };
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn image_content_hash_differs_on_bytes() {
        let a = EntryContent::Image { width: 10, height: 10, bytes: vec![1, 2, 3] };
        let b = EntryContent::Image { width: 10, height: 10, bytes: vec![4, 5, 6] };
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn byte_len_text() {
        let c = EntryContent::Text { text: "hello".into() };
        assert_eq!(c.byte_len(), 5);
    }

    #[test]
    fn byte_len_image() {
        let c = EntryContent::Image { width: 1, height: 1, bytes: vec![0; 100] };
        assert_eq!(c.byte_len(), 100);
    }

    // --- Serialization round-trip tests ---

    #[test]
    fn text_entry_serialization_roundtrip() {
        let entry = ClipboardEntry {
            id: 42,
            content: EntryContent::Text { text: "hello world".into() },
            timestamp: 1234567890,
            pinned: true,
            tag: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ClipboardEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 42);
        assert_eq!(back.timestamp, 1234567890);
        assert!(back.pinned);
        match back.content {
            EntryContent::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn image_entry_serialization_roundtrip() {
        let entry = ClipboardEntry {
            id: 7,
            content: EntryContent::Image { width: 64, height: 32, bytes: vec![255, 0, 128] },
            timestamp: 999,
            pinned: false,
            tag: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ClipboardEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 7);
        assert!(!back.pinned);
        match back.content {
            EntryContent::Image { width, height, bytes } => {
                assert_eq!(width, 64);
                assert_eq!(height, 32);
                assert_eq!(bytes, vec![255, 0, 128]);
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn request_serialization_roundtrip() {
        let requests = vec![
            Request::List,
            Request::Get { index: 3 },
            Request::Pin { index: 0 },
            Request::Unpin { index: 1 },
            Request::Clear,
            Request::Store { content: "test".into(), pin: true },
            Request::Copy { index: 5 },
            Request::Delete { index: 2 },
            Request::Tag { index: 0, tag: Some("email".into()) },
            Request::Tag { index: 1, tag: None },
        ];
        for req in requests {
            let json = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&json).unwrap();
            // Verify round-trip by re-serializing
            assert_eq!(json, serde_json::to_string(&back).unwrap());
        }
    }

    #[test]
    fn response_serialization_roundtrip() {
        let responses = vec![
            Response::Ok,
            Response::Error("bad".into()),
            Response::Entry(ClipboardEntry {
                id: 1,
                content: EntryContent::Text { text: "x".into() },
                timestamp: 0,
                pinned: false,
                tag: None,
            }),
            Response::Entries(vec![]),
        ];
        for resp in responses {
            let json = serde_json::to_string(&resp).unwrap();
            let back: Response = serde_json::from_str(&json).unwrap();
            assert_eq!(resp, back);
        }
    }

    // --- Config tests ---

    #[test]
    fn config_default_values() {
        let config = Config::default();
        assert_eq!(config.max_history, 50);
        assert_eq!(config.poll_interval_ms, 500);
        assert_eq!(config.max_entry_bytes, 10 * 1024 * 1024);
        assert!(config.history_path.to_str().unwrap().contains("yoinker"));
        assert!(config.socket_path.to_str().unwrap().contains("yoinker"));
    }

    #[test]
    fn config_load_missing_file_returns_default() {
        // Config::load() with no file should return defaults without panicking
        let config = Config::load();
        assert_eq!(config.max_history, 50);
    }

    #[test]
    fn config_toml_deserialization() {
        let toml_str = r#"
            max_history = 100
            history_path = "/tmp/test-history.json"
            socket_path = "/tmp/test.sock"
            poll_interval_ms = 250
            max_entry_bytes = 5000
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.max_history, 100);
        assert_eq!(config.poll_interval_ms, 250);
        assert_eq!(config.max_entry_bytes, 5000);
    }

    #[test]
    fn test_config_helper() {
        let tmp = std::path::Path::new("/tmp/yoinker-unit-test");
        let config = test_config(tmp);
        assert_eq!(config.max_history, 5);
        assert_eq!(config.socket_path, tmp.join("yoinker-test.sock"));
    }

    // --- Special content tests ---

    #[test]
    fn unicode_text_roundtrip() {
        let entry = ClipboardEntry {
            id: 1,
            content: EntryContent::Text { text: "日本語 emoji: 🎉🚀 → ← ñ ü".into() },
            timestamp: 0,
            pinned: false,
            tag: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ClipboardEntry = serde_json::from_str(&json).unwrap();
        match back.content {
            EntryContent::Text { text } => assert_eq!(text, "日本語 emoji: 🎉🚀 → ← ñ ü"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn empty_text_hash_and_preview() {
        let c = EntryContent::Text { text: "".into() };
        assert_eq!(c.preview(10), "");
        assert_eq!(c.byte_len(), 0);
        // hash should still be deterministic
        let c2 = EntryContent::Text { text: "".into() };
        assert_eq!(c.content_hash(), c2.content_hash());
    }

    #[test]
    fn large_text_preview() {
        let text = "a".repeat(10000);
        let c = EntryContent::Text { text };
        let p = c.preview(50);
        assert_eq!(p.len(), 53); // 50 + "..."
    }

    #[test]
    fn multiline_text_preview() {
        let c = EntryContent::Text { text: "fn main() {\n    println!(\"hello\");\n}".into() };
        let p = c.preview(100);
        assert!(p.contains("\\n"));
        assert!(p.contains("fn main()"));
    }
}
