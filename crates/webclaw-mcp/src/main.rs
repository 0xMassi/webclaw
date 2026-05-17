/// webclaw-mcp: MCP (Model Context Protocol) server for webclaw.
/// Exposes web extraction tools over stdio transport for AI agents
/// like Claude Desktop, Claude Code, and other MCP clients.
mod server;
mod tools;

use rmcp::ServiceExt;
use rmcp::transport::stdio;

use server::WebclawMcp;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if print_help_or_version() {
        return Ok(());
    }

    dotenvy::dotenv().ok();

    // Log to stderr -- stdout is the MCP transport channel
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let service = WebclawMcp::new().await.serve(stdio()).await?;

    service.waiting().await?;
    Ok(())
}

fn print_help_or_version() -> bool {
    let mut args = std::env::args().skip(1);
    let Some(arg) = args.next() else {
        return false;
    };

    match arg.as_str() {
        "-h" | "--help" => {
            println!("{}", help_text());
            true
        }
        "-V" | "--version" => {
            println!("webclaw-mcp {}", env!("CARGO_PKG_VERSION"));
            true
        }
        _ => false,
    }
}

fn help_text() -> String {
    format!(
        "\
webclaw-mcp {version}
MCP server for webclaw web extraction toolkit

Usage: webclaw-mcp

Options:
  -h, --help     Print help
  -V, --version  Print version

Tools:
  scrape, crawl, map, batch, extract, summarize, diff, brand, research, search,
  capture_network, discover_endpoints, show_endpoint, replay_endpoint,
  export_openapi, list_captures, list_extractors, vertical_scrape",
        version = env!("CARGO_PKG_VERSION")
    )
}
