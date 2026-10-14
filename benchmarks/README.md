# Benchmarks

Reproducible benchmarks comparing `webclaw` against open-source web extraction
tools. Every number here ships with the script that produced it, and one
command reruns the whole thing on your machine.

## Results

Run on 2026-10-12 with webclaw v0.6.24, from one machine, 78 public URLs, 3 live rounds per URL. Full tables, including every category: [results/2026-10-12-v0.6.24/summary.md](results/2026-10-12-v0.6.24/summary.md).

Each tool fetching the page itself (url mode, 234 runs per tool):

| Tool | ok % | Fact recall % | Median tokens | p50 s | p95 s |
|---|---:|---:|---:|---:|---:|
| **webclaw `-f llm`** | 91 | 75.9 | 2,761 | 0.25 | 0.62 |
| webclaw `-f markdown` | 91 | 74.3 | 4,267 | 0.23 | 0.58 |
| Crawl4AI 0.9.4, `raw_markdown` | 85 | 78.1 | 8,448 | 0.42 | 1.06 |
| Crawl4AI 0.9.4, `fit_markdown` | 84 | 70.0 | 1,799 | 0.45 | 2.09 |
| Scrapling 0.4.15, `Fetcher` | 91 | 74.3 | 4,825 | 0.29 | 0.72 |
| Trafilatura 2.3.1 | 74 | 51.4 | 1,066 | 0.13 | 0.46 |

What the numbers say:

- webclaw `-f llm` keeps about as many facts as the raw markdown of Crawl4AI and Scrapling with a third to a half of their tokens, and it has the lowest p95 latency of the tools with comparable recall.
- webclaw is the only tool here that extracts PDFs: 100% ok and 94% fact recall on 9 PDFs. The others return nothing or fail on most of them.
- Pages that render their content with JavaScript are where webclaw loses. On the 9 `spa` cases Crawl4AI, which drives a headless browser, keeps 100% of facts; webclaw keeps 22%.

## Reproducing this benchmark

```bash
./benchmarks/run.sh            # builds webclaw, installs the other tools, runs everything
./benchmarks/run.sh my-run ./path/to/webclaw 3
```

You need [`uv`](https://docs.astral.sh/uv/) and a Rust toolchain. The script installs each tool into its own pinned environment under `benchmarks/.work/`, so nothing touches your system Python. The first run also saves each page's HTML into `.work/corpus/` for the html-mode comparison. That HTML belongs to the sites, so it never leaves your machine.

### CSS selectors and XPath

```bash
./benchmarks/select.sh         # same tools, records picked with each tool's CSS selector API and its XPath API
```

It freezes 50 books.toscrape.com catalogue pages into `.work/select-corpus/`, asks each tool for one record per book (title, price, rating) through CSS selectors and again through XPath, checks every record against the values in the raw HTML, and times the run. It also repeats the nested-divs test from Scrapling's own benchmark script. Results land in `results/<label>/select-summary.md`.

### Find by text and find similar

```bash
./benchmarks/similar.sh        # webclaw --find-text/--similar against Scrapling's find_by_text/find_similar
```

It freezes 43 list pages into `.work/similar-corpus/`: 23 pages the widening rule was developed on (books, quotes, Hacker News, GitHub trending, Wikipedia lists, shop category pages, news indexes) and 20 held-out pages picked and scored by a separate review (package indexes, docs, forums, job boards, tables, European and Asian news sites), reported apart so the result is not scored only on the pages it was tuned on. A hand-written CSS selector per page picks the ground-truth items. Each tool searches for the second item's text, then asks for every element like the one it found, and again starting from the first ground-truth item. It scores precision and recall against the ground truth, checks that webclaw's suggested selector picks the same elements again, and times the run. Crawl4AI has no equivalent (its schema generator needs an LLM call), so its row says so. Results land in `results/<label>/similar-summary.md`.

### Data tables

```bash
./benchmarks/tables.sh         # webclaw --tables against Crawl4AI's result.tables and a CSS loop in Scrapling
```

It freezes 35 public pages into `.work/tables-corpus/`: Wikipedia lists and sports pages, government statistics, sports results, finance tables, docs with option tables, and old sites built from layout tables. It also freezes 19 held-out pages into `.work/tables-held/`, chosen by a reviewer; one rule (the legend row) was fixed after seeing them. Each table on those pages is labelled by hand in `tables-labels.json` and `tables-held-labels.json` as a data table, a layout table, or neither (infoboxes and key/value boxes). The truth for every table is what headless Chrome shows: the frozen page loads with scripts off and stylesheets on, and each visible cell's `innerText` is laid out on a grid by its spans. That truth is saved once in `.work/tables-truth-*.json` and reused; delete those files after re-freezing a page, or the old truth scores the new page. The corpus is also scored against the raw HTML laid out with lxml. The script scores which tables each tool returns and how many cells it gets right. Crawl4AI returns tables from its default scraping pass (`DefaultTableExtraction`). Scrapling has no table API, so its arm is the loop a Scrapling user writes: every `table`, its `tr` rows and their `th, td` text, first row as headers. Results land in `results/<label>/tables-summary.md`.

### Question-focused extraction

```bash
./benchmarks/query.sh          # webclaw --query against the other tools' question filters
```

It asks the 40 questions in `questions.jsonl` about pages frozen by `run.sh`, and scores how many answers survive each tool's question filter and at what token cost. A control row cuts each tool's full output to the size its filter returned, so a filter has to beat keeping the top of the page. Results land in `results/<label>/query-summary.md`.

### Browser profiles

```bash
./benchmarks/profiles.sh       # every webclaw --browser profile against the matching impersonation of another tool
```

It fetches the non-PDF url-mode cases once per browser identity over HTTP only, scores each page on its case facts, and writes `results/<label>/profiles-summary.md`.

### Response cache

```bash
./benchmarks/cache.sh          # webclaw --cache against the other tools' disk caches
```

It fetches the url-mode cases twice through each tool's cache, starting empty, and sends the second run to a proxy that refuses connections, so only cached pages come back. It writes `results/<label>/cache-summary.md` with both run times and how many pages the cache served unchanged.

### Screenshots

```bash
./benchmarks/screenshot.sh     # webclaw --screenshot and the MCP screenshot tool against the other tools
```

It screenshots 10 public pages with each tool, the viewport and the whole page, first with a fresh browser per page and then with one browser kept open (webclaw and Scrapling through their MCP servers, Scrapling inside a browser session). It records the time, the image size and whether the image is blank, and writes `results/<label>/screenshot-summary.md`.

### Blocking resources while rendering

```bash
./benchmarks/render-block.sh   # webclaw --block against the other tools' resource blocking
```

It renders 20 script-heavy pages (the `spa` cases, eight JavaScript-built homepages from `facts.json`, Books to Scrape, BBC News and The Verge) with each tool in one browser kept open, 3 rounds, with its blocking off and on: webclaw's `block` list through the MCP `scrape` tool, Scrapling's `disable_resources` (images, fonts, media, stylesheets, beacons, websockets and a few more, by resource type), and Crawl4AI's `text_mode` (images, fonts, media, documents and archives by file extension, plus Chrome flags that turn images and remote fonts off). Every request goes through a local proxy that counts the bytes it carries, leaving out Chrome's own background downloads. It records the time, the bytes, the golden facts and the word count, lists the pages that lost content against the same tool with blocking off, and writes `results/<label>/render-block-summary.md`.

### MCP over HTTP

```bash
./benchmarks/mcp-http.sh       # webclaw-mcp and the other MCP server over streamable HTTP
```

It starts each server on loopback with a bearer token, drives it with the official MCP Python client (handshake, then one fetch-and-extract call per page), checks that a client without the token is refused, and writes `results/<label>/mcp-http-summary.md`.

## Methodology

See [methodology.md](methodology.md) for each tool's exact call, the url and html modes, how facts are matched, the metrics and the limits of this setup.

## Raw data

Each published run commits `results/<label>/rows.jsonl` (one row per tool, case, mode and round), `summary.md` and `versions.txt`. `results/2026-04-17.json` is the earlier 18-site run against webclaw v0.3.18, kept for history.
