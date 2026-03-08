use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use yoinker_common::{Config, Request, Response};

pub async fn send(config: &Config, request: Request) -> Result<Response, String> {
    let stream = UnixStream::connect(&config.socket_path)
        .await
        .map_err(|e| format!("cannot connect to yoinkerd at {:?}: {} (is the daemon running?)", config.socket_path, e))?;

    let (reader, mut writer) = stream.into_split();

    let json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|e| e.to_string())?;
    writer.shutdown().await.map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;

    serde_json::from_str(line.trim()).map_err(|e| format!("invalid response: {}", e))
}
