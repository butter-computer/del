//! stdio -> streamable-HTTP proxy used by `del-mcp-stdio`.
//!
//! Reads newline-delimited JSON-RPC from `input`, POSTs each message to the
//! server with the bearer token, and writes each response as one line to
//! `output`. Requests run on their own threads so a blocking `ask_user` does
//! not stall `ping` or other calls.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

/// Responses carry browser results of up to 6 MiB before JSON escaping, which
/// can double their size; ureq's default 10 MB limit would fail them.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
/// In-flight requests; reading pauses (backpressure) once this many are open.
const MAX_WORKERS: usize = 64;

fn emit(output: &Mutex<impl Write>, line: &str) {
    let mut o = output.lock().unwrap();
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
}

fn forward(
    agent: &ureq::Agent,
    url: &str,
    token: &str,
    line: &str,
) -> Result<Option<String>, String> {
    let resp = agent
        .post(url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .send(line)
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp
        .into_body()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    match status {
        202 => Ok(None),
        200 => Ok(Some(body.trim().to_string())),
        s => Err(format!("del returned HTTP {s}: {}", body.trim())),
    }
}

pub fn run<R, W>(url: &str, token: &str, input: R, output: W)
where
    R: BufRead,
    W: Write + Send + 'static,
{
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(None)
        .build()
        .into();
    let output = Arc::new(Mutex::new(output));
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        workers.retain(|w| !w.is_finished());
        if workers.len() >= MAX_WORKERS {
            let _ = workers.remove(0).join();
        }
        let (agent, output) = (agent.clone(), output.clone());
        let (url, token) = (url.to_string(), token.to_string());
        workers.push(std::thread::spawn(move || {
            match forward(&agent, &url, &token, &line) {
                Ok(Some(resp)) => emit(&output, &resp),
                Ok(None) => {}
                Err(e) => {
                    // Only requests (with an id) can be answered with an error.
                    if let Some(id) = serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|v| v.get("id").cloned())
                    {
                        let err = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": e}});
                        emit(&output, &err.to_string());
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.join();
    }
}
