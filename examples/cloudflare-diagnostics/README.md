# Cloudflare Diagnostics

Use this checklist when a page works in the browser but fails from a scraper, returns a challenge page, or produces empty extracted content.

## 1. Save the Raw Response

```bash
webclaw https://protected.example.com --raw-html > raw.html
```

Inspect `raw.html` for challenge copy, blocked request text, empty shells, or application HTML that needs JavaScript rendering.

## 2. Compare Extracted Formats

```bash
webclaw https://protected.example.com --format markdown > page.md
webclaw https://protected.example.com --format json > page.json
webclaw https://protected.example.com --format llm > page.txt
```

If raw HTML has content but markdown is empty, tune extraction with selectors:

```bash
webclaw https://protected.example.com \
  --include "main, article, [role=main]" \
  --exclude "nav, footer, aside, .cookie-banner" \
  --format markdown
```

## 3. Try Another Browser Fingerprint

```bash
webclaw https://protected.example.com --browser firefox --format markdown
webclaw https://protected.example.com --browser random --format markdown
```

## 4. Use Cloud Fallback

```bash
export WEBCLAW_API_KEY=wc_your_key

webclaw https://protected.example.com --cloud --format markdown
```

Cloud mode can use hosted routing, JS rendering, and protected-site handling that are not part of the fully local open-source path.

## 5. Keep a Reproducible Report

When reporting a problem, include:

- target URL
- command used
- selected format
- whether `--raw-html` returned a challenge or normal page HTML
- whether `--browser firefox` changed the result
- whether cloud mode changed the result

Remove cookies, tokens, customer data, and private URLs before sharing logs.
