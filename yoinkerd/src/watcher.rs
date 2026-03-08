use crate::history::ClipboardHistory;
use arboard::Clipboard;
use std::sync::Arc;
use tokio::sync::Mutex;
use yoinker_common::{Config, EntryContent};

pub async fn run(history: Arc<Mutex<ClipboardHistory>>, config: Config) {
    let interval = tokio::time::Duration::from_millis(config.poll_interval_ms);
    let max_bytes = config.max_entry_bytes;

    // Clipboard must be used from a single thread (X11 requirement)
    let history_clone = Arc::clone(&history);
    tokio::task::spawn_blocking(move || {
        let mut clipboard = match Clipboard::new() {
            Ok(c) => {
                eprintln!("yoinkerd: clipboard watcher connected");
                c
            }
            Err(e) => {
                eprintln!("yoinkerd: failed to connect to clipboard: {}", e);
                return;
            }
        };

        loop {
            std::thread::sleep(interval);

            let content = poll_clipboard(&mut clipboard, max_bytes);
            if let Some(content) = content {
                // Block on the async lock from sync context
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    let mut history = history_clone.lock().await;
                    if history.add(content) {
                        eprintln!(
                            "yoinkerd: new clipboard entry (total: {})",
                            history.entries.len()
                        );
                    }
                });
            }
        }
    })
    .await
    .ok();
}

fn poll_clipboard(clipboard: &mut Clipboard, max_bytes: usize) -> Option<EntryContent> {
    // Try text first
    if let Ok(text) = clipboard.get_text() {
        if !text.is_empty() && (max_bytes == 0 || text.len() <= max_bytes) {
            return Some(EntryContent::Text { text });
        }
    }

    // Try image
    if let Ok(img) = clipboard.get_image() {
        let byte_len = img.bytes.len();
        if byte_len > 0 && (max_bytes == 0 || byte_len <= max_bytes) {
            return Some(EntryContent::Image {
                width: img.width,
                height: img.height,
                bytes: img.bytes.into_owned(),
            });
        }
    }

    None
}
