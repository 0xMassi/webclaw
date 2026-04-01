---
name: webclaw
description: Web extraction engine with antibot bypass. Scrape, crawl, extract, summarize, search, map, diff, monitor, research, and analyze any URL — including Cloudflare-protected sites. Use when you need reliable web content, the built-in web_fetch fails, or you need structured data extraction from web pages.
homepage: https://webclaw.io
user-invocable: true
metadata: {"openclaw":{"emoji":"🦀","requires":{"env":["WEBCLAW_API_KEY"]},"primaryEnv":"WEBCLAW_API_KEY","homepage":"https://webclaw.io","install":[{"id":"npx","kind":"node","bins":["webclaw-mcp"],"label":"npx create-webclaw"}]}}
---

# webclaw

High-quality web extraction with automatic antibot bypass. Beats Firecrawl on extraction quality and handles Cloudflare, DataDome, and JS-rendered pages automatically.

## When to use this skill

- **Always** when you need to fetch web content and want reliable results
- When `web_fetch` returns empty/blocked content (403, Cloudflare challenges)
- When you need structured data extraction (pricing tables, product info)
- When you need to crawl an entire site or discover all URLs
- When you need LLM-optimized content (cleaner than raw markdown)
- When you need to summarize a page without reading the full content
- When you need to detect content changes between visits
- When you need brand identity analysis (colors, fonts, logos)
- When you need web search results with optional page scraping
- When you need deep multi-source research on a topic
- When you need AI-guided scraping to accomplish a goal on a page
- When you need to monitor a URL for changes over time

## API base

All requests go to `https://api.webclaw.io/v1/`.

Authentication: `Authorization: Bearer $WEBCLAW_API_KEY`

## Endpoints

### 1. Scrape — extract content from a single URL

```bash
curl -X POST https://api.webclaw.io/v1/scrape \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com",
    "formats": ["markdown"],
    "only_main_content": true
  }'
```

**Request fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | required | URL to scrape |
| `formats` | string[] | `["markdown"]` | Output formats: `markdown`, `text`, `llm`, `json` |
| `include_selectors` | string[] | `[]` | CSS selectors to keep (e.g. `["article", ".content"]`) |
| `exclude_selectors` | string[] | `[]` | CSS selectors to remove (e.g. `["nav", "footer", ".ads"]`) |
| `only_main_content` | bool | `false` | Extract only the main article/content area |
| `no_cache` | bool | `false` | Skip cache, fetch fresh |
| `max_cache_age` | int | server default | Max acceptable cache age in seconds |

**Response:**

```json
{
  "url": "https://example.com",
  "metadata": {
    "title": "Example",
    "description": "...",
    "language": "en",
    "word_count": 1234
  },
  "markdown": "# Page Title\n\nContent here...",
  "cache": { "status": "miss" }
}
```

**Format options:**
- `markdown` — clean markdown, best for general use
- `text` — plain text without formatting
- `llm` — optimized for LLM consumption: includes page title, URL, and cleaned content with link references. Best for feeding to AI models.
- `json` — full extraction result with all metadata

**When antibot bypass activates** (automatic, no extra config):
```json
{
  "antibot": {
    "bypass": true,
    "elapsed_ms": 3200
  }
}
```

### 2. Crawl — scrape an entire website

Starts an async job. Poll for results.

**Start crawl:**
```bash
curl -X POST https://api.webclaw.io/v1/crawl \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://docs.example.com",
    "max_depth": 3,
    "max_pages": 50,
    "use_sitemap": true
  }'
```

Response: `{ "job_id": "abc-123", "status": "running" }`

**Poll status:**
```bash
curl https://api.webclaw.io/v1/crawl/abc-123 \
  -H "Authorization: Bearer $WEBCLAW_API_KEY"
```

Response when complete:
```json
{
  "job_id": "abc-123",
  "status": "completed",
  "total": 47,
  "completed": 45,
  "errors": 2,
  "pages": [
    {
      "url": "https://docs.example.com/intro",
      "markdown": "# Introduction\n...",
      "metadata": { "title": "Intro", "word_count": 500 }
    }
  ]
}
```

**Request fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | required | Starting URL |
| `max_depth` | int | `3` | How many links deep to follow |
| `max_pages` | int | `100` | Maximum pages to crawl |
| `use_sitemap` | bool | `false` | Seed URLs from sitemap.xml |
| `formats` | string[] | `["markdown"]` | Output formats per page |
| `include_selectors` | string[] | `[]` | CSS selectors to keep |
| `exclude_selectors` | string[] | `[]` | CSS selectors to remove |
| `only_main_content` | bool | `false` | Main content only |

### 3. Map — discover all URLs on a site

Fast URL discovery without full content extraction.

```bash
curl -X POST https://api.webclaw.io/v1/map \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://example.com"}'
```

Response:
```json
{
  "url": "https://example.com",
  "count": 142,
  "urls": [
    "https://example.com/about",
    "https://example.com/pricing",
    "https://example.com/docs/intro"
  ]
}
```

### 4. Batch — scrape multiple URLs in parallel

```bash
curl -X POST https://api.webclaw.io/v1/batch \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "urls": [
      "https://a.com",
      "https://b.com",
      "https://c.com"
    ],
    "formats": ["markdown"],
    "concurrency": 5
  }'
```

Response:
```json
{
  "total": 3,
  "completed": 3,
  "errors": 0,
  "results": [
    { "url": "https://a.com", "markdown": "...", "metadata": {} },
    { "url": "https://b.com", "markdown": "...", "metadata": {} },
    { "url": "https://c.com", "error": "timeout" }
  ]
}
```

### 5. Extract — LLM-powered structured extraction

Pull structured data from any page using a JSON schema or plain-text prompt.

**With JSON schema:**
```bash
curl -X POST https://api.webclaw.io/v1/extract \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/pricing",
    "schema": {
      "type": "object",
      "properties": {
        "plans": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "name": { "type": "string" },
              "price": { "type": "string" },
              "features": { "type": "array", "items": { "type": "string" } }
            }
          }
        }
      }
    }
  }'
```

**With prompt:**
```bash
curl -X POST https://api.webclaw.io/v1/extract \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/pricing",
    "prompt": "Extract all pricing tiers with names, monthly prices, and key features"
  }'
```

Response:
```json
{
  "url": "https://example.com/pricing",
  "data": {
    "plans": [
      { "name": "Starter", "price": "$49/mo", "features": ["10k pages", "Email support"] },
      { "name": "Pro", "price": "$99/mo", "features": ["100k pages", "Priority support", "API access"] }
    ]
  }
}
```

### 6. Summarize — get a quick summary of any page

```bash
curl -X POST https://api.webclaw.io/v1/summarize \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/long-article",
    "max_sentences": 3
  }'
```

Response:
```json
{
  "url": "https://example.com/long-article",
  "summary": "The article discusses... Key findings include... The author concludes that..."
}
```

### 7. Diff — detect content changes

Compare current page content against a previous snapshot.

```bash
curl -X POST https://api.webclaw.io/v1/diff \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com",
    "previous": {
      "markdown": "# Old content...",
      "metadata": { "title": "Old Title" }
    }
  }'
```

Response:
```json
{
  "url": "https://example.com",
  "status": "changed",
  "diff": "--- previous\n+++ current\n@@ -1 +1 @@\n-# Old content\n+# New content",
  "metadata_changes": [
    { "field": "title", "old": "Old Title", "new": "New Title" }
  ]
}
```

### 8. Brand — extract brand identity

Analyze a website's visual identity: colors, fonts, logo.

```bash
curl -X POST https://api.webclaw.io/v1/brand \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://example.com"}'
```

Response:
```json
{
  "url": "https://example.com",
  "brand": {
    "colors": [
      { "hex": "#FF6B35", "usage": "primary" },
      { "hex": "#1A1A2E", "usage": "background" }
    ],
    "fonts": ["Inter", "JetBrains Mono"],
    "logo_url": "https://example.com/logo.svg",
    "favicon_url": "https://example.com/favicon.ico"
  }
}
```

### 9. Search — web search with optional scraping

Search the web and optionally scrape each result page.

```bash
curl -X POST https://api.webclaw.io/v1/search \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "best rust web frameworks 2026",
    "num_results": 5,
    "scrape": true,
    "formats": ["markdown"]
  }'
```

**Request fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `query` | string | required | Search query |
| `num_results` | int | `10` | Number of search results to return |
| `scrape` | bool | `false` | Also scrape each result page for full content |
| `formats` | string[] | `["markdown"]` | Output formats when `scrape` is true |
| `country` | string | none | Country code for localized results (e.g. `"us"`, `"de"`) |
| `lang` | string | none | Language code for results (e.g. `"en"`, `"fr"`) |

**Response:**

```json
{
  "query": "best rust web frameworks 2026",
  "results": [
    {
      "title": "Top Rust Web Frameworks in 2026",
      "url": "https://blog.example.com/rust-frameworks",
      "snippet": "A comprehensive comparison of Axum, Actix, and Rocket...",
      "position": 1,
      "markdown": "# Top Rust Web Frameworks\n\n..."
    },
    {
      "title": "Choosing a Rust Backend Framework",
      "url": "https://dev.to/rust-backends",
      "snippet": "When starting a new Rust web project...",
      "position": 2,
      "markdown": "# Choosing a Rust Backend\n\n..."
    }
  ]
}
```

The `markdown` field on each result is only present when `scrape: true`. Without it, you get titles, URLs, snippets, and positions only.

### 10. Research — deep multi-source research

Starts an async research job that searches, scrapes, and synthesizes information across multiple sources. Poll for results.

**Start research:**
```bash
curl -X POST https://api.webclaw.io/v1/research \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "How does Cloudflare Turnstile work and what are its known bypass methods?",
    "max_iterations": 5,
    "max_sources": 10,
    "topic": "security",
    "deep": true
  }'
```

**Request fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `query` | string | required | Research question or topic |
| `max_iterations` | int | server default | Maximum research iterations (search-read-analyze cycles) |
| `max_sources` | int | server default | Maximum number of sources to consult |
| `topic` | string | none | Topic hint to guide search strategy (e.g. `"security"`, `"finance"`, `"engineering"`) |
| `deep` | bool | `false` | Enable deep research mode for more thorough analysis (costs 10 credits instead of 1) |

Response: `{ "id": "res-abc-123", "status": "running" }`

**Poll results:**
```bash
curl https://api.webclaw.io/v1/research/res-abc-123 \
  -H "Authorization: Bearer $WEBCLAW_API_KEY"
```

Response when complete:
```json
{
  "id": "res-abc-123",
  "status": "completed",
  "query": "How does Cloudflare Turnstile work and what are its known bypass methods?",
  "report": "# Cloudflare Turnstile Analysis\n\n## Overview\nCloudflare Turnstile is a CAPTCHA replacement that...\n\n## How It Works\n...\n\n## Known Bypass Methods\n...",
  "sources": [
    { "url": "https://developers.cloudflare.com/turnstile/", "title": "Turnstile Documentation" },
    { "url": "https://blog.cloudflare.com/turnstile-ga/", "title": "Turnstile GA Announcement" }
  ],
  "findings": [
    "Turnstile uses browser environment signals and proof-of-work challenges",
    "Managed mode auto-selects challenge difficulty based on visitor risk score",
    "Known bypass approaches include instrumented browser automation"
  ],
  "iterations": 5,
  "elapsed_ms": 34200
}
```

**Status values:** `running`, `completed`, `failed`

### 11. Agent Scrape — AI-guided scraping

Use an AI agent to navigate and interact with a page to accomplish a specific goal. The agent can click, scroll, fill forms, and extract data across multiple steps.

```bash
curl -X POST https://api.webclaw.io/v1/agent-scrape \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/products",
    "goal": "Find the cheapest laptop with at least 16GB RAM and extract its full specs",
    "max_steps": 10
  }'
```

**Request fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | required | Starting URL |
| `goal` | string | required | What the agent should accomplish |
| `max_steps` | int | server default | Maximum number of actions the agent can take |

**Response:**

```json
{
  "url": "https://example.com/products",
  "result": "The cheapest laptop with 16GB+ RAM is the ThinkPad E14 Gen 6 at $649. Specs: AMD Ryzen 5 7535U, 16GB DDR4, 512GB SSD, 14\" FHD IPS display, 57Wh battery.",
  "steps": [
    { "action": "navigate", "detail": "Loaded products page" },
    { "action": "click", "detail": "Clicked 'Laptops' category filter" },
    { "action": "click", "detail": "Applied '16GB+' RAM filter" },
    { "action": "click", "detail": "Sorted by price: low to high" },
    { "action": "extract", "detail": "Extracted specs from first matching product" }
  ]
}
```

### 12. Watch — monitor a URL for changes

Create persistent monitors that check a URL on a schedule and notify via webhook when content changes.

**Create a monitor:**
```bash
curl -X POST https://api.webclaw.io/v1/watch \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com/pricing",
    "interval": "0 */6 * * *",
    "webhook_url": "https://hooks.example.com/pricing-changed",
    "formats": ["markdown"]
  }'
```

**Request fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | required | URL to monitor |
| `interval` | string | required | Check frequency as cron expression or seconds (e.g. `"0 */6 * * *"` or `"3600"`) |
| `webhook_url` | string | none | URL to POST when changes are detected |
| `formats` | string[] | `["markdown"]` | Output formats for snapshots |

Response:
```json
{
  "id": "watch-abc-123",
  "url": "https://example.com/pricing",
  "interval": "0 */6 * * *",
  "webhook_url": "https://hooks.example.com/pricing-changed",
  "formats": ["markdown"],
  "created_at": "2026-03-20T10:00:00Z",
  "last_check": null,
  "status": "active"
}
```

**List all monitors:**
```bash
curl https://api.webclaw.io/v1/watch \
  -H "Authorization: Bearer $WEBCLAW_API_KEY"
```

Response:
```json
{
  "monitors": [
    {
      "id": "watch-abc-123",
      "url": "https://example.com/pricing",
      "interval": "0 */6 * * *",
      "status": "active",
      "last_check": "2026-03-20T16:00:00Z",
      "checks": 4
    }
  ]
}
```

**Get a monitor with snapshots:**
```bash
curl https://api.webclaw.io/v1/watch/watch-abc-123 \
  -H "Authorization: Bearer $WEBCLAW_API_KEY"
```

Response:
```json
{
  "id": "watch-abc-123",
  "url": "https://example.com/pricing",
  "interval": "0 */6 * * *",
  "status": "active",
  "snapshots": [
    {
      "checked_at": "2026-03-20T16:00:00Z",
      "status": "changed",
      "diff": "--- previous\n+++ current\n@@ -5 +5 @@\n-Pro: $99/mo\n+Pro: $119/mo"
    },
    {
      "checked_at": "2026-03-20T10:00:00Z",
      "status": "baseline"
    }
  ]
}
```

**Trigger an immediate check:**
```bash
curl -X POST https://api.webclaw.io/v1/watch/watch-abc-123/check \
  -H "Authorization: Bearer $WEBCLAW_API_KEY"
```

**Delete a monitor:**
```bash
curl -X DELETE https://api.webclaw.io/v1/watch/watch-abc-123 \
  -H "Authorization: Bearer $WEBCLAW_API_KEY"
```

## Choosing the right format

| Goal | Format | Why |
|------|--------|-----|
| Read and understand a page | `markdown` | Clean structure, headings, links preserved |
| Feed content to an AI model | `llm` | Optimized: includes title + URL header, clean link refs |
| Search or index content | `text` | Plain text, no formatting noise |
| Programmatic analysis | `json` | Full metadata, structured data, DOM statistics |

## Tips

- **Use `llm` format** when passing content to yourself or another AI — it's specifically optimized for LLM consumption with better context framing.
- **Use `only_main_content: true`** to skip navigation, sidebars, and footers. Reduces noise significantly.
- **Use `include_selectors`/`exclude_selectors`** for fine-grained control when `only_main_content` isn't enough.
- **Batch over individual scrapes** when fetching multiple URLs — it's faster and more efficient.
- **Use `map` before `crawl`** to discover the site structure first, then crawl specific sections.
- **Use `extract` with a JSON schema** for reliable structured output (e.g., pricing tables, product specs, contact info).
- **Antibot bypass is automatic** — no extra configuration needed. Works on Cloudflare, DataDome, AWS WAF, and JS-rendered SPAs.
- **Use `search` with `scrape: true`** to get full page content for each search result in one call instead of searching then scraping separately.
- **Use `research` for complex questions** that need multiple sources — it handles the search-read-synthesize loop automatically. Enable `deep: true` for thorough analysis.
- **Use `agent-scrape` for interactive pages** where data is behind filters, pagination, or form submissions that a simple scrape cannot reach.
- **Use `watch` for ongoing monitoring** — set up a cron schedule and a webhook to get notified when a page changes without polling manually.

## Smart Fetch Architecture

The webclaw MCP server uses a **local-first** approach:

1. **Local fetch** — fast, free, no API credits used (~80% of sites)
2. **Cloud API fallback** — automatic when bot protection or JS rendering is detected

This means:
- Most scrapes cost zero credits (local extraction)
- Cloudflare, DataDome, AWS WAF sites automatically fall back to the cloud API
- JS-rendered SPAs (React, Next.js, Vue) also fall back automatically
- Set `WEBCLAW_API_KEY` to enable cloud fallback

## vs web_fetch

| | webclaw | web_fetch |
|---|---------|-----------|
| Cloudflare bypass | Automatic (cloud fallback) | Fails (403) |
| JS-rendered pages | Automatic fallback | Readability only |
| Output quality | 20-step optimization pipeline | Basic HTML parsing |
| Structured extraction | LLM-powered, schema-based | None |
| Crawling | Full site crawl with sitemap | Single page only |
| Caching | Built-in, configurable TTL | Per-session |
| Rate limiting | Managed server-side | Client responsibility |

Use `web_fetch` for simple, fast lookups. Use webclaw when you need reliability, quality, or advanced features.
