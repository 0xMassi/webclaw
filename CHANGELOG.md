# Changelog

All notable changes to webclaw are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/).

## [0.3.11] — 2026-04-10

### Added
- **Sitemap fallback paths**: discovery now tries `/sitemap_index.xml`, `/wp-sitemap.xml`, and `/sitemap/sitemap-index.xml` in addition to the standard `/sitemap.xml`. Sites using WordPress or non-standard sitemap locations are now discovered without needing external search.

---

## [0.3.10] — 2026-04-10

### Changed
- **Fetch timeout reduced from 30s to 12s**: prevents cascading slowdowns when proxies are unresponsive. Worst-case per-URL drops from ~94s to ~25s.
- **Retry attempts reduced from 3 to 2**: combined with shorter timeout, total worst-case is 12s + 1s delay + 12s = 25s instead of 30s + 1s + 30s + 3s + 30s = 94s.

---

## [0.3.9] — 2026-04-04

### Fixed
- **Layout tables rendered as sections**: tables used for page layout (containing block elements like `<p>`, `<div>`, `<hr>`) are now rendered as standalone sections instead of pipe-delimited markdown tables. Fixes Drudge Report and similar sites where all content was flattened into a single unreadable line. (by [@devnen](https://github.com/devnen) in #14)
- **Stack overflow on deeply nested HTML**: pages with 200+ DOM nesting levels (e.g., Express.co.uk live blogs) no longer overflow the stack. Two-layer fix: depth guard in markdown.rs falls back to iterator-based text collection at depth 200, and `extract_with_options()` spawns an 8 MB worker thread for safety on Windows. (by [@devnen](https://github.com/devnen) in #14)
- **Noise filter swallowing content in malformed HTML**: `<form>` tags no longer unconditionally treated as noise — ASP.NET page-wrapping forms (>500 chars) are preserved. Safety valve prevents unclosed noise containers (header/footer with >5000 chars) from absorbing entire page content. (by [@devnen](https://github.com/devnen) in #14)

### Changed
- **Bold/italic block passthrough**: `<b>`/`<strong>`/`<em>`/`<i>` tags containing block-level children (e.g., Drudge wrapping columns in `<b>`) now act as transparent containers instead of collapsing everything into inline bold/italic. (by [@devnen](https://github.com/devnen) in #14)

---

## [0.3.8] — 2026-04-03

### Fixed
- **MCP research token overflow**: research results are now saved to `~/.webclaw/research/` and the MCP tool returns file paths + findings instead of the full report. Prevents "exceeds maximum allowed tokens" errors in Claude/Cursor.
- **Research caching**: same query returns cached result instantly without spending credits.
- **Anthropic rate limit throttling**: 60s delay between LLM calls in research to stay under Tier 1 limits (50K input tokens/min).

### Added
- **`dirs` dependency** for `~/.webclaw/research/` path resolution.

---
## [0.3.7] — 2026-04-03

### Added
- **`--research` CLI flag**: run deep research via the cloud API. Prints report to stdout and saves full result (report + sources + findings) to a JSON file. Supports `--deep` for longer reports.
- **MCP extract/summarize cloud fallback**: when no local LLM is available, these tools now fall back to the cloud API instead of erroring. Set `WEBCLAW_API_KEY` for automatic fallback.
- **MCP research structured output**: the research tool now returns structured JSON (report + sources + findings + metadata) instead of raw text, so agents can reference individual findings and source URLs.

---

## [0.3.6] — 2026-04-02

### Added
- **Structured data in markdown/LLM output**: `__NEXT_DATA__`, SvelteKit, and JSON-LD data now appears as a `## Structured Data` section with a JSON code block at the end of `-f markdown` and `-f llm` output. Works with `--only-main-content` and all other flags.

### Fixed
- **Homebrew CI**: formula now updates all 4 platform checksums after Docker build completes, preventing SHA mismatch on Linux installs (#12).

---

## [0.3.5] — 2026-04-02

### Added
- **`__NEXT_DATA__` extraction**: Next.js pages now have their `pageProps` JSON extracted into `structured_data`. Contains prices, product info, page state, and other data that isn't in the visible HTML. Tested on 45 sites — 13 now return rich structured data (BBC, Forbes, Nike, Stripe, TripAdvisor, Glassdoor, NASA, etc.).

---

## [0.3.4] — 2026-04-01

### Added
- **SvelteKit data island extraction**: extracts structured JSON from `kit.start()` data arrays. Handles unquoted JS object keys by converting to valid JSON before parsing. Data appears in the `structured_data` field.

### Changed
- **License changed from MIT to AGPL-3.0**.

---

## [0.3.3] — 2026-04-01

### Changed
- **Replaced custom TLS stack with wreq**: migrated from webclaw-tls (patched rustls/h2/hyper/reqwest) to [wreq](https://github.com/0x676e67/wreq) by [@0x676e67](https://github.com/0x676e67). wreq uses BoringSSL for TLS and the [http2](https://github.com/0x676e67/http2) crate for HTTP/2 fingerprinting — both battle-tested with 60+ browser profiles.
- **Removed all `[patch.crates-io]` entries**: consumers no longer need to patch rustls, h2, hyper, hyper-util, or reqwest. Just depend on webclaw normally.
- **Browser profiles rebuilt on wreq's Emulation API**: Chrome 145, Firefox 135, Safari 18, Edge 145 with correct TLS options (cipher suites, curves, GREASE, ECH, PSK session resumption), HTTP/2 SETTINGS ordering, pseudo-header order, and header wire order.
- **Better TLS compatibility**: BoringSSL handles more server configurations than patched rustls (e.g. servers that previously returned IllegalParameter alerts).

### Removed
- webclaw-tls dependency and all 5 forked crates (webclaw-rustls, webclaw-h2, webclaw-hyper, webclaw-hyper-util, webclaw-reqwest).

### Acknowledgments
- TLS and HTTP/2 fingerprinting powered by [wreq](https://github.com/0x676e67/wreq) and [http2](https://github.com/0x676e67/http2) by [@0x676e67](https://github.com/0x676e67), who pioneered browser-grade HTTP/2 fingerprinting in Rust.

---

## [0.3.2] — 2026-03-31

### Added
- **`--cookie-file` flag**: load cookies from JSON files exported by browser extensions (EditThisCookie, Cookie-Editor). Format: `[{name, value, domain, ...}]`.
- **MCP `cookies` parameter**: the `scrape` tool now accepts a `cookies` array for authenticated scraping.
- **Combined cookies**: `--cookie` and `--cookie-file` can be used together and merge automatically.

---

## [0.3.1] — 2026-03-30

### Added
- **Cookie warmup fallback**: when a fetch returns an Akamai challenge page, automatically visits the homepage first to collect `_abck`/`bm_sz` cookies, then retries the original URL. Enables extraction of Akamai-protected subpages (e.g. fansale ticket pages) without JS rendering.

### Changed
- Fixed HTTP header wire order (accept/user-agent were in wrong positions) and added H2 PRIORITY flag in HEADERS frames.
- `FetchResult.headers` now uses `http::HeaderMap` instead of `HashMap<String, String>` — avoids per-response allocation, preserves multi-value headers.

## [0.3.0] — 2026-03-29

### Changed
- **Replaced primp with webclaw-tls**: switched to custom TLS fingerprinting stack.
- **Browser profiles**: Chrome 146 (Win/Mac), Firefox 135+, Safari 18, Edge 146 — captured from real browsers.
- **HTTP/2 fingerprinting**: SETTINGS frame ordering and pseudo-header ordering based on concepts pioneered by [@0x676e67](https://github.com/0x676e67).

### Fixed
- **HTTPS completely broken (#5)**: primp's forked rustls rejected valid certificates (UnknownIssuer on cross-signed chains like example.com). Fixed by using native OS root CAs alongside Mozilla bundle.
- **Unknown certificate extensions**: servers returning SCT in certificate entries no longer cause TLS errors.

### Added
- **Native root CA support**: uses OS trust store (macOS Keychain, Windows cert store) in addition to webpki-roots.
- **HTTP/2 fingerprinting**: SETTINGS frame ordering and pseudo-header ordering match real browsers.
- **Per-browser header ordering**: HTTP headers sent in browser-specific wire order.
- **Bandwidth tracking**: atomic byte counters shared across cloned clients.

---

## [0.2.2] — 2026-03-27

### Fixed
- **`cargo install` broken with primp 1.2.0**: added missing `reqwest` patch to `[patch.crates-io]`. primp moved to reqwest 0.13 which requires a patched fork.
- **Weekly dependency check**: CI now runs every Monday to catch primp patch drift before users hit it.

---

## [0.2.1] — 2026-03-27

### Added
- **Docker image on GHCR**: `docker run ghcr.io/0xmassi/webclaw` — auto-built on every release
- **QuickJS data island extraction**: inline `<script>` execution catches `window.__PRELOADED_STATE__`, Next.js hydration data, and other JS-embedded content

### Fixed
- Docker CI now runs as part of the release workflow (was missing, image was never published)

---

## [0.2.0] — 2026-03-26

### Added
- **DOCX extraction**: auto-detected by Content-Type or URL extension, outputs markdown with headings
- **XLSX/XLS extraction**: spreadsheets converted to markdown tables, multi-sheet support via calamine
- **CSV extraction**: parsed with quoted field handling, output as markdown table
- **HTML output format**: `-f html` returns sanitized HTML from the extracted content
- **Multi-URL watch**: `--watch` now works with `--urls-file` to monitor multiple URLs in parallel
- **Batch + LLM extraction**: `--extract-prompt` and `--extract-json` now work with multiple URLs
- **Scheduled batch watch**: watch multiple URLs with aggregate change reports and per-URL diffs

---

## [0.1.7] — 2026-03-26

### Fixed
- `--only-main-content`, `--include`, and `--exclude` now work in batch mode (#3)

---

## [0.1.6] — 2026-03-26

### Added
- `--watch`: monitor a URL for changes at a configurable interval with diff output
- `--watch-interval`: seconds between checks (default: 300)
- `--on-change`: run a command when changes are detected (diff JSON piped to stdin)
- `--webhook`: POST JSON notifications on crawl/batch complete and watch changes. Auto-formats for Discord and Slack webhooks

---

## [0.1.5] — 2026-03-26

### Added
- `--output-dir`: save each page to a separate file instead of stdout. Works with single URL, crawl, and batch modes
- CSV input with custom filenames: `url,filename` format in `--urls-file`
- Root URLs use `hostname/index.ext` to avoid collisions in batch mode
- Subdirectories created automatically from URL path structure

---

## [0.1.4] — 2026-03-26

### Added
- QuickJS integration for extracting data from inline JavaScript (NYTimes +168%, Wired +580% more content)
- Executes inline `<script>` tags in a sandboxed runtime to capture `window.__*` data blobs
- Parses Next.js RSC flight data (`self.__next_f`) for App Router sites
- Smart text filtering rejects CSS, base64, file paths, and code — only keeps readable prose
- Feature-gated with `quickjs` feature flag (enabled by default, disable for WASM builds)

---

## [0.1.3] — 2026-03-25

### Added
- Crawl streaming: real-time progress on stderr as pages complete (`[2/50] OK https://... (234ms, 1523 words)`)
- Crawl resume/cancel: `--crawl-state <path>` saves progress on Ctrl+C and resumes from where it left off
- MCP server proxy support via `WEBCLAW_PROXY` and `WEBCLAW_PROXY_FILE` env vars

### Changed
- Crawl results now expose visited set and remaining frontier for accurate state persistence

---

## [0.1.2] — 2026-03-25

### Changed
- Default TLS profile switched from Chrome145/Win to Safari26/Mac (highest pass rate across CF-protected sites)
- Plain client fallback: when impersonated TLS gets connection error or 403, automatically retries without impersonation (fixes ycombinator.com, producthunt.com, and similar sites)

### Fixed
- Reddit scraping: use plain HTTP client for `.json` endpoint (TLS fingerprinting was getting blocked)

### Added
- YouTube transcript extraction infrastructure in webclaw-core (caption track parsing, timed text XML parser) — wired up when cloud API launches

---

## [0.1.1] — 2026-03-24

### Fixed
- MCP server now identifies as `webclaw-mcp` instead of `rmcp` in the MCP handshake
- Research tool polling caps at 200 iterations (~10 min) instead of looping forever
- CLI returns non-zero exit codes on errors (invalid format, fetch failures, missing LLM)
- Text format output strips markdown table syntax (`| --- |` pipes)
- All MCP tools validate URLs before network calls with clear error messages
- Cloud API HTTP client has 60s timeout instead of no timeout
- Local fetch calls timeout after 30s to prevent hanging on slow servers
- Diff cloud fallback computes actual diff instead of returning raw scrape JSON
- FetchClient startup failure logs and exits gracefully instead of panicking

### Added
- Upper bounds: batch capped at 100 URLs, crawl capped at 500 pages

---

## [0.1.0] — 2026-03-18

First public release. Full-featured web content extraction toolkit for LLMs.

### Core Extraction
- Readability-style content scoring with text density, semantic tags, and link density penalties
- Exact CSS class token noise filtering with body-force fallback for SPAs
- HTML → markdown conversion with URL resolution, image alt text, srcset optimization
- 9-step LLM text optimization pipeline (67% token reduction vs raw HTML)
- JSON data island extraction (React, Next.js, Contentful CMS)
- YouTube transcript extraction (title, channel, views, duration, description)
- Lazy-loaded image detection (data-src, data-lazy-src, data-original)
- Brand identity extraction (name, colors, fonts, logos, OG image)
- Content change tracking / diff engine
- CSS selector filtering (include/exclude)

### Fetching & Crawling
- TLS fingerprint impersonation via Impit (Chrome 142, Firefox 144, random mode)
- BFS same-origin crawler with configurable depth, concurrency, and delay
- Sitemap.xml and robots.txt discovery
- Batch multi-URL concurrent extraction
- Per-request proxy rotation from pool file
- Reddit JSON API and LinkedIn post extractors

### LLM Integration
- Provider chain: Ollama (local-first) → OpenAI → Anthropic
- JSON schema extraction (structured data from pages)
- Natural language prompt extraction
- Page summarization with configurable sentence count

### PDF
- PDF text extraction via pdf-extract
- Auto-detection by Content-Type header

### MCP Server
- 8 tools: scrape, crawl, map, batch, extract, summarize, diff, brand
- stdio transport for Claude Desktop, Claude Code, and any MCP client
- Smart Fetch: local extraction first, cloud API fallback

### CLI
- 4 output formats: markdown, JSON, plain text, LLM-optimized
- CSS selector filtering, crawling, sitemap discovery
- Brand extraction, content diffing, LLM features
- Browser profile selection, proxy support, stdin/file input

### Infrastructure
- Docker multi-stage build with Ollama sidecar
- Deploy script for Hetzner VPS
