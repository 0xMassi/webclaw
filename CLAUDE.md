# Noxa

Rust workspace: CLI + MCP server for web content extraction into LLM-optimized formats.

## Architecture

```
noxa/
  crates/
    noxa-core/     # Pure extraction engine. WASM-safe. Zero network deps.
                      # + ExtractionOptions (include/exclude CSS selectors)
                      # + diff engine (change tracking)
                      # + brand extraction (DOM/CSS analysis)
    noxa-fetch/    # HTTP client via primp. Crawler. Sitemap discovery. Batch ops.
                      # + proxy pool rotation (per-request)
                      # + PDF content-type detection
                      # + document parsing (DOCX, XLSX, CSV)
    noxa-llm/      # LLM provider chain (Gemini CLI -> OpenAI -> Ollama -> Anthropic)
                      # + JSON schema extraction (validated + retry), prompt extraction, summarization
    noxa-pdf/      # PDF text extraction via pdf-extract
    noxa-mcp/      # MCP server (Model Context Protocol) for AI agents
    noxa/      # CLI binary
```

Two binaries: `noxa` (CLI), `noxa-mcp` (MCP server).

### Core Modules (`noxa-core`)
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

### Fetch Modules (`noxa-fetch`)
- `client.rs` — FetchClient with primp TLS impersonation
- `browser.rs` — Browser profiles: Chrome (142/136/133/131), Firefox (144/135/133/128)
- `crawler.rs` — BFS same-origin crawler with configurable depth/concurrency/delay
- `sitemap.rs` — Sitemap discovery and parsing (sitemap.xml, robots.txt)
- `batch.rs` — Multi-URL concurrent extraction
- `proxy.rs` — Proxy pool with per-request rotation
- `document.rs` — Document parsing: DOCX, XLSX, CSV auto-detection and extraction
- `search.rs` — Web search via Serper.dev with parallel result scraping

### LLM Modules (`noxa-llm`)
- Provider chain: Gemini CLI (primary) -> OpenAI -> Ollama -> Anthropic
- Gemini CLI requires the `gemini` binary on PATH; `GEMINI_MODEL` env var controls model (default: `gemini-2.5-pro`)
- JSON schema extraction with jsonschema validation; parse failures retry once; schema mismatches fail immediately
- Prompt-based extraction, summarization

### PDF Modules (`noxa-pdf`)
- PDF text extraction via pdf-extract crate

### MCP Server (`noxa-mcp`)
- Model Context Protocol server over stdio transport
- 8 tools: scrape, crawl, map, batch, extract, summarize, diff, brand
- Works with Claude Desktop, Claude Code, and any MCP client
- Uses `rmcp` crate (official Rust MCP SDK)

## Hard Rules

- **Core has ZERO network dependencies** — takes `&str` HTML, returns structured output. Keep it WASM-compatible.
- **primp requires `[patch.crates-io]`** for patched rustls/h2 forks at workspace level.
- **RUSTFLAGS are set in `.cargo/config.toml`** — no need to pass manually.
- **noxa-llm uses plain reqwest** (NOT primp-patched). LLM APIs don't need TLS fingerprinting.
- **qwen3 thinking tags** (`<think>`) are stripped at both provider and consumer levels.

## Build & Test

```bash
cargo build --release           # Both binaries
cargo test --workspace          # All tests
cargo test -p noxa-core      # Core only
cargo test -p noxa-llm       # LLM only
```

## CLI

```bash
# Basic extraction
noxa https://example.com
noxa https://example.com --format llm

# Content filtering
noxa https://example.com --include "article" --exclude "nav,footer"
noxa https://example.com --only-main-content

# Batch + proxy rotation
noxa url1 url2 url3 --proxy-file proxies.txt
noxa --urls-file urls.txt --concurrency 10

# Sitemap discovery
noxa https://docs.example.com --map

# Crawling (with sitemap seeding)
noxa https://docs.example.com --crawl --depth 2 --max-pages 50 --sitemap

# Change tracking
noxa https://example.com -f json > snap.json
noxa https://example.com --diff-with snap.json

# Brand extraction
noxa https://example.com --brand

# LLM features (Gemini CLI primary; requires `gemini` on PATH)
noxa https://example.com --summarize
noxa https://example.com --extract-prompt "Get all pricing tiers"
noxa https://example.com --extract-json '{"type":"object","properties":{"title":{"type":"string"}}}'

# Force a specific LLM provider
noxa https://example.com --llm-provider gemini --summarize
noxa https://example.com --llm-provider openai --summarize

# PDF (auto-detected via Content-Type)
noxa https://example.com/report.pdf

# Browser impersonation: chrome (default), firefox, random
noxa https://example.com --browser firefox

# Local file / stdin
noxa --file page.html
cat page.html | noxa --stdin
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
    "noxa": {
      "command": "/path/to/noxa-mcp"
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

- Remote: `git@github.com:jmagar/noxa.git`
- Use `/commit` skill for commits
