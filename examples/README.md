# Examples

Practical examples showing what webclaw can do. Each example is a self-contained command you can run immediately.

## Workflow Guides

- [HTML to Markdown for RAG](./html-to-markdown-rag/) turns web pages into markdown or compact LLM text for retrieval pipelines.
- [Firecrawl-Compatible API](./firecrawl-compatible-api/) shows the `/v2` compatibility routes for scrape, crawl, map, and search.
- [MCP Web Scraping](./mcp-web-scraping/) connects webclaw to MCP clients such as Claude Code, Claude Desktop, Cursor, and Codex CLI.
- [Proxy-Backed Crawling](./proxy-backed-crawling/) shows single-proxy and proxy-pool crawling from the CLI.
- [Cloudflare Diagnostics](./cloudflare-diagnostics/) gives a reproducible checklist for blocked or empty protected-site results.

## Basic Extraction

```bash
# Extract as markdown (default)
webclaw https://example.com

# Multiple output formats
webclaw https://example.com -f markdown    # Clean markdown
webclaw https://example.com -f json        # Full structured JSON
webclaw https://example.com -f text        # Plain text (no formatting)
webclaw https://example.com -f llm         # Token-optimized for LLMs (67% fewer tokens)

# Bare domains work (auto-prepends https://)
webclaw example.com
```

## Content Filtering

```bash
# Only extract main content (skip nav, sidebar, footer)
webclaw https://docs.rs/tokio --only-main-content

# Include specific CSS selectors
webclaw https://news.ycombinator.com --include ".titleline,.score"

# Exclude specific elements
webclaw https://example.com --exclude "nav,footer,.ads,.sidebar"

# Combine both
webclaw https://docs.rs/reqwest --only-main-content --exclude ".sidebar"
```

## Brand Identity Extraction

```bash
# Extract colors, fonts, logos from any website
webclaw --brand https://stripe.com
# Output: { "name": "Stripe", "colors": [...], "fonts": ["Sohne"], "logos": [...] }

webclaw --brand https://github.com
# Output: { "name": "GitHub", "colors": [{"hex": "#1F2328", ...}], "fonts": ["Mona Sans"], ... }

webclaw --brand wikipedia.org
# Output: 10 colors, 5 fonts, favicon, logo URL
```

## Sitemap Discovery

```bash
# Discover all URLs from a site's sitemaps
webclaw --map https://sitemaps.org
# Output: one URL per line (84 URLs found)

# JSON output with metadata
webclaw --map https://sitemaps.org -f json
# Output: [{ "url": "...", "last_modified": "...", "priority": 0.8 }]
```

## Recursive Crawling

```bash
# Crawl a site (default: depth 1, max 20 pages)
webclaw --crawl https://example.com

# Control depth and page limit
webclaw --crawl --depth 2 --max-pages 50 https://docs.rs/tokio

# Crawl with sitemap seeding (finds more pages)
webclaw --crawl --sitemap --depth 2 https://docs.rs/tokio

# Filter crawl paths
webclaw --crawl --include-paths "/api/*,/guide/*" https://docs.example.com
webclaw --crawl --exclude-paths "/changelog/*,/blog/*" https://docs.example.com

# Control concurrency and delay
webclaw --crawl --concurrency 10 --delay 200 https://example.com
```

## Change Detection (Diff)

```bash
# Step 1: Save a snapshot
webclaw https://example.com -f json > snapshot.json

# Step 2: Later, compare against the snapshot
webclaw --diff-with snapshot.json https://example.com
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

## PDF Extraction

```bash
# PDF URLs are auto-detected via Content-Type
webclaw https://example.com/report.pdf

# Control PDF mode
webclaw --pdf-mode auto https://example.com/report.pdf  # Error on empty (catches scanned PDFs)
webclaw --pdf-mode fast https://example.com/report.pdf  # Return whatever text is found
```

## Batch Processing

```bash
# Multiple URLs in one command
webclaw https://example.com https://httpbin.org/html https://rust-lang.org

# URLs from a file (one per line, # comments supported)
webclaw --urls-file urls.txt

# Batch with JSON output
webclaw --urls-file urls.txt -f json

# Proxy rotation for large batches
webclaw --urls-file urls.txt --proxy-file proxies.txt --concurrency 10
```

## Local Files & Stdin

```bash
# Extract from a local HTML file
webclaw --file page.html

# Pipe HTML from another command
curl -s https://example.com | webclaw --stdin

# Chain with other tools
webclaw https://example.com -f text | wc -w    # Word count
webclaw https://example.com -f json | jq '.metadata.title'  # Extract title with jq
```

## Cloud API Mode

When you have a webclaw API key, the CLI can route through the cloud for bot protection bypass, JS rendering, and proxy rotation.

```bash
# Set API key (one time)
export WEBCLAW_API_KEY=wc_your_key_here

# Automatic fallback: tries local first, cloud on bot detection
webclaw https://protected-site.com

# Force cloud mode (skip local, always use API)
webclaw --cloud https://spa-site.com

# Cloud mode works with all features
webclaw --cloud --brand https://stripe.com
webclaw --cloud -f json https://producthunt.com
webclaw --cloud --crawl --depth 2 https://protected-docs.com
```

## Browser Impersonation

```bash
# Chrome (default) — latest Chrome TLS fingerprint
webclaw https://example.com

# Firefox fingerprint
webclaw --browser firefox https://example.com

# Random browser per request (good for batch)
webclaw --browser random --urls-file urls.txt
```

## Custom Headers & Cookies

```bash
# Custom headers
webclaw -H "Authorization: Bearer token123" https://api.example.com
webclaw -H "Accept-Language: de-DE" https://example.com

# Cookies
webclaw --cookie "session=abc123; theme=dark" https://example.com

# Multiple headers
webclaw -H "X-Custom: value" -H "Authorization: Bearer token" https://example.com
```

## LLM-Powered Features

These require an LLM provider (Ollama local, or OpenAI/Anthropic API key).

```bash
# Summarize a page (default: 3 sentences)
webclaw --summarize https://example.com

# Control summary length
webclaw --summarize 5 https://example.com

# Extract structured JSON with a schema
webclaw --extract-json '{"type":"object","properties":{"title":{"type":"string"},"price":{"type":"number"}}}' https://example.com/product

# Extract with a schema from file
webclaw --extract-json @schema.json https://example.com/product

# Extract with natural language prompt
webclaw --extract-prompt "Get all pricing tiers with name, price, and features" https://stripe.com/pricing

# Use a specific LLM provider
webclaw --llm-provider ollama --summarize https://example.com
webclaw --llm-provider openai --llm-model gpt-4o --extract-prompt "..." https://example.com
webclaw --llm-provider anthropic --summarize https://example.com
```

## Raw HTML Output

```bash
# Get the raw fetched HTML (no extraction)
webclaw --raw-html https://example.com

# Useful for debugging extraction issues
webclaw --raw-html https://example.com > raw.html
webclaw --file raw.html  # Then extract locally
```

## Metadata & Verbose Mode

```bash
# Include YAML frontmatter with metadata
webclaw --metadata https://example.com
# Output:
#   ---
#   title: "Example Domain"
#   source: "https://example.com"
#   word_count: 20
#   ---
#   # Example Domain
#   ...

# Verbose logging (debug extraction pipeline)
webclaw -v https://example.com
```

## Proxy Usage

```bash
# Single proxy
webclaw --proxy http://user:pass@proxy.example.com:8080 https://example.com

# SOCKS5 proxy
webclaw --proxy socks5://proxy.example.com:1080 https://example.com

# Proxy rotation from file (one per line: host:port:user:pass)
webclaw --proxy-file proxies.txt https://example.com

# Auto-load proxies.txt from current directory
echo "proxy1.com:8080:user:pass" > proxies.txt
webclaw https://example.com  # Automatically detects and uses proxies.txt
```

## MCP Server (AI Agent Integration)

```bash
# Start the MCP server (stdio transport)
webclaw-mcp

# Configure in Claude Desktop (~/.config/claude/claude_desktop_config.json):
# {
#   "mcpServers": {
#     "webclaw": {
#       "command": "/path/to/webclaw-mcp",
#       "env": {
#         "WEBCLAW_API_KEY": "wc_your_key"  // optional, enables cloud fallback
#       }
#     }
#   }
# }

# Available tools: scrape, crawl, map, batch, extract, summarize, diff, brand, research, search
```

## Real-World Recipes

### Monitor competitor pricing

```bash
# Save today's pricing
webclaw --extract-json '{"type":"array","items":{"type":"object","properties":{"plan":{"type":"string"},"price":{"type":"string"}}}}' \
  https://competitor.com/pricing -f json > pricing-$(date +%Y%m%d).json
```

### Build a documentation search index

```bash
# Crawl docs and extract as LLM-optimized text
webclaw --crawl --sitemap --depth 3 --max-pages 500 -f llm https://docs.example.com > docs.txt
```

### Extract all images from a page

```bash
webclaw https://example.com -f json | jq -r '.content.images[].src'
```

### Get all external links

```bash
webclaw https://example.com -f json | jq -r '.content.links[] | select(.href | startswith("http")) | .href'
```

### Compare two pages

```bash
webclaw https://site-a.com -f json > a.json
webclaw https://site-b.com --diff-with a.json
```
