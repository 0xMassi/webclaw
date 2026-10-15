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

### Question-guided crawling

```bash
./benchmarks/query-crawl.sh    # webclaw --crawl --query against the other tools' breadth-first and best-first crawls
```

It asks three sets of questions, scored apart: the 60 in `query-crawl.jsonl` about docs.python.org, the PostgreSQL docs, MDN's HTTP section and quotes.toscrape.com tag pages, which the scoring weights were chosen on; 48 held-out questions in `query-crawl-held.jsonl` about kubernetes.io, the Django docs, sqlite.org and go.dev, written by a reviewer after the weights were fixed; and 16 questions in Japanese, French, Spanish and Chinese in `query-crawl-intl.jsonl` about the translated Python docs. Each question names the page that answers it, and the script first checks that those pages still hold the answer. Every arm crawls from the site's start page with a budget of 50 pages, depth 5, concurrency 4, the same path prefix and robots.txt obeyed, and the script records how many pages it fetched before the answer page, how long that took, and whether the answer page is in the top 3 of the tool's own ranking. The arms are webclaw breadth-first and with `--query`; Scrapling's spider, whose scheduler is a priority queue without any relevance scorer, so it runs breadth-first; and Crawl4AI breadth-first, best-first with its `KeywordRelevanceScorer` (keywords: the question's words of three letters or more, minus common English question words), and its adaptive crawler (statistical strategy, scoped to the same prefix), which stops when its own confidence is high enough and previews the `<head>` of linked pages on top of the pages it crawls. Results land in `results/<label>/query-crawl-summary.md`.

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

### Chunking for retrieval

```bash
./benchmarks/chunking.sh       # webclaw --chunk against Crawl4AI's chunkers and a plain split of Scrapling's markdown
```

It chunks the 69 pages frozen by `run.sh` at 256 and 512 tokens with each tool. Crawl4AI's `SlidingWindowChunking`, `OverlappingWindowChunking`, `RegexChunking` and its LLM extraction path (`RegexChunking` then `merge_chunks`) run on its own `raw_markdown` and again on webclaw's markdown, so the chunkers are also compared on the same text; their window sizes are words, the token target over Crawl4AI's own 1.3 tokens-per-word rate. Scrapling has no chunker, so its row cuts Scrapling's markdown every 4 characters per target token, the split a user would write. Two baselines cut webclaw's markdown into fixed windows of the real cl100k tokenizer, at the target and at the median size webclaw's chunks come out at. The script counts chunk sizes in cl100k tokens against the target, checks how many prose sentences and golden case facts sit whole inside one chunk and how many chunks end mid-sentence, ranks each page's chunks for the 40 questions in `questions.jsonl` with one BM25 scorer shared by every arm and checks the top 1, the top 3 and the ranked chunks that fit in 512 and 1024 tokens, and times each page from the frozen HTML. Results land in `results/<label>/chunking-summary.md`.

### Screenshots

```bash
./benchmarks/screenshot.sh     # webclaw --screenshot and the MCP screenshot tool against the other tools
```

It screenshots 10 public pages with each tool, the viewport and the whole page, first with a fresh browser per page and then with one browser kept open (webclaw and Scrapling through their MCP servers, Scrapling inside a browser session). It records the time, the image size and whether the image is blank, and writes `results/<label>/screenshot-summary.md`.

### PDFs

```bash
./benchmarks/pdf.sh            # webclaw --save-pdf and the MCP screenshot tool's pdf format against the other tools
```

It prints the 10 pages of the screenshot benchmark to PDF with each tool, first with a fresh browser per page and then with one browser kept open (webclaw through its MCP server). It records the time, the file size and the page count, reads each PDF's text back with webclaw's PDF reader, and scores the share of the page's content words, as webclaw reads the rendered page, that the PDF holds. Every arm prints US Letter with backgrounds; Crawl4AI's `pdf=True` prints with no margins, webclaw with Chrome's default margins. Scrapling has no PDF export, so it has no arm. Recall is a relative measure: webclaw's PDF reader drops some right-to-left text (the Arabic line on example.com, for one), which understates the absolute number for every tool alike; read with macOS PDFKit instead, the cold run scores 0.939 for webclaw and 0.934 for Crawl4AI, against 0.911 and 0.906 with webclaw's reader, so the comparison holds. It writes `results/<label>/pdf-summary.md`.

### Blocking resources while rendering

```bash
./benchmarks/render-block.sh   # webclaw --block against the other tools' resource blocking
```

It renders 20 script-heavy pages (the `spa` cases, eight JavaScript-built homepages from `facts.json`, Books to Scrape, BBC News and The Verge) with each tool in one browser kept open, 3 rounds, with its blocking off and on: webclaw's `block` list through the MCP `scrape` tool, Scrapling's `disable_resources` (images, fonts, media, stylesheets, beacons, websockets and a few more, by resource type), and Crawl4AI's `text_mode` (images, fonts, media, documents and archives by file extension, plus Chrome flags that turn images and remote fonts off). Every request goes through a local proxy that counts the bytes it carries, leaving out Chrome's own background downloads. It records the time, the bytes, the golden facts and the word count, lists the pages that lost content against the same tool with blocking off, and writes `results/<label>/render-block-summary.md`.

### Page actions

```bash
./benchmarks/actions.sh        # webclaw --actions against Scrapling's page_action and Crawl4AI's scroll, wait and script options
```

It runs nine pages that need interaction, each in a fresh headless browser, 2 rounds: infinite scroll (quotes.toscrape.com/scroll, the webscraper.io scroll shop, the Y Combinator directory filtered to one batch), "load more" buttons (the webscraper.io shop, a GitHub topic, NPR news), a pagination button that loads page 3 in place, content that appears after ten seconds, and a login form. `actions-cases.json` holds each page's steps, a CSS selector for its items, the ground truth, and how that truth was checked by hand (static copies of the same pages, the site's own counters, or the requests behind each button fetched over plain HTTP). Each tool gets the same steps. webclaw runs them with `--actions`. Scrapling has no scroll or click helpers, so its arm is a `page_action` that runs the steps through Playwright with a fixed pause after each one. Crawl4AI uses its own options: `scan_full_page` with its documented `scroll_delay=1.0` for scrolling, `wait_for` for waiting, C4A-Script for typing, key presses and single clicks, and a `js_code` loop for "load more" buttons, since C4A-Script's `REPEAT` takes a count fixed before the loop starts and cannot stop once the button is gone. That loop, like Scrapling's, waits up to 10 s for a visible button the first time and 2 s after, as webclaw's click step does, and Scrapling clicks with `force=True`. `webclaw-none` renders the same pages with no actions. The script counts the items each tool's HTML holds against the truth, times each run, and writes `results/<label>/actions-summary.md`.

On 2026-10-15 ([results/actions-2026-10-15/actions-summary.md](results/actions-2026-10-15/actions-summary.md)) webclaw got every item in 18 of 18 runs (median 8.1 s), Scrapling in 16 (12.0 s) and Crawl4AI in 16 (11.7 s); rendering with no actions got none. Scrapling and Crawl4AI both kept NPR's first 24 stories in both rounds. NPR shows its button only once its scripts have run and covers it with a cookie banner; in a separate review run Crawl4AI's waiting loop reached 48 to 72 stories, so its result there varies from run to run. With its documented `scroll_delay=1.0`, Crawl4AI's `scan_full_page` gets all 117 items of the webscraper.io scroll shop (12 to 15 with the default 0.2 s), and it is slower on every scroll page for it.

### Cloudflare challenges

```bash
./benchmarks/cloudflare.sh     # webclaw --solve against Scrapling's solve_cloudflare and Crawl4AI's stealth modes
```

It loads 16 pages from `cloudflare-cases.json`, each in a fresh browser with 60 s: the two ScrapingCourse challenge pages, four pages with an embedded Turnstile widget (Cloudflare's own Turnstile demo, the seleniumbase and two peet.ws test pages), and ten real sites that answer a plain HTTP client with a managed or non-interactive challenge (bscscan, basescan, the GitLab sign-in, Glassdoor, Crunchbase, Cardmarket, bizjournals, PriceCharting, BBB, Capterra). No public page with an interactive challenge was found that is not run by a captcha-solving business, so that kind is left out. webclaw runs over HTTP, with `--render always`, and with `--solve` (with `--render always` and an `executeJavascript` step that reads the token on the widget pages). Scrapling runs its `StealthyFetcher` with `solve_cloudflare=True`, once with its bundled Chromium and once with `real_chrome=True` (the installed Chrome). Crawl4AI runs headless with `enable_stealth`, its `UndetectedAdapter`, and `magic`, `simulate_user` and `override_navigator`; it has no challenge solver. A challenge page counts as solved when the output holds a text marker from the real page, checked to be absent from the challenge; Glassdoor redirects by country, so its marker is a company from its list or its "250 million" line in English or Italian. A widget counts when the page holds a token. `reuse` counts solved challenge pages that the tool's own plain HTTP client then loads in the same process, at the page's URL with `?again=1`, with the browser's cookies and User-Agent: webclaw as the second URL of one `--solve` run, Scrapling with its `Fetcher`. Outputs go to `.work/cloudflare-out/<label>/`, and the script writes `results/<label>/cloudflare-summary.md`.

On 2026-10-15 ([results/cloudflare-2026-10-15/cloudflare-summary.md](results/cloudflare-2026-10-15/cloudflare-summary.md)) webclaw `--solve` got 15 of 16 pages (median 4.2 s), Scrapling 14 (3.7 s), Scrapling with `real_chrome=True` 5 and Crawl4AI 2; webclaw over HTTP or rendering without `--solve` got none. webclaw's miss is Cloudflare's Turnstile demo, a login form with almost no text: the widget gives its token, but webclaw refuses the page as having no readable content, so the command fails. Crawl4AI got that page, Scrapling returned it before the token came. Cookie reuse is where webclaw is behind: 5 of 12 solved challenge pages loaded again over webclaw's HTTP client, against 9 of 12 with Scrapling's. Keeping only the `cf_clearance` cookie lost none of webclaw's reused sites against keeping every cookie. The cause of the gap is open; earlier runs that handed Scrapling's cookies to a separate webclaw HTTP run passed on Crunchbase, Cardmarket, bizjournals and BBB, where webclaw's own cookies fail, so it points at the browser webclaw launches (its request interception or launch flags) rather than its HTTP client.

### MCP over HTTP

```bash
./benchmarks/mcp-http.sh       # webclaw-mcp and the other MCP server over streamable HTTP
```

It starts each server on loopback with a bearer token, drives it with the official MCP Python client (handshake, then one fetch-and-extract call per page), checks that a client without the token is refused, and writes `results/<label>/mcp-http-summary.md`.

### Python package

```bash
./benchmarks/python.sh         # the webclaw-cli wheel against Scrapling's Fetcher and Crawl4AI's AsyncWebCrawler
./benchmarks/python.sh my-run ./path/to/webclaw_cli.whl
```

It builds the wheel from `python/` (or takes the one you pass) and saves each tool's wheels in a local folder. It installs each tool from that folder with no network and an empty uv cache into a fresh venv, three times, then runs its documented post-install step (`scrapling install`, `crawl4ai-setup`) into a fresh browser directory. It records the download size, the install time, the number of installed packages and the size on disk, with a Scrapling row without its browser download, since its plain `Fetcher` does not need it. webclaw renders with a Chrome already on the machine, which is not counted. Then it calls each tool through its Python API on the static golden pages (`cases.jsonl` without `spa` and `pdf`): a first call in a new Python process (import plus one scrape, on 5 pages), then one process calling every page twice. It scores the golden facts the same way as `run.sh`, samples the peak RSS of the Python process and all its children, lists every failed call with its error, and writes `results/<label>/python-summary.md`.

webclaw raises an error when a page answers with an HTTP error status, and that call counts as failed. In the 2026-10-14 runs, most of webclaw's lower fact recall came from sites that refused webclaw but served Scrapling a real page: hashicorp.com answered webclaw with 429 and ft.com with 403 on some calls, while Scrapling got 200 from both. The rest came from a page that is gone (simonwillison.net, 404 for every tool): webclaw raises, Scrapling returns the 404 page, which still holds two of the case's facts. The economist.com pages answered 403 to every tool. Correction: an earlier version of this section said the failures were error pages whose body Scrapling returned; that was true only of the 404 page.

## Methodology

See [methodology.md](methodology.md) for each tool's exact call, the url and html modes, how facts are matched, the metrics and the limits of this setup.

## Raw data

Each published run commits `results/<label>/rows.jsonl` (one row per tool, case, mode and round), `summary.md` and `versions.txt`. `results/2026-04-17.json` is the earlier 18-site run against webclaw v0.3.18, kept for history.
