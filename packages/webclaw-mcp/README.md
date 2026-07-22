# @webclaw/mcp

**Clean web access for AI agents, over MCP.** Turn any URL into markdown, JSON, or LLM-ready context — including pages that block bots or need JavaScript — straight from Claude, Cursor, and any MCP client.

Zero-install launcher for the [webclaw](https://webclaw.io) MCP server. `npx @webclaw/mcp` downloads a prebuilt `webclaw-mcp` binary once (verified against the release `SHA256SUMS`), caches it, and runs it as an MCP stdio server. No Rust build, no global install. It runs on your machine; the hosted webclaw cloud is used only for the tools that need it, and only when you set `WEBCLAW_API_KEY`.

## Add to your MCP client

Point any stdio MCP client at the npx command — Claude Desktop, Cursor, Windsurf, Antigravity (`mcpServers` JSON):

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

Add a key to unlock the cloud-backed tools (bot-protection bypass, JS rendering, web search, research, lead enrichment):

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

Or run `npx create-webclaw` to auto-detect your AI tools and write their configs for you.

## Tools (14)

scrape, search, crawl, map, batch, extract, summarize, diff, brand, research, lead, lead_batch, plus `list_extractors` / `vertical_scrape` for 30+ site-specific extractors (Amazon, GitHub, Reddit, YouTube, npm, PyPI, and more).

- **No key needed:** scrape, crawl, map, batch, diff, brand, list_extractors, vertical_scrape.
- **Needs an LLM** (local Ollama or a provider key): extract, summarize.
- **Needs `WEBCLAW_API_KEY`:** search, research, lead, lead_batch — plus automatic bot-protection bypass and JS rendering for the fetch tools.

Get a key at [webclaw.io](https://webclaw.io).

## Environment

| Variable | Purpose |
|---|---|
| `WEBCLAW_API_KEY` | Enables cloud-backed tools (optional). |
| `WEBCLAW_MCP_BIN` | Absolute path to a `webclaw-mcp` binary; skips the download. |
| `WEBCLAW_MCP_VERSION` | Release tag to install (default: the pinned release). |
| `WEBCLAW_MCP_CACHE` | Cache directory (default: `~/.cache/webclaw`). |

## Links

- **Docs:** [webclaw.io/docs/mcp](https://webclaw.io/docs/mcp)
- **Source** (CLI, REST API, SDKs, extraction engine): [github.com/0xMassi/webclaw](https://github.com/0xMassi/webclaw)
- **Hosted API & keys:** [webclaw.io](https://webclaw.io)

## License

AGPL-3.0-only
