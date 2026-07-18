<p align="center">
  <a href="https://webclaw.io">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/0xMassi/webclaw/main/.github/banner.png" />
      <img src="https://raw.githubusercontent.com/0xMassi/webclaw/main/.github/banner.png" alt="webclaw" width="700" />
    </picture>
  </a>
</p>

<h3 align="center">
  One command to give your AI agent reliable web access.<br/>
  <sub>No headless browser. No Puppeteer. No 403s.</sub>
</h3>

<p align="center">
  <a href="https://www.npmjs.com/package/create-webclaw"><img src="https://img.shields.io/npm/dt/create-webclaw?style=for-the-badge&logo=npm&logoColor=white&label=Installs&color=CB3837" alt="npm installs" /></a>
  <a href="https://github.com/0xMassi/webclaw"><img src="https://img.shields.io/github/stars/0xMassi/webclaw?style=for-the-badge&logo=github&logoColor=white&label=Stars&color=181717" alt="Stars" /></a>
  <a href="https://github.com/0xMassi/webclaw/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-AGPL--3.0-10B981?style=for-the-badge" alt="License" /></a>
</p>

---

## Quick Start

```bash
npx create-webclaw
```

That's it. Auto-detects your AI tools, downloads the MCP server, configures everything.

Works with **Claude Desktop**, **Claude Code**, **Cursor**, **Windsurf**, **VS Code**, **OpenCode**, **Codex CLI**, and **Antigravity**.

---

## The Problem

Your AI agent calls `fetch()` and gets a 403. Cloudflare, Akamai, and every major CDN fingerprint the TLS handshake and block non-browser clients before the request hits the server.

When it does work, you get 100KB+ of raw HTML — navigation, ads, cookie banners, scripts. Your agent burns 4,000+ tokens parsing noise.

## The Fix

webclaw impersonates the latest Chrome at the TLS protocol level. Perfect JA4 fingerprint. Perfect HTTP/2 Akamai hash. 99% bypass rate on 102 tested sites.

Then it extracts just the content — clean markdown, 67% fewer tokens.

```
                     Raw HTML                          webclaw
┌──────────────────────────────────┐    ┌──────────────────────────────────┐
│ <div class="ad-wrapper">         │    │ # Breaking: AI Breakthrough      │
│ <nav class="global-nav">         │    │                                  │
│ <script>window.__NEXT_DATA__     │    │ Researchers achieved 94%         │
│ ={...8KB of JSON...}</script>    │    │ accuracy on cross-domain         │
│ <div class="social-share">       │    │ reasoning benchmarks.            │
│ <!-- 142,847 characters -->      │    │                                  │
│                                  │    │ ## Key Findings                  │
│         4,820 tokens             │    │         1,590 tokens             │
└──────────────────────────────────┘    └──────────────────────────────────┘
```

---

## What It Does

```bash
npx create-webclaw
```

1. Detects installed AI tools (Claude, Cursor, Windsurf, VS Code, OpenCode, Codex, Antigravity)
2. Downloads the `webclaw-mcp` binary for your platform (macOS arm64/x86, Linux x86/arm64)
3. Asks for your API key (optional — **works locally without one**)
4. Writes the MCP config for each detected tool

## MCP Tools

After setup, your AI agent has access to these 14 tools:

| Tool | What it does | API key needed? |
|------|-------------|-----------------|
| **scrape** | Extract content from any URL | No |
| **crawl** | Recursively crawl a website | No |
| **search** | Web search + parallel scrape | Yes (Serper) |
| **map** | Discover URLs from sitemaps | No |
| **batch** | Extract multiple URLs in parallel | No |
| **extract** | LLM-powered structured extraction | Yes |
| **summarize** | Content summarization | Yes |
| **diff** | Track content changes | No |
| **brand** | Extract brand identity | No |
| **research** | Deep multi-page research | Yes |
| **list_extractors** | List the built-in vertical extractors | No |
| **vertical_scrape** | Scrape with a built-in vertical extractor | No |
| **lead** | Enrich a company URL into an outreach-ready lead (leadership + socials, tech, pricing, public emails) | Yes (webclaw) |
| **lead_batch** | Enrich up to 25 company URLs at once | Yes (webclaw) |

**8 of the 14 tools work fully offline** — no API key, no cloud, no tracking.

## Supported Tools

| Tool | Config location |
|------|----------------|
| Claude Desktop | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Claude Code | `~/.claude.json` |
| Cursor | `.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| VS Code (Continue) | `~/.continue/config.json` |
| OpenCode | `~/.config/opencode/opencode.json` (or `./opencode.json`) |
| Codex CLI | `~/.codex/config.toml` |
| Antigravity | `~/.config/antigravity/mcp.json` (or `~/.antigravity/mcp.json`) |

## Sites That Work

webclaw gets through where default `fetch()` gets blocked:

Nike, Cloudflare, Bloomberg, Zillow, Indeed, Viagogo, Fansale, Wikipedia, Stripe, and 93 more. Tested on 102 sites with **99% success rate**.

## Alternative Install Methods

### Homebrew

```bash
brew tap 0xMassi/webclaw && brew install webclaw
```

### Docker

```bash
docker run --rm ghcr.io/0xmassi/webclaw https://example.com
```

### Cargo

```bash
cargo install --git https://github.com/0xMassi/webclaw.git webclaw-cli
```

### Prebuilt Binaries

Download from [GitHub Releases](https://github.com/0xMassi/webclaw/releases) for macOS (arm64, x86_64) and Linux (x86_64, aarch64).

---

## Links

- [Website](https://webclaw.io)
- [Documentation](https://webclaw.io/docs)
- [GitHub](https://github.com/0xMassi/webclaw)
- [Discord](https://discord.gg/KDfd48EpnW)
- [Status](https://status.webclaw.io)

## License

AGPL-3.0
