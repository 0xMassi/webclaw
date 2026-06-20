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
                      # + layered URL discovery (map) + Serper web search (BYO key)
    webclaw-llm/      # LLM provider chain (Ollama -> OpenAI -> Gemini -> Anthropic)
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
- `structured_data.rs` — JSON-LD, Next.js `__NEXT_DATA__`, and SvelteKit data-island extraction
- `js_eval.rs` — QuickJS sandbox (rquickjs) that runs inline `<script>` tags to recover JS-assigned blobs (`window.__PRELOADED_STATE__`, Next.js `self.__next_f`) the static path can't see. Behind the default `quickjs` feature, gated `cfg(not(target_arch = "wasm32"))` — rquickjs links a C lib and won't build for wasm. Never ungate it (see Hard Rules). Runtime-gated for speed: the VM is skipped entirely when the page has no JS-candidate markers (`has_js_candidate_data`), and it reuses the already-parsed document instead of re-parsing.
- `endpoints.rs` — API surface discovery: REST paths, GraphQL, and WebSocket endpoints mined from inline scripts + JS bundle text (regex over string literals, DoS-bounded). Pure: caller passes raw text.
- `markdown.rs` — HTML to markdown with URL resolution, asset collection
- `llm/` — directory module (`mod` + `body`/`cleanup`/`images`/`links`/`metadata`): 9-step LLM optimization pipeline (image strip, emphasis strip, link dedup, stat merge, whitespace collapse)
- `domain.rs` — Domain detection from URL patterns + DOM heuristics
- `metadata.rs` — OG, Twitter Card, standard meta tag extraction
- `types.rs` — Core data structures (ExtractionResult, Metadata, Content, plus ExtractionOptions for include/exclude CSS selectors — applied in `extractor.rs`; there is no `filter.rs`)
- `diff.rs` — Content change tracking engine (snapshot diffing)
- `brand.rs` — Brand identity extraction from DOM structure and CSS
- `reddit.rs` — old.reddit.com thread vertical extractor (parses server-rendered HTML directly; no JS/API key). Test fixtures under `testdata/reddit/*.html` are `exclude`d from the published crate (Cargo.toml).
- `youtube.rs` — `ytInitialPlayerResponse` parser, structured markdown for `youtube.com/watch` URLs (title, channel, views, published, duration, description). Produces the legacy markdown shape — for transcripts and a structured `YoutubeData` block see the production server's `youtube_transcript.rs` short-circuit (yt-dlp via proxy pool).

### Fetch Modules (`webclaw-fetch`)
- `client.rs` — `FetchClient` with wreq BoringSSL TLS impersonation; also implements batch (`BatchResult`/`BatchExtractResult` — there is no `batch.rs`). Implements the public `Fetcher` trait so callers (incl. server adapters) can swap implementations.
- `fetcher.rs` — the public `Fetcher` trait (`Send + Sync`). Vertical extractors take `&dyn Fetcher`, not `&FetchClient`.
- `browser.rs` — `BrowserProfile`/`BrowserVariant` enums only (Chrome, ChromeMacos, Firefox, Safari, SafariIos26, Edge). No version numbers live here.
- `tls.rs` — the real fingerprint builder: per-variant wreq `Emulation` (cipher/sigalg/curve lists, TLS extension order, HTTP/2 SETTINGS, header wire-order). Browser versions are set HERE: Chrome 145, Firefox 135, Edge 145, Safari 18.3.1, Safari iOS 26. SafariIos26 composes on top of `wreq_util::Profile::SafariIos26`. SSRF-safe redirect policy lives here too.
- `extractors/` — ~30 vertical site extractors (Amazon, eBay, GitHub, Instagram, LinkedIn, Reddit, YouTube, npm, PyPI, HuggingFace, Etsy, Shopify, WooCommerce, Trustpilot, arXiv, Hacker News, StackOverflow, ...); `extractors/mod.rs` is the dispatch table. All reach the network through `&dyn Fetcher`. Shared helpers (not verticals themselves): `extractors/og.rs` (single-pass Open Graph `og:*` parser, `raw()` vs `unescaped()`), `extractors/github_common.rs` (shared GitHub API fetch + status handling), `extractors/jsonld_product.rs` / `ecommerce_product.rs` (shared JSON-LD product walker reused by the e-commerce verticals).
- `reddit.rs` / `linkedin.rs` — top-level fetch-side verticals (distinct from `extractors/` and from `webclaw-core`'s parsers): `reddit.rs` rewrites Reddit hosts to `old.reddit.com` (the `*.json` API is blocked) so `webclaw-core::reddit` can parse server-rendered HTML; `linkedin.rs` reconstructs post + comments from the SPA's HTML-escaped JSON in `<code>` tags (the `included` typed-entity array).
- `progress.rs` — wraps a slow fetch future in `tokio::select!` against an interval, emitting a periodic `# webclaw: still fetching <URL> (Ns)` line to STDERR.
- `crawler.rs` — BFS same-origin crawler with configurable depth/concurrency/delay
- `sitemap.rs` — Sitemap discovery and parsing (sitemap.xml, robots.txt; gzip `.xml.gz` supported via `decode_sitemap_body`, sitemap-index recursion)
- `map.rs` — layered URL discovery (`discover_urls` / `MapOptions`): sitemaps first, then a bounded same-origin crawl fallback when the sitemap is thin, harvesting links from fetched pages + the unfetched frontier (deduped against the sitemap set)
- `search.rs` — web search via Serper.dev with the caller's own key (`search` / `SearchOptions` / `SearchResult`; pure `parse_serper_organic`). Plain wreq client (JSON API, no fingerprinting); optional bounded concurrent fetch+extract of result pages. Powers the CLI `search` subcommand, the MCP `search` tool, and the OSS server `POST /v1/search`.
- `proxy.rs` — Proxy pool with per-request rotation
- `document.rs` — Document parsing: DOCX, XLSX, CSV auto-detection and extraction
- `cloud.rs` — `CloudClient` for hosted antibot escalation, exposed via `Fetcher::cloud()`
- `locale.rs` — Accept-Language by TLD (`accept_language_for_tld` / `_for_url`)
- `url_security.rs` — SSRF guards + SSRF-safe redirect policy

### LLM Modules (`webclaw-llm`)
- Provider chain (`chain.rs`): Ollama (local-first, always added; availability checked at call time) -> OpenAI -> Gemini -> Anthropic. Gemini sits ahead of Anthropic so Google Cloud credits are preferred; Anthropic is the last-resort fallback. Each provider lives in `providers/` (`ollama.rs`, `openai.rs`, `gemini.rs`, `anthropic.rs`).
- JSON schema extraction, prompt-based extraction, summarization

### PDF Modules (`webclaw-pdf`)
- PDF text extraction via pdf-extract crate

### MCP Server (`webclaw-mcp`)
- Model Context Protocol server over stdio transport
- 12 tools: scrape, crawl, map, batch, extract, summarize, diff, brand, research, search, list_extractors, vertical_scrape. `search` is local-first via the caller's `SERPER_API_KEY` (falls back to the hosted API when unset); `research` uses the hosted deep-research API. The rest run locally.
- Works with Claude Desktop, Claude Code, and any MCP client
- Uses `rmcp` crate (official Rust MCP SDK)

### REST API Server (`webclaw-server`)
- Axum 0.8, stateless, no database, no job queue
- 10 POST routes (incl. `POST /v1/scrape/{vertical}` and `POST /v1/search`) +
  `GET /v1/extractors` + `GET /health`. JSON shapes mirror api.webclaw.io
  where the capability exists in OSS. The vertical surface
  (`routes/structured.rs`) mirrors the MCP `list_extractors` /
  `vertical_scrape` tools. `POST /v1/search` is gated on `SERPER_API_KEY`
  (returns 501 when unset).
- Constant-time bearer-token auth via `subtle::ConstantTimeEq` when
  `--api-key` / `WEBCLAW_API_KEY` is set; otherwise open mode
- Hard caps: crawl ≤ 500 pages, batch ≤ 100 URLs, 20 concurrent
- Does NOT include: anti-bot bypass, JS rendering, async jobs,
  multi-tenant auth, billing, proxy rotation, research/watch/
  agent-scrape. Those live behind api.webclaw.io and are closed-source.
  (Web search IS available here as a bring-your-own-Serper-key path.)

## Hard Rules

- **Core has ZERO network dependencies** — takes `&str` HTML, returns structured output. Keep it WASM-compatible. The `quickjs` feature (default ON) pulls in rquickjs, which links a C lib and can't target wasm32; it's gated `cfg(not(target_arch = "wasm32"))` in `lib.rs`. CI compiles webclaw-core for wasm32 both with AND without default features — never ungate that.
- **webclaw-fetch pins wreq exactly**: `wreq = "=6.0.0-rc.29"` + `wreq-util = "=3.0.0-rc.12"` (BoringSSL). The `=` pin is deliberate — these are release candidates with no semver stability between rc.N builds. No `[patch.crates-io]` forks needed; wreq handles TLS internally.
- **No build flags in `.cargo/config.toml`** (it is comments-only) — don't add any locally. BUT CI (`.github/workflows/ci.yml`, `deps.yml`) DOES export `RUSTFLAGS: "--cfg reqwest_unstable"` for the wreq path; don't remove it from CI.
- **webclaw-llm uses plain reqwest**. LLM APIs don't need TLS fingerprinting, so no wreq dep.
- **Vertical extractors take `&dyn Fetcher`**, not `&FetchClient`. This lets the production server plug in a `ProductionFetcher` that adds domain_hints routing and antibot escalation on top of the same wreq client.
- **qwen3 thinking tags** (`<think>`) are stripped at both provider and consumer levels.

## Build & Test

```bash
cargo build --release           # All three binaries (webclaw, webclaw-mcp, webclaw-server)
cargo test --workspace          # All tests
cargo test -p webclaw-core      # Core only
cargo test -p webclaw-llm       # LLM only
```

CI (`.github/workflows/ci.yml`, with `RUSTFLAGS=--cfg reqwest_unstable`) runs four jobs — match them locally before pushing:
- `cargo test --workspace`
- `cargo fmt --check --all` + `cargo clippy --all -- -D warnings` (warnings fail CI)
- `cargo check --target wasm32-unknown-unknown -p webclaw-core` **with and without** `--no-default-features` (guards the WASM-safe rule)
- `cargo doc --no-deps --workspace`

## Repo Layout & Packaging

Workspace is version **0.6.13**, edition **2024**, license **AGPL-3.0** (matters for the public-OSS scrubbing rules). No crate declares `rust-version`, so MSRV is implicit — edition 2024 floors it at Rust 1.85+; CI pins `dtolnay/rust-toolchain@stable`.

Artifacts outside `crates/` that need separate attention:
- `packages/create-webclaw/` — `npx create-webclaw` Node scaffolder that installs/configures the MCP server for AI agents (Claude, Cursor, Windsurf, ...). Versioned independently (own `package.json`) — bump it separately when MCP setup changes.
- `smithery.yaml` + `glama.json` — MCP-registry manifests (Smithery stdio config spawning `webclaw-mcp` with optional `WEBCLAW_API_KEY`; Glama). Update when the MCP launch command or env changes.
- `examples/` — runnable demos (cloudflare-diagnostics, firecrawl-compatible-api, html-to-markdown-rag, mcp-web-scraping, proxy-backed-crawling).
- `Dockerfile` / `Dockerfile.ci` / `docker-compose.yml`, `benchmarks/` (`/benchmark` skill), `SKILL.md` + `skill/` (Claude Code skill).

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

# URL discovery (--map): sitemaps first, bounded crawl fallback when the sitemap is thin
webclaw https://docs.example.com --map
webclaw https://news.ycombinator.com --map --map-pages 150 --map-limit 500
webclaw https://docs.example.com --map --no-map-crawl   # sitemap-only (no crawl fallback)

# Crawling (with sitemap seeding)
webclaw https://docs.example.com --crawl --depth 2 --max-pages 50 --sitemap

# Web search via Serper.dev (bring your own key: --serper-key or SERPER_API_KEY)
webclaw search "rust async runtime" --num 5
webclaw search "best web scraper" --scrape -f json   # also fetch + extract result pages

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
- Link density (generic divs): >50% = 0.1x score, >30% = 0.5x. Semantic nodes (article/main/role=main) get a milder curve: >70% = 0.3x, >50% = 0.5x (`extractor.rs`)
- Data island fallback triggers when DOM word count < 500 (`SPARSE_THRESHOLD` in `data_island.rs`)
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
