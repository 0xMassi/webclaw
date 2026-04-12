/// noxa-mcp library wrapper.
///
/// This exposes the MCP server so it can be embedded by the `noxa` CLI via
/// `noxa mcp` without duplicating the transport/bootstrap code.
///
/// Callers must initialize tracing before calling `run()`. Stdout must remain
/// untouched after `run()` begins because it carries the MCP wire protocol.
pub(crate) mod cloud;
pub(crate) mod server;
pub(crate) mod tools;

use rmcp::ServiceExt;
use rmcp::transport::stdio;

/// Start the MCP server over stdio and block until the client disconnects.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let service = server::NoxaMcp::new().await.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
