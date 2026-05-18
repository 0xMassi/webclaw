# Firecrawl-Compatible API

webclaw exposes Firecrawl-compatible v2 routes for teams migrating existing scrape, crawl, map, or search calls.

## Scrape

```bash
curl https://api.webclaw.io/v2/scrape \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com",
    "formats": ["markdown"]
  }'
```

## Crawl

```bash
curl https://api.webclaw.io/v2/crawl \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://docs.example.com",
    "limit": 25,
    "maxDepth": 2
  }'
```

Poll the returned crawl id:

```bash
curl https://api.webclaw.io/v2/crawl/$CRAWL_ID \
  -H "Authorization: Bearer $WEBCLAW_API_KEY"
```

## Map

```bash
curl https://api.webclaw.io/v2/map \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://docs.example.com"
  }'
```

## Search

```bash
curl https://api.webclaw.io/v2/search \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "site:docs.rs tokio tutorial",
    "limit": 5
  }'
```

Compatibility routes are meant to reduce migration friction. For new projects, prefer the native `/v1` API because it exposes webclaw-specific options more directly.
