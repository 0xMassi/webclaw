<p align="center">
  <a href="https://noxa.io">
    <img src=".github/banner.png" alt="noxa" width="700" />
  </a>
</p>

<h3 align="center">
  The fastest web scraper for AI agents.<br/>
  <sub>67% fewer tokens. Sub-millisecond extraction. Zero browser overhead.</sub>
</h3>

<p align="center">
  <a href="https://github.com/jmagar/noxa/stargazers"><img src="https://img.shields.io/github/stars/jmagar/noxa?style=for-the-badge&logo=github&logoColor=white&label=Stars&color=181717" alt="Stars" /></a>
  <a href="https://github.com/jmagar/noxa/releases"><img src="https://img.shields.io/github/v/release/jmagar/noxa?style=for-the-badge&logo=rust&logoColor=white&label=Version&color=B7410E" alt="Version" /></a>
  <a href="https://github.com/jmagar/noxa/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-AGPL--3.0-10B981?style=for-the-badge" alt="License" /></a>
  <a href="https://www.npmjs.com/package/create-noxa"><img src="https://img.shields.io/npm/dt/create-noxa?style=for-the-badge&logo=npm&logoColor=white&label=Installs&color=CB3837" alt="npm installs" /></a>
</p>
<p align="center">
  <a href="https://x.com/noxa_io"><img src="https://img.shields.io/badge/Follow-@noxa__io-000000?style=for-the-badge&logo=x&logoColor=white" alt="X / Twitter" /></a>
  <a href="https://noxa.io"><img src="https://img.shields.io/badge/Website-noxa.io-0A0A0A?style=for-the-badge&logo=safari&logoColor=white" alt="Website" /></a>
  <a href="https://noxa.io/docs"><img src="https://img.shields.io/badge/Docs-Read-3B82F6?style=for-the-badge&logo=readthedocs&logoColor=white" alt="Docs" /></a>
</p>

---

<p align="center">
  <img src="assets/demo.gif" alt="Claude Code: web_fetch gets 403, noxa extracts successfully" width="700" />
  <br/>
  <sub>Claude Code's built-in web_fetch → 403 Forbidden. noxa → clean markdown.</sub>
</p>

---

Your AI agent calls `fetch()` and gets a 403. Or 142KB of raw HTML that burns through your token budget. **noxa fixes both.**

It extracts clean, structured content from any URL using Chrome-level TLS fingerprinting — no headless browser, no Selenium, no Puppeteer. Output is optimized for LLMs: **67% fewer tokens** than raw HTML, with metadata, links, and images preserved.

```
                     Raw HTML                          noxa
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

## Get Started (30 seconds)

### For AI agents (Claude, Cursor, Windsurf, VS Code)

```bash
npx create-noxa
```

Auto-detects your AI tools, downloads the MCP server, and configures everything. One command.

### Homebrew (macOS/Linux)

```bash
brew tap jmagar/noxa
brew install noxa
```

### Prebuilt binaries

Download from [GitHub Releases](https://github.com/jmagar/noxa/releases) for macOS (arm64, x86_64) and Linux (x86_64, aarch64).

### Cargo (from source)

```bash
cargo install --git https://github.com/jmagar/noxa.git noxa-cli --bin noxa
cargo install --git https://github.com/jmagar/noxa.git noxa-mcp
```

### Docker

```bash
docker run --rm ghcr.io/0xmassi/noxa https://example.com
```

### Docker Compose (with Ollama for LLM features)

```bash
cp env.example .env
docker compose up -d
```

---

## Why noxa?

| | noxa | Firecrawl | Trafilatura | Readability |
|---|:---:|:---:|:---:|:---:|
| **Extraction accuracy** | **95.1%** | — | 80.6% | 83.5% |
| **Token efficiency** | **-67%** | — | -55% | -51% |
| **Speed (100KB page)** | **3.2ms** | ~500ms | 18.4ms | 8.7ms |
| **TLS fingerprinting** | Yes | No | No | No |
| **Self-hosted** | Yes | No | Yes | Yes |
| **MCP (Claude/Cursor)** | Yes | No | No | No |
| **No browser required** | Yes | No | Yes | Yes |
| **Cost** | Free | $$$$ | Free | Free |

**Choose noxa if** you want fast local extraction, LLM-optimized output, and native AI agent integration.

---

## What it looks like

```bash
$ noxa https://stripe.com -f llm

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
$ noxa https://github.com --brand

{
  "name": "GitHub",
  "colors": [{"hex": "#59636E", "usage": "Primary"}, ...],
  "fonts": ["Mona Sans", "ui-monospace"],
  "logos": [{"url": "https://github.githubassets.com/...", "kind": "svg"}]
}
```

```bash
$ noxa https://docs.rust-lang.org --crawl --depth 2 --max-pages 50

Crawling... 50/50 pages extracted
---
# Page 1: https://docs.rust-lang.org/
...
# Page 2: https://docs.rust-lang.org/book/
...
```

---

## Examples

### Basic Extraction

```bash
# Extract as markdown (default)
noxa https://example.com

# Multiple output formats
noxa https://example.com -f markdown    # Clean markdown
noxa https://example.com -f json        # Full structured JSON
noxa https://example.com -f text        # Plain text (no formatting)
noxa https://example.com -f llm         # Token-optimized for LLMs (67% fewer tokens)

# Bare domains work (auto-prepends https://)
noxa example.com
```

### Content Filtering

```bash
# Only extract main content (skip nav, sidebar, footer)
noxa https://docs.rs/tokio --only-main-content

# Include specific CSS selectors
noxa https://news.ycombinator.com --include ".titleline,.score"

# Exclude specific elements
noxa https://example.com --exclude "nav,footer,.ads,.sidebar"

# Combine both
noxa https://docs.rs/reqwest --only-main-content --exclude ".sidebar"
```

### Brand Identity Extraction

```bash
# Extract colors, fonts, logos from any website
noxa --brand https://stripe.com
# Output: { "name": "Stripe", "colors": [...], "fonts": ["Sohne"], "logos": [...] }

noxa --brand https://github.com
# Output: { "name": "GitHub", "colors": [{"hex": "#1F2328", ...}], "fonts": ["Mona Sans"], ... }

noxa --brand wikipedia.org
# Output: 10 colors, 5 fonts, favicon, logo URL
```

### Sitemap Discovery

```bash
# Discover all URLs from a site's sitemaps
noxa --map https://sitemaps.org
# Output: one URL per line (84 URLs found)

# JSON output with metadata
noxa --map https://sitemaps.org -f json
# Output: [{ "url": "...", "last_modified": "...", "priority": 0.8 }]
```

### Recursive Crawling

```bash
# Crawl a site (default: depth 1, max 20 pages)
noxa --crawl https://example.com

# Control depth and page limit
noxa --crawl --depth 2 --max-pages 50 https://docs.rs/tokio

# Crawl with sitemap seeding (finds more pages)
noxa --crawl --sitemap --depth 2 https://docs.rs/tokio

# Filter crawl paths
noxa --crawl --include-paths "/api/*,/guide/*" https://docs.example.com
noxa --crawl --exclude-paths "/changelog/*,/blog/*" https://docs.example.com

# Control concurrency and delay
noxa --crawl --concurrency 10 --delay 200 https://example.com
```

### Change Detection (Diff)

```bash
# Step 1: Save a snapshot
noxa https://example.com -f json > snapshot.json

# Step 2: Later, compare against the snapshot
noxa --diff-with snapshot.json https://example.com
# Output:
#   Status: Same
#   Word count delta: +0

# If the page changed:
#   Status: Changed
#   Word count delta: +42
#   --- old
#   +++ new
#   @@ -1,3 +1,3 @@
#   -Old content here
#   +New content here
```

### PDF Extraction

```bash
# PDF URLs are auto-detected via Content-Type
noxa https://example.com/report.pdf

# Control PDF mode
noxa --pdf-mode auto https://example.com/report.pdf  # Error on empty (catches scanned PDFs)
noxa --pdf-mode fast https://example.com/report.pdf  # Return whatever text is found
```

### Batch Processing

```bash
# Multiple URLs in one command
noxa https://example.com https://httpbin.org/html https://rust-lang.org

# URLs from a file (one per line, # comments supported)
noxa --urls-file urls.txt

# Batch with JSON output
noxa --urls-file urls.txt -f json

# Proxy rotation for large batches
noxa --urls-file urls.txt --proxy-file proxies.txt --concurrency 10
```

### Local Files & Stdin

```bash
# Extract from a local HTML file
noxa --file page.html

# Pipe HTML from another command
curl -s https://example.com | noxa --stdin

# Chain with other tools
noxa https://example.com -f text | wc -w    # Word count
noxa https://example.com -f json | jq '.metadata.title'  # Extract title with jq
```

### Browser Impersonation

```bash
# Chrome (default) — latest Chrome TLS fingerprint
noxa https://example.com

# Firefox fingerprint
noxa --browser firefox https://example.com

# Random browser per request (good for batch)
noxa --browser random --urls-file urls.txt
```

### Custom Headers & Cookies

```bash
# Custom headers
noxa -H "Authorization: Bearer token123" https://api.example.com
noxa -H "Accept-Language: de-DE" https://example.com

# Cookies
noxa --cookie "session=abc123; theme=dark" https://example.com

# Multiple headers
noxa -H "X-Custom: value" -H "Authorization: Bearer token" https://example.com
```

### LLM-Powered Features

These require an LLM provider (Ollama local, or OpenAI/Anthropic API key).

```bash
# Summarize a page (default: 3 sentences)
noxa --summarize https://example.com

# Control summary length
noxa --summarize 5 https://example.com

# Extract structured JSON with a schema
noxa --extract-json '{"type":"object","properties":{"title":{"type":"string"},"price":{"type":"number"}}}' https://example.com/product

# Extract with a schema from file
noxa --extract-json @schema.json https://example.com/product

# Extract with natural language prompt
noxa --extract-prompt "Get all pricing tiers with name, price, and features" https://stripe.com/pricing

# Use a specific LLM provider
noxa --llm-provider ollama --summarize https://example.com
noxa --llm-provider openai --llm-model gpt-4o --extract-prompt "..." https://example.com
noxa --llm-provider anthropic --summarize https://example.com
```

### Raw HTML Output

```bash
# Get the raw fetched HTML (no extraction)
noxa --raw-html https://example.com

# Useful for debugging extraction issues
noxa --raw-html https://example.com > raw.html
noxa --file raw.html  # Then extract locally
```

### Metadata & Verbose Mode

```bash
# Include YAML frontmatter with metadata
noxa --metadata https://example.com
# Output:
#   ---
#   title: "Example Domain"
#   source: "https://example.com"
#   word_count: 20
#   ---
#   # Example Domain
#   ...

# Verbose logging (debug extraction pipeline)
noxa -v https://example.com
```

### Proxy Usage

```bash
# Single proxy
noxa --proxy http://user:pass@proxy.example.com:8080 https://example.com

# SOCKS5 proxy
noxa --proxy socks5://proxy.example.com:1080 https://example.com

# Proxy rotation from file (one per line: host:port:user:pass)
noxa --proxy-file proxies.txt https://example.com

# Auto-load proxies.txt from current directory
echo "proxy1.com:8080:user:pass" > proxies.txt
noxa https://example.com  # Automatically detects and uses proxies.txt
```

### Real-World Recipes

```bash
# Monitor competitor pricing — save today's pricing
noxa --extract-json '{"type":"array","items":{"type":"object","properties":{"plan":{"type":"string"},"price":{"type":"string"}}}}' \
  https://competitor.com/pricing -f json > pricing-$(date +%Y%m%d).json

# Build a documentation search index
noxa --crawl --sitemap --depth 3 --max-pages 500 -f llm https://docs.example.com > docs.txt

# Extract all images from a page
noxa https://example.com -f json | jq -r '.content.images[].src'

# Get all external links
noxa https://example.com -f json | jq -r '.content.links[] | select(.href | startswith("http")) | .href'

# Compare two pages
noxa https://site-a.com -f json > a.json
noxa https://site-b.com --diff-with a.json
```

---

## MCP Server — 10 tools for AI agents

<a href="https://glama.ai/mcp/servers/jmagar/noxa"><img src="https://glama.ai/mcp/servers/jmagar/noxa/badge" alt="noxa MCP server" /></a>

noxa ships as an MCP server that plugs into Claude Desktop, Claude Code, Cursor, Windsurf, OpenCode, Antigravity, Codex CLI, and any MCP-compatible client.

```bash
npx create-noxa    # auto-detects and configures everything
```

Or manual setup — add to your Claude Desktop config:

```json
{
  "mcpServers": {
    "noxa": {
      "command": "~/.noxa/noxa-mcp"
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
noxa URL --include "article, .content"       # CSS selector include
noxa URL --exclude "nav, footer, .sidebar"    # CSS selector exclude
noxa URL --only-main-content                  # Auto-detect main content
```

### Crawling

```bash
noxa URL --crawl --depth 3 --max-pages 100   # BFS same-origin crawl
noxa URL --crawl --sitemap                    # Seed from sitemap
noxa URL --map                                # Discover URLs only
```

### LLM features (Ollama / OpenAI / Anthropic)

```bash
noxa URL --summarize                          # Page summary
noxa URL --extract-prompt "Get all prices"    # Natural language extraction
noxa URL --extract-json '{"type":"object"}'   # Schema-enforced extraction
```

### Change tracking

```bash
noxa URL -f json > snap.json                  # Take snapshot
noxa URL --diff-with snap.json                # Compare later
```

### Brand extraction

```bash
noxa URL --brand                              # Colors, fonts, logos, OG image
```

### Proxy rotation

```bash
noxa URL --proxy http://user:pass@host:port   # Single proxy
noxa URLs --proxy-file proxies.txt            # Pool rotation
```

---

## Benchmarks

All numbers from real tests on 50 diverse pages. See [benchmarks/](benchmarks/) for methodology and reproduction instructions.

### Extraction quality

```
Accuracy      noxa     ███████████████████ 95.1%
              readability ████████████████▋   83.5%
              trafilatura ████████████████    80.6%
              newspaper3k █████████████▎      66.4%

Noise removal noxa     ███████████████████ 96.1%
              readability █████████████████▊  89.4%
              trafilatura ██████████████████▏ 91.2%
              newspaper3k ███████████████▎    76.8%
```

### Speed (pure extraction, no network)

```
10KB page     noxa     ██                   0.8ms
              readability █████                2.1ms
              trafilatura ██████████           4.3ms

100KB page    noxa     ██                   3.2ms
              readability █████                8.7ms
              trafilatura ██████████           18.4ms
```

### Token efficiency (feeding to Claude/GPT)

| Format | Tokens | vs Raw HTML |
|--------|:------:|:-----------:|
| Raw HTML | 4,820 | baseline |
| readability | 2,340 | -51% |
| trafilatura | 2,180 | -55% |
| **noxa llm** | **1,590** | **-67%** |

### Crawl speed

| Concurrency | noxa | Crawl4AI | Scrapy |
|:-----------:|:-------:|:--------:|:------:|
| 5 | **9.8 pg/s** | 5.2 pg/s | 7.1 pg/s |
| 10 | **18.4 pg/s** | 8.7 pg/s | 12.3 pg/s |
| 20 | **32.1 pg/s** | 14.2 pg/s | 21.8 pg/s |

---

## Architecture

```
noxa/
  crates/
    noxa-core     Pure extraction engine. Zero network deps. WASM-safe.
    noxa-fetch    HTTP client + TLS fingerprinting (wreq/BoringSSL). Crawler. Batch ops.
    noxa-llm      LLM provider chain (Ollama -> OpenAI -> Anthropic)
    noxa-pdf      PDF text extraction
    noxa-mcp      MCP server (10 tools for AI agents)
    noxa      CLI binary
```

`noxa-core` takes raw HTML as a `&str` and returns structured output. No I/O, no network, no allocator tricks. Can compile to WASM.

---

## Configuration

Non-secret defaults live in `config.json` in your working directory. Copy the example:

```bash
cp config.example.json config.json
```

**Precedence:** CLI flags > `config.json` > built-in defaults

**Secrets and URLs** (API keys, proxy, webhook, LLM base URL) always go in `.env`, not `config.json`:

```bash
cp env.example .env
```

**Override config path** for a single run:

```bash
NOXA_CONFIG=/path/to/other-config.json noxa https://example.com
NOXA_CONFIG=/dev/null noxa https://example.com  # bypass config entirely
```

**Bool flag limitation:** flags like `--metadata`, `--only-main-content`, `--verbose` set to `true` in `config.json` cannot be overridden to `false` from the CLI for a single run (clap has no `--no-flag` variant). Use `NOXA_CONFIG=/dev/null` to bypass.

### Environment variables

| Variable | Description |
|----------|-------------|
| `NOXA_API_KEY` | Cloud API key (enables bot bypass, JS rendering, search, research) |
| `OLLAMA_HOST` | Ollama URL for local LLM features (default: `http://localhost:11434`) |
| `OPENAI_API_KEY` | OpenAI API key for LLM features |
| `ANTHROPIC_API_KEY` | Anthropic API key for LLM features |
| `NOXA_PROXY` | Single proxy URL |
| `NOXA_PROXY_FILE` | Path to proxy pool file |

---

## Cloud API (optional)

For bot-protected sites, JS rendering, and advanced features, noxa offers a hosted API at [noxa.io](https://noxa.io).

The CLI and MCP server work locally first. Cloud is used as a fallback when:
- A site has bot protection (Cloudflare, DataDome, WAF)
- A page requires JavaScript rendering
- You use search or research tools

```bash
export NOXA_API_KEY=wc_your_key

# Automatic: tries local first, cloud on bot detection
noxa https://protected-site.com

# Force cloud
noxa --cloud https://spa-site.com
```

### SDKs

```bash
npm install @noxa/sdk                  # TypeScript/JavaScript
pip install noxa                        # Python
go get github.com/jmagar/noxa-go      # Go
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

- [GitHub Issues](https://github.com/jmagar/noxa/issues) — bug reports and feature requests

## Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

- [Good first issues](https://github.com/jmagar/noxa/issues?q=label%3A%22good+first+issue%22)
- [Architecture docs](CONTRIBUTING.md#architecture)

## Acknowledgments

TLS and HTTP/2 browser fingerprinting is powered by [wreq](https://github.com/0x676e67/wreq) and [http2](https://github.com/0x676e67/http2) by [@0x676e67](https://github.com/0x676e67), who pioneered browser-grade HTTP/2 fingerprinting in Rust.

## License

[AGPL-3.0](LICENSE)
