# MCP Web Scraping

Use webclaw as a local MCP server so Claude Code, Claude Desktop, Cursor, Windsurf, OpenCode, Codex CLI, or another MCP client can fetch clean web context.

## Install

```bash
npx create-webclaw
```

The installer detects supported MCP clients and can write the config for you.

## Manual Config

```json
{
  "mcpServers": {
    "webclaw": {
      "command": "~/.webclaw/webclaw-mcp",
      "env": {
        "WEBCLAW_API_KEY": "wc_your_key"
      }
    }
  }
}
```

`WEBCLAW_API_KEY` is optional for local extraction. Add it when you want cloud fallback for protected sites, JS rendering, hosted search, or hosted research.

## Example Prompts

```text
Scrape https://docs.rs/tokio and summarize the parts about task spawning.
```

```text
Crawl https://docs.example.com up to depth 2 and return the pages most relevant to authentication.
```

```text
Extract the pricing tiers from https://example.com/pricing as JSON with fields name, price, limits, and features.
```

The MCP server exposes tools for scrape, crawl, map, batch, extract, summarize, diff, brand, research, search, and vertical extractors.
