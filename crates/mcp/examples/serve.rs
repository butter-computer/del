//! Run the server over a mock host and write launch configs for real CLIs.
//! `cargo run -p del-mcp --example serve -- <dir>` prints how to launch each
//! agent; Ctrl-C to stop. Answers `ask_user` with the first option after 1s.

use del_mcp::{launch::{self, Transport}, *};
use del_agent::{Provider, SessionId};
use std::sync::Arc;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: serve <dir>"));
    std::fs::create_dir_all(&dir)?;
    let host = Arc::new(MockHost::new());
    host.add_session("demo", Provider::Claude);
    let server = McpServer::start(host.clone(), ServerOptions { trace: true, ..Default::default() }).await?;
    let c = server.register_session(&SessionId::new("demo"));
    let stdio = Transport::Stdio { binary: std::env::current_exe()?.with_file_name("del-mcp-stdio") };
    let cl = launch::claude(&c, &Transport::Http, &dir)?;
    println!("claude http  : claude {}", cl.args.join(" "));
    let dir2 = dir.join("stdio");
    std::fs::create_dir_all(&dir2)?;
    let cls = launch::claude(&c, &stdio, &dir2)?;
    println!("claude stdio : claude {}", cls.args.join(" "));
    println!("codex http   : codex {}", launch::codex(&c, &Transport::Http).args.join(" "));
    println!("codex stdio  : codex {}", launch::codex(&c, &stdio).args.join(" "));
    // Token goes to a 0600 file, not stdout.
    let env = dir.join("env.sh");
    let body = format!("export DEL_MCP_URL='{}'\nexport DEL_MCP_TOKEN='{}'\n", c.url, c.token);
    std::fs::write(&env, body)?;
    println!("env (token)  : source {}", env.display());
    loop {
        let q = host.take_question().await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let text = q.options.first().cloned().unwrap_or_else(|| "ok".into());
        q.answer(UserAnswer::Text { text });
    }
}
