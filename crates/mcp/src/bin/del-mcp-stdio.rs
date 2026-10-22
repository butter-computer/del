//! stdio entry point for agents that prefer to spawn their MCP servers.
//! Configured via `DEL_MCP_URL` and `DEL_MCP_TOKEN`; proxies to del's HTTP server.

fn main() {
    let (Ok(url), Ok(token)) = (
        std::env::var(del_mcp::launch::ENV_URL),
        std::env::var(del_mcp::launch::ENV_TOKEN),
    ) else {
        eprintln!("del-mcp-stdio: DEL_MCP_URL and DEL_MCP_TOKEN must be set");
        std::process::exit(2);
    };
    del_mcp::stdio::run(&url, &token, std::io::stdin().lock(), std::io::stdout());
}
