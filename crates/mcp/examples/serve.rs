//! Run the server over a mock host and write launch configs for real CLIs.
//! `cargo run -p del-mcp --example serve -- <dir>` prints how to launch each
//! agent; Ctrl-C to stop. Answers `ask_user` with the first option after 1s.

use del_agent::{Provider, SessionId};
use del_mcp::{
    launch::{self, Transport},
    *,
};
use std::sync::Arc;

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn shell_args(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: serve <dir>"));
    std::fs::create_dir_all(&dir)?;
    let host = Arc::new(MockHost::new());
    host.add_session("demo", Provider::Claude);
    let server = McpServer::start(
        host.clone(),
        ServerOptions {
            trace: true,
            ..Default::default()
        },
    )
    .await?;
    let c = server.register_session(&SessionId::new("demo"));
    let stdio = Transport::Stdio {
        binary: std::env::current_exe()?.with_file_name("../del-mcp-stdio"),
    };
    let cl = launch::claude(&c, &Transport::Http, &dir)?;
    println!("claude http  : claude {}", shell_args(&cl.args));
    let dir2 = dir.join("stdio");
    std::fs::create_dir_all(&dir2)?;
    let cls = launch::claude(&c, &stdio, &dir2)?;
    println!("claude stdio : claude {}", shell_args(&cls.args));
    println!(
        "codex http   : codex {}",
        shell_args(&launch::codex(&c, &Transport::Http).args)
    );
    println!(
        "codex stdio  : codex {}",
        shell_args(&launch::codex(&c, &stdio).args)
    );
    // Token goes to a 0600 file, not stdout.
    let env = dir.join("env.sh");
    let body = format!(
        "export DEL_MCP_URL='{}'\nexport DEL_MCP_TOKEN='{}'\n",
        c.url, c.token
    );
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&env)?.write_all(body.as_bytes())?;
    println!(
        "env (token)  : source {}",
        shell_quote(&env.to_string_lossy())
    );
    loop {
        let q = host.take_question().await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let text = q.options.first().cloned().unwrap_or_else(|| "ok".into());
        q.answer(UserAnswer::Text { text });
    }
}
