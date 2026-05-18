# Proxy-Backed Crawling

Use proxy rotation when you need to distribute a crawl across a proxy pool. webclaw supports a single proxy or a proxy file.

## Single Proxy

```bash
webclaw https://example.com \
  --proxy http://user:pass@proxy.example.com:8080 \
  --format markdown
```

SOCKS5 is supported too:

```bash
webclaw https://example.com \
  --proxy socks5://proxy.example.com:1080 \
  --format markdown
```

## Proxy Pool

Create `proxies.txt` with one proxy per line:

```text
http://user:pass@proxy-1.example.com:8080
http://user:pass@proxy-2.example.com:8080
http://user:pass@proxy-3.example.com:8080
```

Run a crawl with controlled concurrency:

```bash
webclaw https://docs.example.com \
  --crawl \
  --depth 2 \
  --max-pages 100 \
  --concurrency 10 \
  --delay 200 \
  --proxy-file proxies.txt \
  --format markdown
```

## Batch URLs

```bash
webclaw --urls-file urls.txt \
  --proxy-file proxies.txt \
  --concurrency 10 \
  --format json
```

Proxy rotation helps with throughput and IP reputation. It does not replace request fingerprinting, JS rendering, or challenge handling for heavily protected sites. For those, use hosted cloud mode with `WEBCLAW_API_KEY`.
