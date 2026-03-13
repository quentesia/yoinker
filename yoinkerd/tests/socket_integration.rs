use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use yoinker_common::{test_config, ClipboardEntry, EntryContent, Request, Response};

// We need access to yoinkerd internals, so we replicate the socket handler setup.
// The socket module isn't exposed as a library, so we test via the protocol directly.

async fn send_request(socket_path: &std::path::Path, req: &Request) -> Response {
    let stream = UnixStream::connect(socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();

    let json = serde_json::to_string(req).unwrap();
    writer.write_all(json.as_bytes()).await.unwrap();
    writer.write_all(b"\n").await.unwrap();
    writer.shutdown().await.unwrap();

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

/// Minimal socket server for testing (mirrors yoinkerd socket::run without clipboard ops)
async fn start_test_server(
    socket_path: std::path::PathBuf,
    entries: Arc<Mutex<Vec<ClipboardEntry>>>,
) {
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();

    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let entries = Arc::clone(&entries);

        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            if reader.read_line(&mut line).await.is_err() {
                return;
            }

            let req: Request = match serde_json::from_str(line.trim()) {
                Ok(r) => r,
                Err(e) => {
                    let resp = Response::Error(format!("parse error: {}", e));
                    let json = serde_json::to_string(&resp).unwrap();
                    let _ = writer.write_all(json.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                    return;
                }
            };

            let response = {
                let mut entries = entries.lock().await;
                match req {
                    Request::List => Response::Entries(entries.clone()),
                    Request::Get { index } => {
                        if let Some(entry) = entries.get(index) {
                            Response::Entry(entry.clone())
                        } else {
                            Response::Error(format!("index {} out of range", index))
                        }
                    }
                    Request::Pin { index } => {
                        if let Some(entry) = entries.get_mut(index) {
                            entry.pinned = true;
                            Response::Ok
                        } else {
                            Response::Error(format!("index {} out of range", index))
                        }
                    }
                    Request::Unpin { index } => {
                        if let Some(entry) = entries.get_mut(index) {
                            entry.pinned = false;
                            Response::Ok
                        } else {
                            Response::Error(format!("index {} out of range", index))
                        }
                    }
                    Request::Clear => {
                        entries.retain(|e| e.pinned);
                        Response::Ok
                    }
                    Request::Store { content, pin } => {
                        let id = entries.len() as u64 + 1;
                        entries.insert(
                            0,
                            ClipboardEntry {
                                id,
                                content: EntryContent::Text { text: content },
                                timestamp: 0,
                                pinned: pin,
                                tag: None,
                            },
                        );
                        Response::Ok
                    }
                    Request::Tag { index, tag } => {
                        if let Some(entry) = entries.get_mut(index) {
                            entry.tag = tag;
                            Response::Ok
                        } else {
                            Response::Error(format!("index {} out of range", index))
                        }
                    }
                    Request::Delete { index } => {
                        if index < entries.len() {
                            entries.remove(index);
                            Response::Ok
                        } else {
                            Response::Error(format!("index {} out of range", index))
                        }
                    }
                    Request::Copy { index } => {
                        if index < entries.len() {
                            // In tests we can't actually set the clipboard, just ack
                            Response::Ok
                        } else {
                            Response::Error(format!("index {} out of range", index))
                        }
                    }
                }
            };

            let json = serde_json::to_string(&response).unwrap();
            let _ = writer.write_all(json.as_bytes()).await;
            let _ = writer.write_all(b"\n").await;
        });
    }
}

#[tokio::test]
async fn ipc_store_and_list() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Store entries
    let resp = send_request(
        &socket_path,
        &Request::Store {
            content: "first".into(),
            pin: false,
        },
    )
    .await;
    assert_eq!(resp, Response::Ok);

    let resp = send_request(
        &socket_path,
        &Request::Store {
            content: "second".into(),
            pin: false,
        },
    )
    .await;
    assert_eq!(resp, Response::Ok);

    // List
    let resp = send_request(&socket_path, &Request::List).await;
    match resp {
        Response::Entries(e) => {
            assert_eq!(e.len(), 2);
            match &e[0].content {
                EntryContent::Text { text } => assert_eq!(text, "second"),
                _ => panic!("expected text"),
            }
        }
        _ => panic!("expected Entries"),
    }
}

#[tokio::test]
async fn ipc_get_valid_index() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    send_request(
        &socket_path,
        &Request::Store {
            content: "item0".into(),
            pin: false,
        },
    )
    .await;

    let resp = send_request(&socket_path, &Request::Get { index: 0 }).await;
    match resp {
        Response::Entry(e) => match &e.content {
            EntryContent::Text { text } => assert_eq!(text, "item0"),
            _ => panic!("expected text"),
        },
        _ => panic!("expected Entry"),
    }
}

#[tokio::test]
async fn ipc_get_out_of_range() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_request(&socket_path, &Request::Get { index: 99 }).await;
    match resp {
        Response::Error(msg) => assert!(msg.contains("out of range")),
        _ => panic!("expected Error"),
    }
}

#[tokio::test]
async fn ipc_pin_and_clear() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    send_request(
        &socket_path,
        &Request::Store {
            content: "keep".into(),
            pin: false,
        },
    )
    .await;
    send_request(
        &socket_path,
        &Request::Store {
            content: "delete".into(),
            pin: false,
        },
    )
    .await;

    // Pin first entry (index 0 = "delete" since it's newest)
    send_request(&socket_path, &Request::Pin { index: 0 }).await;

    // Clear unpinned
    send_request(&socket_path, &Request::Clear).await;

    let resp = send_request(&socket_path, &Request::List).await;
    match resp {
        Response::Entries(e) => {
            assert_eq!(e.len(), 1);
            assert!(e[0].pinned);
            match &e[0].content {
                EntryContent::Text { text } => assert_eq!(text, "delete"), // "delete" was pinned
                _ => panic!("expected text"),
            }
        }
        _ => panic!("expected Entries"),
    }
}

#[tokio::test]
async fn ipc_unpin() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    send_request(
        &socket_path,
        &Request::Store {
            content: "x".into(),
            pin: true,
        },
    )
    .await;

    // Verify pinned
    let resp = send_request(&socket_path, &Request::List).await;
    match &resp {
        Response::Entries(e) => assert!(e[0].pinned),
        _ => panic!("expected Entries"),
    }

    // Unpin
    send_request(&socket_path, &Request::Unpin { index: 0 }).await;

    let resp = send_request(&socket_path, &Request::List).await;
    match resp {
        Response::Entries(e) => assert!(!e[0].pinned),
        _ => panic!("expected Entries"),
    }
}

#[tokio::test]
async fn ipc_copy_valid_index() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    send_request(
        &socket_path,
        &Request::Store {
            content: "copy me".into(),
            pin: false,
        },
    )
    .await;

    let resp = send_request(&socket_path, &Request::Copy { index: 0 }).await;
    assert_eq!(resp, Response::Ok);
}

#[tokio::test]
async fn ipc_copy_out_of_range() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_request(&socket_path, &Request::Copy { index: 0 }).await;
    match resp {
        Response::Error(msg) => assert!(msg.contains("out of range")),
        _ => panic!("expected Error"),
    }
}

#[tokio::test]
async fn ipc_store_with_pin() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    send_request(
        &socket_path,
        &Request::Store {
            content: "pinned!".into(),
            pin: true,
        },
    )
    .await;

    let resp = send_request(&socket_path, &Request::List).await;
    match resp {
        Response::Entries(e) => {
            assert_eq!(e.len(), 1);
            assert!(e[0].pinned);
        }
        _ => panic!("expected Entries"),
    }
}

#[tokio::test]
async fn ipc_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Send raw garbage
    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"not json at all\n").await.unwrap();
    writer.shutdown().await.unwrap();

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let resp: Response = serde_json::from_str(line.trim()).unwrap();
    match resp {
        Response::Error(msg) => assert!(msg.contains("parse error")),
        _ => panic!("expected Error for invalid JSON"),
    }
}

#[tokio::test]
async fn ipc_empty_list() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_request(&socket_path, &Request::List).await;
    match resp {
        Response::Entries(e) => assert!(e.is_empty()),
        _ => panic!("expected empty Entries"),
    }
}

#[tokio::test]
async fn ipc_concurrent_requests() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Fire 10 store requests concurrently
    let mut handles = vec![];
    for i in 0..10 {
        let sp = socket_path.clone();
        handles.push(tokio::spawn(async move {
            send_request(
                &sp,
                &Request::Store {
                    content: format!("concurrent-{}", i),
                    pin: false,
                },
            )
            .await
        }));
    }

    for h in handles {
        let resp = h.await.unwrap();
        assert_eq!(resp, Response::Ok);
    }

    let resp = send_request(&socket_path, &Request::List).await;
    match resp {
        Response::Entries(e) => assert_eq!(e.len(), 10),
        _ => panic!("expected Entries"),
    }
}

#[tokio::test]
async fn ipc_pin_out_of_range() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_request(&socket_path, &Request::Pin { index: 0 }).await;
    match resp {
        Response::Error(msg) => assert!(msg.contains("out of range")),
        _ => panic!("expected Error"),
    }
}

#[tokio::test]
async fn ipc_clear_empty_history() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let resp = send_request(&socket_path, &Request::Clear).await;
    assert_eq!(resp, Response::Ok);
}

#[tokio::test]
async fn ipc_unicode_content() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let unicode = "日本語テスト 🎉 emojis → ← ñ";
    send_request(
        &socket_path,
        &Request::Store {
            content: unicode.into(),
            pin: false,
        },
    )
    .await;

    let resp = send_request(&socket_path, &Request::Get { index: 0 }).await;
    match resp {
        Response::Entry(e) => match &e.content {
            EntryContent::Text { text } => assert_eq!(text, unicode),
            _ => panic!("expected text"),
        },
        _ => panic!("expected Entry"),
    }
}

#[tokio::test]
async fn ipc_multiline_content() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let multiline = "line1\nline2\nline3\n\ttabbed";
    send_request(
        &socket_path,
        &Request::Store {
            content: multiline.into(),
            pin: false,
        },
    )
    .await;

    let resp = send_request(&socket_path, &Request::Get { index: 0 }).await;
    match resp {
        Response::Entry(e) => match &e.content {
            EntryContent::Text { text } => assert_eq!(text, multiline),
            _ => panic!("expected text"),
        },
        _ => panic!("expected Entry"),
    }
}

#[tokio::test]
async fn ipc_large_content() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(tmp.path());
    let entries = Arc::new(Mutex::new(Vec::new()));

    let socket_path = config.socket_path.clone();
    tokio::spawn(start_test_server(socket_path.clone(), Arc::clone(&entries)));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let large = "x".repeat(100_000);
    send_request(
        &socket_path,
        &Request::Store {
            content: large.clone(),
            pin: false,
        },
    )
    .await;

    let resp = send_request(&socket_path, &Request::Get { index: 0 }).await;
    match resp {
        Response::Entry(e) => match &e.content {
            EntryContent::Text { text } => assert_eq!(text.len(), 100_000),
            _ => panic!("expected text"),
        },
        _ => panic!("expected Entry"),
    }
}
