# @webclaw/mcp

Zero-install launcher for the [webclaw](https://webclaw.io) MCP server — web extraction and anti-bot web access for AI agents, over the Model Context Protocol.

`npx @webclaw/mcp` downloads a prebuilt `webclaw-mcp` binary once (from the pinned GitHub release), caches it, and runs it as an MCP stdio server. No Rust build, no global install. It runs on your machine; the hosted webclaw cloud is only used for tools that need it, and only when you set `WEBCLAW_API_KEY`.

## Use in an MCP client

Point any stdio MCP client at the npx command. Claude Desktop / Cursor / Windsurf / Antigravity (`mcpServers` JSON):

```json
{
  "mcpServers": {
    "webclaw": {
      "command": "npx",
      "args": ["-y", "@webclaw/mcp"]
    }
  }
}
```

Add a key for cloud-backed tools (anti-bot bypass, JS rendering, search, research):

```json
{
  "mcpServers": {
    "webclaw": {
      "command": "npx",
      "args": ["-y", "@webclaw/mcp"],
      "env": { "WEBCLAW_API_KEY": "wc_your_key" }
    }
  }
}
```

Claude Code:

```bash
claude mcp add webclaw -- npx -y @webclaw/mcp
```

## Tools

scrape, search, crawl, map, batch, extract, summarize, diff, brand, research, lead, lead_batch, plus `list_extractors` / `vertical_scrape` for 30+ site-specific extractors. Local tools run with no API key; cloud tools require `WEBCLAW_API_KEY`.

## Environment

| Variable | Purpose |
|---|---|
| `WEBCLAW_API_KEY` | Enables cloud-backed tools (optional). |
| `WEBCLAW_MCP_BIN` | Absolute path to a `webclaw-mcp` binary; skips download. |
| `WEBCLAW_MCP_VERSION` | Release tag to install (default: the pinned release). |
| `WEBCLAW_MCP_CACHE` | Cache directory (default: `~/.cache/webclaw`). |

The downloaded archive is verified against the release `SHA256SUMS` before use.

## Alternatives

- `npx create-webclaw` — interactive installer that auto-detects your AI tools and writes their configs for you.
- Prebuilt binaries, Homebrew, Docker, and `cargo install` — see the [main README](https://github.com/0xMassi/webclaw).

## License

AGPL-3.0-only
