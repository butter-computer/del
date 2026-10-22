//! Transcript replay: plays a recorded `>` (client->server) / `<` (server->client)
//! conversation against the real client code over an in-memory pipe.

use del_codex::{CodexClient, Incoming};
use serde_json::Value;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub type Replay = JoinHandle<Result<(), String>>;

pub fn fixture(name: &str) -> Vec<String> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Methods whose params depend on the caller's config, so are not compared.
const LOOSE: [&str; 4] = [
    "initialize",
    "thread/start",
    "thread/resume",
    "account/login/start",
];

pub fn replay(lines: Vec<String>) -> (CodexClient, mpsc::UnboundedReceiver<Incoming>, Replay) {
    replay_with(lines, Default::default())
}

pub fn replay_with(
    lines: Vec<String>,
    backoff: del_codex::rpc::BackoffPolicy,
) -> (CodexClient, mpsc::UnboundedReceiver<Incoming>, Replay) {
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (cr, cw) = tokio::io::split(client_io);
    let (sr, mut sw) = tokio::io::split(server_io);
    let (client, rx) = CodexClient::from_io_with(cr, cw, backoff);
    let task = tokio::spawn(async move {
        let mut reader = BufReader::new(sr).lines();
        // recorded client request id -> actual id the client used
        let mut ids: HashMap<String, Value> = HashMap::new();
        for (n, line) in lines.iter().enumerate() {
            let (dir, body) = line.split_at(2);
            let mut msg: Value =
                serde_json::from_str(body).map_err(|e| format!("line {n}: {e}"))?;
            match dir {
                "< " => {
                    let is_response = msg.get("method").is_none() && msg.get("id").is_some();
                    if is_response {
                        let key = msg["id"].to_string();
                        msg["id"] = ids
                            .get(&key)
                            .cloned()
                            .ok_or(format!("line {n}: response to unseen id {key}"))?;
                    }
                    sw.write_all(msg.to_string().as_bytes())
                        .await
                        .map_err(|e| e.to_string())?;
                    sw.write_all(b"\n").await.map_err(|e| e.to_string())?;
                    sw.flush().await.map_err(|e| e.to_string())?;
                }
                "> " => {
                    let got = reader
                        .next_line()
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or(format!("line {n}: client closed, expected {body}"))?;
                    let got: Value = serde_json::from_str(&got).map_err(|e| e.to_string())?;
                    if msg.get("method").is_some() {
                        if got["method"] != msg["method"] {
                            return Err(format!("line {n}: expected {} got {got}", msg["method"]));
                        }
                        let m = msg["method"].as_str().unwrap();
                        if !LOOSE.contains(&m) && got.get("params") != msg.get("params") {
                            return Err(format!(
                                "line {n}: params differ\n want {}\n got  {}",
                                msg["params"], got["params"]
                            ));
                        }
                        if let Some(id) = msg.get("id") {
                            ids.insert(id.to_string(), got["id"].clone());
                        }
                    } else if got["result"] != msg["result"] || got["id"] != msg["id"] {
                        return Err(format!(
                            "line {n}: response differs\n want {msg}\n got  {got}"
                        ));
                    }
                }
                other => return Err(format!("line {n}: bad direction {other:?}")),
            }
        }
        // Keep the pipe open until the client goes away.
        while let Ok(Some(_)) = reader.next_line().await {}
        Ok(())
    });
    (client, rx, task)
}
