# Webclaw

Rust workspace: CLI + MCP server for web content extraction into LLM-optimized formats.

## Architecture

```
webclaw/
  crates/
    webclaw-core/     # Pure extraction engine. WASM-safe. Zero network deps.
                      # + ExtractionOptions (include/exclude CSS selectors)
                      # + diff engine (change tracking)
                      # + brand extraction (DOM/CSS analysis)
    webclaw-fetch/    # HTTP client via wreq (BoringSSL). Crawler. Sitemap discovery. Batch ops.
                      # + proxy pool rotation (per-request)
                      # + PDF content-type detection
                      # + document parsing (DOCX, XLSX, CSV)
    webclaw-llm/      # LLM provider chain (Ollama -> OpenAI -> Anthropic)
                      # + JSON schema extraction, prompt extraction, summarization
    webclaw-pdf/      # PDF text extraction via pdf-extract
    webclaw-mcp/      # MCP server (Model Context Protocol) for AI agents
    webclaw-cli/      # CLI binary
    webclaw-server/   # Minimal axum REST API (self-hosting; OSS counterpart
                      # of api.webclaw.io, without anti-bot / JS / jobs / auth)
```

Three binaries: `webclaw` (CLI), `webclaw-mcp` (MCP server), `webclaw-server` (REST API for self-hosting).

### Core Modules (`webclaw-core`)
- `extractor.rs` — Readability-style scoring: text density, semantic tags, link density penalty
- `noise.rs` — Shared noise filter: tags, ARIA roles, class/ID patterns. Tailwind-safe.
- `data_island.rs` — JSON data island extraction for React SPAs, Next.js, Contentful CMS
- `markdown.rs` — HTML to markdown with URL resolution, asset collection
- `llm.rs` — 9-step LLM optimization pipeline (image strip, emphasis strip, link dedup, stat merge, whitespace collapse)
- `domain.rs` — Domain detection from URL patterns + DOM heuristics
- `metadata.rs` — OG, Twitter Card, standard meta tag extraction
- `types.rs` — Core data structures (ExtractionResult, Metadata, Content)
- `filter.rs` — CSS selector include/exclude filtering (ExtractionOptions)
- `diff.rs` — Content change tracking engine (snapshot diffing)
- `brand.rs` — Brand identity extraction from DOM structure and CSS

### Fetch Modules (`webclaw-fetch`)
- `client.rs` — FetchClient with wreq BoringSSL TLS impersonation; implements the public `Fetcher` trait so callers (including server adapters) can swap in alternative implementations
- `browser.rs` — Browser profiles: Chrome (142/136/133/131), Firefox (144/135/133/128)
- `crawler.rs` — BFS same-origin crawler with configurable depth/concurrency/delay
- `sitemap.rs` — Sitemap discovery and parsing (sitemap.xml, robots.txt)
- `batch.rs` — Multi-URL concurrent extraction
- `proxy.rs` — Proxy pool with per-request rotation
- `document.rs` — Document parsing: DOCX, XLSX, CSV auto-detection and extraction
- `search.rs` — Web search via Serper.dev with parallel result scraping

### LLM Modules (`webclaw-llm`)
- Provider chain: Ollama (local-first) -> OpenAI -> Anthropic
- JSON schema extraction, prompt-based extraction, summarization

### PDF Modules (`webclaw-pdf`)
- PDF text extraction via pdf-extract crate

### MCP Server (`webclaw-mcp`)
- Model Context Protocol server over stdio transport
- 8 tools: scrape, crawl, map, batch, extract, summarize, diff, brand
- Works with Claude Desktop, Claude Code, and any MCP client
- Uses `rmcp` crate (official Rust MCP SDK)

### REST API Server (`webclaw-server`)
- Axum 0.8, stateless, no database, no job queue
- 8 POST routes + /health, JSON shapes mirror api.webclaw.io where the
  capability exists in OSS
- Constant-time bearer-token auth via `subtle::ConstantTimeEq` when
  `--api-key` / `WEBCLAW_API_KEY` is set; otherwise open mode
- Hard caps: crawl ≤ 500 pages, batch ≤ 100 URLs, 20 concurrent
- Does NOT include: anti-bot bypass, JS rendering, async jobs,
  multi-tenant auth, billing, proxy rotation, search/research/watch/
  agent-scrape. Those live behind api.webclaw.io and are closed-source.

## Hard Rules

- **Core has ZERO network dependencies** — takes `&str` HTML, returns structured output. Keep it WASM-compatible.
- **webclaw-fetch uses wreq 6.x** (BoringSSL). No `[patch.crates-io]` forks needed; wreq handles TLS internally.
- **No special RUSTFLAGS** — `.cargo/config.toml` is currently empty of build flags. Don't add any.
- **webclaw-llm uses plain reqwest**. LLM APIs don't need TLS fingerprinting, so no wreq dep.
- **Vertical extractors take `&dyn Fetcher`**, not `&FetchClient`. This lets the production server plug in a `ProductionFetcher` that adds domain_hints routing and antibot escalation on top of the same wreq client.
- **qwen3 thinking tags** (`<think>`) are stripped at both provider and consumer levels.

## Build & Test

```bash
cargo build --release           # Both binaries
cargo test --workspace          # All tests
cargo test -p webclaw-core      # Core only
cargo test -p webclaw-llm       # LLM only
```

## CLI

```bash
# Basic extraction
webclaw https://example.com
webclaw https://example.com --format llm

# Content filtering
webclaw https://example.com --include "article" --exclude "nav,footer"
webclaw https://example.com --only-main-content

# Batch + proxy rotation
webclaw url1 url2 url3 --proxy-file proxies.txt
webclaw --urls-file urls.txt --concurrency 10

# Sitemap discovery
webclaw https://docs.example.com --map

# Crawling (with sitemap seeding)
webclaw https://docs.example.com --crawl --depth 2 --max-pages 50 --sitemap

# Change tracking
webclaw https://example.com -f json > snap.json
webclaw https://example.com --diff-with snap.json

# Brand extraction
webclaw https://example.com --brand

# LLM features (Ollama local-first)
webclaw https://example.com --summarize
webclaw https://example.com --extract-prompt "Get all pricing tiers"
webclaw https://example.com --extract-json '{"type":"object","properties":{"title":{"type":"string"}}}'

# PDF (auto-detected via Content-Type)
webclaw https://example.com/report.pdf

# Browser impersonation: chrome (default), firefox, random
webclaw https://example.com --browser firefox

# Local file / stdin
webclaw --file page.html
cat page.html | webclaw --stdin
```

## Key Thresholds

- Scoring minimum: 50 chars text length
- Semantic bonus: +50 for `<article>`/`<main>`, +25 for content class/ID
- Link density: >50% = 0.1x score, >30% = 0.5x
- Data island fallback triggers when DOM word count < 30
- Eyebrow text max: 80 chars

## MCP Setup

Add to Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json`):
```json
{
  "mcpServers": {
    "webclaw": {
      "command": "/path/to/webclaw-mcp"
    }
  }
}
```

## Skills

- `/scrape <url>` — extract content from a URL
- `/benchmark [url]` — run extraction performance benchmarks
- `/research <url>` — deep web research via crawl + extraction
- `/crawl <url>` — crawl a website
- `/commit` — conventional commit with change analysis

## Git

- Remote: `git@github.com:0xMassi/webclaw.git`
- Use `/commit` skill for commits
