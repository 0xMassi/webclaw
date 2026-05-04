<p align="center">
  <a href="https://webclaw.io">
    <img src=".github/banner.png" alt="webclaw" width="700" />
  </a>
</p>

<h1 align="center">webclaw</h1>

<p align="center">
  <strong>The fastest web scraper for AI agents.</strong><br/>
  <sub>67% fewer tokens. Sub-millisecond extraction. Zero browser overhead.</sub>
</p>

<p align="center">
  <a href="https://github.com/0xMassi/webclaw/stargazers"><img src="https://shieldcn.dev/github/stars/0xMassi/webclaw.svg?variant=branded" alt="Stars" /></a>
  <a href="https://github.com/0xMassi/webclaw/releases"><img src="https://shieldcn.dev/github/tag/0xMassi/webclaw.svg?variant=branded&logo=rust" alt="Version" /></a>
  <a href="https://github.com/0xMassi/webclaw/blob/main/LICENSE"><img src="https://shieldcn.dev/github/license/0xMassi/webclaw.svg?variant=branded" alt="License" /></a>
  <a href="https://www.npmjs.com/package/create-webclaw"><img src="https://shieldcn.dev/npm/dt/create-webclaw.svg?variant=branded" alt="npm installs" /></a>
</p>
<p align="center">
  <a href="https://discord.gg/KDfd48EpnW"><img src="https://shieldcn.dev/badge/Discord-Join.svg?variant=branded&logo=discord" alt="Discord" /></a>
  <a href="https://x.com/webclaw_io"><img src="https://shieldcn.dev/badge/Follow-@webclaw__io.svg?variant=branded&logo=x" alt="X / Twitter" /></a>
  <a href="https://webclaw.io"><img src="https://shieldcn.dev/badge/Website-webclaw.io.svg?variant=branded&logo=safari" alt="Website" /></a>
  <a href="https://webclaw.io/docs"><img src="https://shieldcn.dev/badge/Docs-Read.svg?variant=branded&logo=readthedocs" alt="Docs" /></a>
</p>

---

<p align="center">
  <img src="assets/demo.gif" alt="Claude Code: web_fetch gets 403, webclaw extracts successfully" width="700" />
  <br/>
  <sub>Claude Code's built-in web_fetch → 403 Forbidden. webclaw → clean markdown.</sub>
</p>

---

Your AI agent calls `fetch()` and gets a 403. Or 142KB of raw HTML that burns through your token budget. **webclaw fixes both.**

It extracts clean, structured content from any URL using Chrome-level TLS fingerprinting — no headless browser, no Selenium, no Puppeteer. Output is optimized for LLMs: **67% fewer tokens** than raw HTML, with metadata, links, and images preserved.

```
                     Raw HTML                          webclaw
┌──────────────────────────────────┐    ┌──────────────────────────────────┐
│ <div class="ad-wrapper">         │    │ # Breaking: AI Breakthrough      │
│ <nav class="global-nav">         │    │                                  │
│ <script>window.__NEXT_DATA__     │    │ Researchers achieved 94%         │
│ ={...8KB of JSON...}</script>    │    │ accuracy on cross-domain         │
│ <div class="social-share">       │    │ reasoning benchmarks.            │
│ <button>Tweet</button>           │    │                                  │
│ <footer class="site-footer">     │    │ ## Key Findings                  │
│ <!-- 142,847 characters -->      │    │ - 3x faster inference            │
│                                  │    │ - Open-source weights            │
│         4,820 tokens             │    │         1,590 tokens             │
└──────────────────────────────────┘    └──────────────────────────────────┘
```

---

## Two ways to use webclaw

**Self-host.** Free, AGPL-3.0, runs locally. Get the CLI, MCP server, or REST API in one command. Ships with the 8 core extraction tools: scrape, crawl, map, batch, extract, summarize, diff, brand.

**Hosted API** at **[webclaw.io](https://webclaw.io)**. 500 pages/month free, no card. Adds what self-hosting can't do alone: antibot bypass (Cloudflare, DataDome, WAF), JS rendering, async crawl/research jobs, web search, watches. For when you want it to *just work*.

---

## Get Started (30 seconds)

### For AI agents (Claude, Cursor, Windsurf, VS Code)

```bash
npx create-webclaw
```

Auto-detects your AI tools, downloads the MCP server, and configures everything. One command.

### Homebrew (macOS/Linux)

```bash
brew tap 0xMassi/webclaw
brew install webclaw
```

### Prebuilt binaries

Download from [GitHub Releases](https://github.com/0xMassi/webclaw/releases) for macOS (arm64, x86_64) and Linux (x86_64, aarch64).

### Cargo (from source)

```bash
cargo install --git https://github.com/0xMassi/webclaw.git webclaw-cli
cargo install --git https://github.com/0xMassi/webclaw.git webclaw-mcp
```

webclaw uses BoringSSL (via `boring-sys2`) for TLS fingerprinting, which
needs a few system packages at build time. If `cargo install` panics with
a `boring-sys2` build error or `Unable to find libclang`, install the
prerequisites first:

| OS | Install command |
|---|---|
| Debian / Ubuntu | `sudo apt install -y pkg-config libssl-dev cmake clang git build-essential` |
| Fedora / RHEL | `sudo dnf install -y pkg-config openssl-devel cmake clang git make gcc` |
| Arch | `sudo pacman -S pkg-config openssl cmake clang git base-devel` |
| macOS | `xcode-select --install` (and Xcode CLT covers everything) |

If you do not want to manage build dependencies yourself, prefer the
**Homebrew**, **Docker**, or **Prebuilt binaries** options above.

### Docker

```bash
docker run --rm ghcr.io/0xmassi/webclaw https://example.com
```

### Docker Compose (with Ollama for LLM features)

```bash
cp env.example .env
docker compose up -d
```

---

## Why webclaw?

| | webclaw | Firecrawl | Trafilatura | Readability |
|---|:---:|:---:|:---:|:---:|
| **Extraction accuracy** | **95.1%** | — | 80.6% | 83.5% |
| **Token efficiency** | **-67%** | — | -55% | -51% |
| **Speed (100KB page)** | **3.2ms** | ~500ms | 18.4ms | 8.7ms |
| **TLS fingerprinting** | Yes | No | No | No |
| **Self-hosted** | Yes | No | Yes | Yes |
| **MCP (Claude/Cursor)** | Yes | No | No | No |
| **No browser required** | Yes | No | Yes | Yes |
| **Cost** | Free | $$$$ | Free | Free |

**Choose webclaw if** you want fast local extraction, LLM-optimized output, and native AI agent integration.

---

## What it looks like

```bash
$ webclaw https://stripe.com -f llm

> URL: https://stripe.com
> Title: Stripe | Financial Infrastructure for the Internet
> Language: en
> Word count: 847

# Stripe | Financial Infrastructure for the Internet

Stripe is a suite of APIs powering online payment processing
and commerce solutions for internet businesses of all sizes.

## Products
- Payments — Accept payments online and in person
- Billing — Manage subscriptions and invoicing
- Connect — Build a marketplace or platform
...
```

```bash
$ webclaw https://github.com --brand

{
  "name": "GitHub",
  "colors": [{"hex": "#59636E", "usage": "Primary"}, ...],
  "fonts": ["Mona Sans", "ui-monospace"],
  "logos": [{"url": "https://github.githubassets.com/...", "kind": "svg"}]
}
```

```bash
$ webclaw https://docs.rust-lang.org --crawl --depth 2 --max-pages 50

Crawling... 50/50 pages extracted
---
# Page 1: https://docs.rust-lang.org/
...
# Page 2: https://docs.rust-lang.org/book/
...
```

---

## MCP Server — 10 tools for AI agents

<a href="https://glama.ai/mcp/servers/0xMassi/webclaw"><img src="https://glama.ai/mcp/servers/0xMassi/webclaw/badge" alt="webclaw MCP server" /></a>

webclaw ships as an MCP server that plugs into Claude Desktop, Claude Code, Cursor, Windsurf, OpenCode, Antigravity, Codex CLI, and any MCP-compatible client.

```bash
npx create-webclaw    # auto-detects and configures everything
```

Or manual setup — add to your Claude Desktop config:

```json
{
  "mcpServers": {
    "webclaw": {
      "command": "~/.webclaw/webclaw-mcp"
    }
  }
}
```

Then in Claude: *"Scrape the top 5 results for 'web scraping tools' and compare their pricing"* — it just works.

### Available tools

| Tool | Description | Requires API key? |
|------|-------------|:-:|
| `scrape` | Extract content from any URL | No |
| `crawl` | Recursive site crawl | No |
| `map` | Discover URLs from sitemaps | No |
| `batch` | Parallel multi-URL extraction | No |
| `extract` | LLM-powered structured extraction | No (needs Ollama) |
| `summarize` | Page summarization | No (needs Ollama) |
| `diff` | Content change detection | No |
| `brand` | Brand identity extraction | No |
| `search` | Web search + scrape results | Yes |
| `research` | Deep multi-source research | Yes |

8 of 10 tools work locally — no account, no API key, fully private.

---

## Features

### Extraction

- **Readability scoring** — multi-signal content detection (text density, semantic tags, link ratio)
- **Noise filtering** — strips nav, footer, ads, modals, cookie banners (Tailwind-safe)
- **Data island extraction** — catches React/Next.js JSON payloads, JSON-LD, hydration data
- **YouTube metadata** — structured data from any YouTube video
- **PDF extraction** — auto-detected via Content-Type
- **5 output formats** — markdown, text, JSON, LLM-optimized, HTML

### Content control

```bash
webclaw URL --include "article, .content"       # CSS selector include
webclaw URL --exclude "nav, footer, .sidebar"    # CSS selector exclude
webclaw URL --only-main-content                  # Auto-detect main content
```

### Crawling

```bash
webclaw URL --crawl --depth 3 --max-pages 100   # BFS same-origin crawl
webclaw URL --crawl --sitemap                    # Seed from sitemap
webclaw URL --map                                # Discover URLs only
```

### LLM features (Ollama / OpenAI / Anthropic)

```bash
webclaw URL --summarize                          # Page summary
webclaw URL --extract-prompt "Get all prices"    # Natural language extraction
webclaw URL --extract-json '{"type":"object"}'   # Schema-enforced extraction
```

### Change tracking

```bash
webclaw URL -f json > snap.json                  # Take snapshot
webclaw URL --diff-with snap.json                # Compare later
```

### Brand extraction

```bash
webclaw URL --brand                              # Colors, fonts, logos, OG image
```

### Proxy rotation

```bash
webclaw URL --proxy http://user:pass@host:port   # Single proxy
webclaw URLs --proxy-file proxies.txt            # Pool rotation
```

---

## Benchmarks

All numbers from real tests on 50 diverse pages. See [benchmarks/](benchmarks/) for methodology and reproduction instructions.

### Extraction quality

```
Accuracy      webclaw     ███████████████████ 95.1%
              readability ████████████████▋   83.5%
              trafilatura ████████████████    80.6%
              newspaper3k █████████████▎      66.4%

Noise removal webclaw     ███████████████████ 96.1%
              readability █████████████████▊  89.4%
              trafilatura ██████████████████▏ 91.2%
              newspaper3k ███████████████▎    76.8%
```

### Speed (pure extraction, no network)

```
10KB page     webclaw     ██                   0.8ms
              readability █████                2.1ms
              trafilatura ██████████           4.3ms

100KB page    webclaw     ██                   3.2ms
              readability █████                8.7ms
              trafilatura ██████████           18.4ms
```

### Token efficiency (feeding to Claude/GPT)

| Format | Tokens | vs Raw HTML |
|--------|:------:|:-----------:|
| Raw HTML | 4,820 | baseline |
| readability | 2,340 | -51% |
| trafilatura | 2,180 | -55% |
| **webclaw llm** | **1,590** | **-67%** |

### Crawl speed

| Concurrency | webclaw | Crawl4AI | Scrapy |
|:-----------:|:-------:|:--------:|:------:|
| 5 | **9.8 pg/s** | 5.2 pg/s | 7.1 pg/s |
| 10 | **18.4 pg/s** | 8.7 pg/s | 12.3 pg/s |
| 20 | **32.1 pg/s** | 14.2 pg/s | 21.8 pg/s |

---

## Architecture

```
webclaw/
  crates/
    webclaw-core     Pure extraction engine. Zero network deps. WASM-safe.
    webclaw-fetch    HTTP client + TLS fingerprinting (wreq/BoringSSL). Crawler. Batch ops.
    webclaw-llm      LLM provider chain (Ollama -> OpenAI -> Anthropic)
    webclaw-pdf      PDF text extraction
    webclaw-mcp      MCP server (10 tools for AI agents)
    webclaw-cli      CLI binary
```

`webclaw-core` takes raw HTML as a `&str` and returns structured output. No I/O, no network, no allocator tricks. Can compile to WASM.

---

## Configuration

| Variable | Description |
|----------|-------------|
| `WEBCLAW_API_KEY` | Cloud API key (enables bot bypass, JS rendering, search, research) |
| `OLLAMA_HOST` | Ollama URL for local LLM features (default: `http://localhost:11434`) |
| `OPENAI_API_KEY` | OpenAI API key for LLM features |
| `ANTHROPIC_API_KEY` | Anthropic API key for LLM features |
| `WEBCLAW_PROXY` | Single proxy URL |
| `WEBCLAW_PROXY_FILE` | Path to proxy pool file |

---

## Cloud API (optional)

For bot-protected sites, JS rendering, and advanced features, webclaw offers a hosted API at [webclaw.io](https://webclaw.io).

The CLI and MCP server work locally first. Cloud is used as a fallback when:
- A site has bot protection (Cloudflare, DataDome, WAF)
- A page requires JavaScript rendering
- You use search or research tools

```bash
export WEBCLAW_API_KEY=wc_your_key

# Automatic: tries local first, cloud on bot detection
webclaw https://protected-site.com

# Force cloud
webclaw --cloud https://spa-site.com
```

### SDKs

```bash
npm install @webclaw/sdk                  # TypeScript/JavaScript
pip install webclaw                        # Python
go get github.com/0xMassi/webclaw-go      # Go
```

---

## Use cases

- **AI agents** — Give Claude/Cursor/GPT real-time web access via MCP
- **Research** — Crawl documentation, competitor sites, news archives
- **Price monitoring** — Track changes with `--diff-with` snapshots
- **Training data** — Prepare web content for fine-tuning with token-optimized output
- **Content pipelines** — Batch extract + summarize in CI/CD
- **Brand intelligence** — Extract visual identity from any website

---

## Community

- [Discord](https://discord.gg/KDfd48EpnW) — questions, feedback, show what you built
- [GitHub Issues](https://github.com/0xMassi/webclaw/issues) — bug reports and feature requests

## Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

- [Good first issues](https://github.com/0xMassi/webclaw/issues?q=label%3A%22good+first+issue%22)
- [Architecture docs](CONTRIBUTING.md#architecture)

## Acknowledgments

TLS and HTTP/2 browser fingerprinting is powered by [wreq](https://github.com/0x676e67/wreq) and [http2](https://github.com/0x676e67/http2) by [@0x676e67](https://github.com/0x676e67), who pioneered browser-grade HTTP/2 fingerprinting in Rust.

## Star History

<a href="https://www.star-history.com/?repos=0xMassi%2Fwebclaw&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=0xMassi/webclaw&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=0xMassi/webclaw&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=0xMassi/webclaw&type=date&legend=top-left" />
 </picture>
</a>

## License

[AGPL-3.0](LICENSE)
