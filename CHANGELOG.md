# Changelog

All notable changes to webclaw are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/).

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
