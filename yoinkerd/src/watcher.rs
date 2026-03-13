use crate::history::ClipboardHistory;
use arboard::Clipboard;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use yoinker_common::{Config, EntryContent};

pub async fn run(history: Arc<Mutex<ClipboardHistory>>, config: Config) {
    let interval = tokio::time::Duration::from_millis(config.poll_interval_ms);
    let max_bytes = config.max_entry_bytes;

    let history_clone = Arc::clone(&history);
    tokio::task::spawn_blocking(move || {
        let mut backoff_secs = 1u64;
        loop {
            let mut clipboard = match Clipboard::new() {
                Ok(c) => {
                    info!("clipboard watcher connected");
                    backoff_secs = 1;
                    c
                }
                Err(e) => {
                    error!("failed to connect to clipboard: {}, retrying in {}s", e, backoff_secs);
                    std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue;
                }
            };

            loop {
                std::thread::sleep(interval);

                match poll_clipboard(&mut clipboard, max_bytes) {
                    Some(content) => {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(async {
                            let mut history = history_clone.lock().await;
                            if history.add(content) {
                                info!("new clipboard entry (total: {})", history.entries.len());
                            }
                        });
                    }
                    None => {
                        // Check if clipboard is still accessible by trying to read
                        if clipboard.get_text().is_err() && clipboard.get_image().is_err() {
                            warn!("clipboard connection lost, reconnecting...");
                            break; // break inner loop to reconnect
                        }
                    }
                }
            }
        }
    })
    .await
    .ok();
}

fn poll_clipboard(clipboard: &mut Clipboard, max_bytes: usize) -> Option<EntryContent> {
    if let Ok(text) = clipboard.get_text() {
        if !text.is_empty() && (max_bytes == 0 || text.len() <= max_bytes) {
            return Some(EntryContent::Text { text });
        }
    }

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
