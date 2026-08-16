/// webclaw-mcp: MCP (Model Context Protocol) server for webclaw.
/// Exposes web extraction tools over stdio transport for AI agents
/// like Claude Desktop, Claude Code, and other MCP clients.
mod handshake_guard;
mod server;
mod tools;

use rmcp::ServiceExt;
use rmcp::service::RoleServer;
use rmcp::transport::async_rw::AsyncRwTransport;

use handshake_guard::PreHandshakeGuard;
use server::WebclawMcp;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    // Log to stderr -- stdout is the MCP transport channel
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    // Built explicitly rather than via `stdio()` so the handshake guard can wrap
    // a concrete transport; see handshake_guard for why it is needed.
    let transport = PreHandshakeGuard::new(AsyncRwTransport::<RoleServer, _, _>::new_server(
        tokio::io::stdin(),
        tokio::io::stdout(),
    ));

    let service = WebclawMcp::new().await.serve(transport).await?;

    service.waiting().await?;
    Ok(())
}
