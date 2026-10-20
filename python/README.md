# webclaw-cli for Python

Extract web pages for LLMs from Python, on your own machine. The wheel ships the `webclaw` binary, and the `webclaw_cli` module runs it and returns Python objects. No API key, no server, no Python dependencies.

For the hosted API, use the [`webclaw`](https://pypi.org/project/webclaw/) package instead. The two install side by side.

## Install

The package is not on PyPI yet. Build the wheel from this directory (needs a Rust toolchain and [uv](https://docs.astral.sh/uv/)):

```bash
cd python
uv build --wheel
pip install dist/webclaw_cli-*.whl
```

The wheel also puts `webclaw` on the `PATH` of the environment it is installed in, so the command line tool works too.

## Use

```python
import webclaw_cli as wc

md = wc.scrape("https://example.com")                   # markdown
text = wc.scrape("https://example.com", "llm")          # compact text for an LLM
page = wc.scrape("https://example.com", "json")         # dict: metadata, content, links, ...
answer = wc.scrape("https://docs.python.org/3/library/asyncio.html", "llm", query="how do I cancel a task")

titles = wc.select("https://books.toscrape.com", css="h3 a::attr(title)")
prices = wc.select("https://books.toscrape.com", xpath="//p[@class='price_color']")
books = wc.select("https://books.toscrape.com", css="article.product_pod",
                  fields={"title": "h3 a::attr(title)", "price": ".price_color"})

rows = wc.tables("https://en.wikipedia.org/wiki/List_of_largest_cities")
hits = wc.find_text("https://books.toscrape.com", "£51.77")        # [{"selector", "text"}]
like = wc.similar("https://books.toscrape.com", text="£51.77")     # {"selector", "matches"}

for page in wc.crawl("https://quotes.toscrape.com", max_pages=10):
    if page["error"]:
        print(page["url"], "failed:", page["error"])
        continue
    print(page["url"], page["extraction"]["metadata"]["word_count"])

urls = wc.map("https://www.rust-lang.org")
wc.screenshot("https://example.com", "example.png", full_page=True)
```

Every function that takes a page also takes an HTML string: a string that starts with a tag, such as `<p>` or `<!doctype html>`, is parsed as HTML instead of fetched. Anything else, `<https://...>` included, is taken as a URL.

```python
wc.select("<ul><li>a</li><li>b</li></ul>", css="li")    # ["a", "b"]
```

`crawl` yields each page as it finishes. Breaking out of the loop stops the crawl. With `query`, it fetches the links most likely to answer the question first and gives each page a `relevance` from 0 to 1:

```python
for page in wc.crawl("https://docs.python.org/3/", query="how do I cancel an asyncio task", max_pages=20):
    print(page["relevance"], page["url"])
```

## Options

Every function takes these keyword arguments:

| Option | Meaning |
|---|---|
| `proxy` | Proxy URL, `http://user:pass@host:port` |
| `browser` | Browser to impersonate: `chrome` (default), `chrome-macos`, `firefox`, `safari`, `edge`, `safari-ios`, `random` |
| `headers` | Extra request headers, a dict |
| `cookies` | A `Cookie` header string, or a dict |
| `timeout` | Seconds the binary allows each request (default 30). A crawl makes many requests, so this does not bound the whole call |
| `time_limit` | Seconds for the whole call. When they run out, the binary and any browser it started are stopped and `TimeoutError` is raised |
| `render` | `auto` (default) renders in a local Chrome only the pages that need JavaScript; `always`, `never` |
| `cache` | Keep fetched pages on disk and reuse them (in `WEBCLAW_CACHE_DIR` or `~/.cache/webclaw`) |

Rendering and screenshots use Chrome or Chromium already installed on your machine. Nothing is downloaded.

## Differences from the hosted SDK

The shared names match the [`webclaw`](https://pypi.org/project/webclaw/) package: `include_selectors`, `exclude_selectors`, `only_main_content` and `query` on `scrape`; `max_depth` (default 2), `max_pages` (default 50), `include_patterns`, `exclude_patterns` and `use_sitemap` on `crawl`; `limit` on `map`; `status_code` on `WebclawError`. What differs comes from running locally:

- Functions, not a client: no API key and no base URL.
- `scrape` takes one `format` and returns that text (a dict for `json`), not a response object with several formats.
- `crawl` yields each page as a dict while it runs, instead of returning a job to poll.
- `map` returns a list of URLs.
- `select`, `tables`, `find_text`, `similar` and `screenshot` run locally; there is no `extract`, `summarize`, `search`, `batch` or `research`, which need the hosted service.
- `proxy`, `browser`, `headers`, `cookies`, `timeout`, `time_limit`, `render` and `cache` control the local fetch.
- `WebclawError` also carries the binary's `stderr` and `returncode`.

## Errors

When the binary fails, the function raises `webclaw_cli.WebclawError`. Its message is the error the binary printed, and it carries the full `stderr`, the `returncode` and, when a page answered with an HTTP error, its `status_code`. Warnings the binary prints on a run that succeeded, such as a page with no content, come as `webclaw_cli.WebclawWarning`.

```python
try:
    wc.scrape("https://example.com/missing")
except wc.WebclawError as e:
    print(e, e.status_code)
```

## Advanced

`wc.binary_path()` returns the path of the bundled binary, for flags this module does not wrap:

```python
import subprocess
subprocess.run([wc.binary_path(), "https://example.com", "--brand"], check=True)
```

Each call starts the binary once. To call it from async code, run it in a thread:

```python
md = await asyncio.to_thread(wc.scrape, "https://example.com")
```

The binary runs in its own process group, so Ctrl-C reaches only Python. A call running in a thread, such as through `asyncio.to_thread`, therefore keeps going after Ctrl-C until it finishes; pass `time_limit` to bound it.

## Tests

```bash
python -m unittest discover -s tests -v                       # offline, on HTML strings
WEBCLAW_REAL_SITES=1 python -m unittest discover -s tests -v  # also against public sites
```

The public-site tests include rendering and screenshots, so they need Chrome or Chromium.
