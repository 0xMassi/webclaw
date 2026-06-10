# Proxy-Backed Crawling

Use proxy rotation when you need to distribute a crawl across a proxy pool. webclaw supports a single proxy or a proxy file, and accepts any standard HTTP/HTTPS or SOCKS5 proxy URL.

## Using ColdProxy

[ColdProxy](https://coldproxy.com/) is webclaw's infrastructure partner, providing residential IPv4, residential IPv6, and datacenter IPv6 proxies across 195+ countries. Use a ColdProxy endpoint as a full URL with `--proxy` / `WEBCLAW_PROXY`, or list several in a `--proxy-file` pool.

### 1. Get your endpoint

Sign in to your [ColdProxy dashboard](https://coldproxy.com/) and copy your proxy host, port, and credentials. Assemble them into a standard proxy URL:

```text
http://USERNAME:PASSWORD@HOST:PORT
```

### 2. One ColdProxy endpoint

```bash
export WEBCLAW_PROXY="http://USERNAME:PASSWORD@HOST:PORT"
webclaw https://example.com --format markdown
```

Or pass it inline:

```bash
webclaw https://example.com \
  --proxy "http://USERNAME:PASSWORD@HOST:PORT" \
  --format markdown
```

### 3. Rotate a ColdProxy pool

List one ColdProxy endpoint per line in `coldproxy.txt`. Pool files use `host:port:user:pass` (one entry per line; lines starting with `#` are ignored). Mix product types and regions to match your workload:

```text
# residential IPv4
HOST:PORT:USERNAME:PASSWORD
# residential IPv6
HOST:PORT:USERNAME:PASSWORD
# datacenter IPv6
HOST:PORT:USERNAME:PASSWORD
```

webclaw rotates across the pool per request:

```bash
webclaw https://docs.example.com \
  --crawl \
  --depth 2 \
  --max-pages 200 \
  --concurrency 10 \
  --delay 200 \
  --proxy-file coldproxy.txt \
  --format markdown
```

### 4. Target a country

ColdProxy offers access across 195+ countries. Use the country-specific endpoint from your ColdProxy dashboard for each region you want to collect from (for example, a France residential endpoint for fr-localized pages). Add one endpoint per country to your pool file to spread a single crawl across regions.

### Choosing a product

- **Residential IPv4 / IPv6** — highest trust; best for consumer sites, geo-restricted content, and regional QA.
- **Datacenter IPv6** — fastest and most cost-effective; best for high-volume crawling of tolerant endpoints.

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

Create `proxies.txt` with one proxy per line in `host:port:user:pass` format (lines starting with `#` are ignored):

```text
proxy-1.example.com:8080:user:pass
proxy-2.example.com:8080:user:pass
proxy-3.example.com:8080:user:pass
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
