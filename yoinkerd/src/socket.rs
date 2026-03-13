use crate::history::ClipboardHistory;
use arboard::Clipboard;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tracing::{error, info};
use yoinker_common::{Config, EntryContent, Request, Response};

pub async fn run(history: Arc<Mutex<ClipboardHistory>>, config: Config) {
    let listener = match UnixListener::bind(&config.socket_path) {
        Ok(l) => l,
        Err(e) => {
            error!("failed to bind socket {:?}: {}", config.socket_path, e);
            return;
        }
    };

    info!("listening on {:?}", config.socket_path);

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("accept error: {}", e);
                continue;
            }
        };

        let history = Arc::clone(&history);
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();

            if reader.read_line(&mut line).await.is_err() {
                return;
            }

            let response = match serde_json::from_str::<Request>(line.trim()) {
                Ok(req) => handle_request(req, &history).await,
                Err(e) => Response::Error(format!("invalid request: {}", e)),
            };

            if let Ok(json) = serde_json::to_string(&response) {
                let _ = writer.write_all(json.as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
            }
        });
    }
}

async fn handle_request(
    req: Request,
    history: &Arc<Mutex<ClipboardHistory>>,
) -> Response {
    let mut history = history.lock().await;

    match req {
        Request::List => Response::Entries(history.entries.clone()),
        Request::Get { index } => {
            if let Some(entry) = history.entries.get(index) {
                Response::Entry(entry.clone())
            } else {
                Response::Error(format!("index {} out of range", index))
            }
        }
        Request::Pin { index } => {
            if history.pin(index) {
                Response::Ok
            } else {
                Response::Error(format!("index {} out of range", index))
            }
        }
        Request::Unpin { index } => {
            if history.unpin(index) {
                Response::Ok
            } else {
                Response::Error(format!("index {} out of range", index))
            }
        }
        Request::Clear => {
            history.clear_unpinned();
            Response::Ok
        }
        Request::Store { content, pin } => {
            let entry = EntryContent::Text { text: content };
            history.add(entry);
            if pin {
                history.pin(0);
            }
            Response::Ok
        }
        Request::Tag { index, tag } => {
            if history.set_tag(index, tag) {
                Response::Ok
            } else {
                Response::Error(format!("index {} out of range", index))
            }
        }
        Request::Delete { index } => {
            if history.delete(index) {
                Response::Ok
            } else {
                Response::Error(format!("index {} out of range", index))
            }
        }
        Request::Copy { index } => {
            if let Some(entry) = history.entries.get(index) {
                let content = entry.content.clone();
                // Drop history lock before blocking clipboard call
                drop(history);
                match set_clipboard(&content) {
                    Ok(()) => Response::Ok,
                    Err(e) => Response::Error(format!("clipboard error: {}", e)),
                }
            } else {
                Response::Error(format!("index {} out of range", index))
            }
        }
    }
}

fn set_clipboard(content: &EntryContent) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    match content {
        EntryContent::Text { text } => {
            clipboard.set_text(text).map_err(|e| e.to_string())
        }
        EntryContent::Image {
            width,
            height,
            bytes,
        } => {
            let img = arboard::ImageData {
                width: *width,
                height: *height,
                bytes: std::borrow::Cow::Borrowed(bytes),
            };
            clipboard.set_image(img).map_err(|e| e.to_string())
        }
    }
}
