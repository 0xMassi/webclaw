<p align="center"><strong>English</strong> | <a href="./README_zh-CN.md">简体中文</a></p>

<p align="center">
  <a href="https://webclaw.io">
    <img src=".github/banner.png" alt="webclaw" width="760" />
  </a>
</p>

<h1 align="center">webclaw</h1>

<p align="center">
  <strong>Turn websites into clean markdown, JSON, and LLM-ready context.</strong><br/>
  <sub>CLI, MCP server, REST API, and SDKs for AI agents and RAG pipelines.</sub>
</p>

<p align="center">
  <a href="https://github.com/0xMassi/webclaw/stargazers"><img src="https://shieldcn.dev/github/stars/0xMassi/webclaw.svg?variant=branded&logo=github" alt="Stars" /></a>
  <a href="https://github.com/0xMassi/webclaw/releases"><img src="https://shieldcn.dev/github/tag/0xMassi/webclaw.svg?variant=branded&logo=rust" alt="Version" /></a>
  <a href="https://github.com/0xMassi/webclaw/blob/main/LICENSE"><img src="https://shieldcn.dev/github/license/0xMassi/webclaw.svg?variant=branded" alt="License" /></a>
  <a href="https://www.npmjs.com/package/create-webclaw"><img src="https://shieldcn.dev/npm/dt/create-webclaw.svg?variant=branded" alt="npm installs" /></a>
</p>

<p align="center">
  <a href="https://discord.gg/KDfd48EpnW"><img src="https://shieldcn.dev/badge/Discord-Join.svg?variant=branded&logo=discord" alt="Discord" /></a>
  <a href="https://x.com/webclaw_io"><img src="https://shieldcn.dev/badge/Follow-@webclaw__io.svg?variant=branded&logo=x" alt="X / Twitter" /></a>
  <a href="https://webclaw.io"><img src="https://shieldcn.dev/badge/Hosted-webclaw.io.svg?variant=branded&logo=safari" alt="Hosted webclaw" /></a>
  <a href="https://webclaw.io/docs"><img src="https://shieldcn.dev/badge/Docs-Read.svg?variant=branded&logo=readthedocs" alt="Docs" /></a>
</p>

<p align="center">
  <a href="https://trendshift.io/repositories/24218?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-24218" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/24218/daily?language=Rust" alt="0xMassi/webclaw | Trendshift" width="250" height="55"/></a>
</p>

<p align="center">
  <img src="assets/demo.gif" alt="webclaw extracting clean markdown from a page" width="760" />
</p>

---

Most web scraping tools give your agent one of two bad outputs:

- a blocked page, login wall, or empty app shell
- raw HTML full of nav, scripts, styling, ads, and duplicated boilerplate

[webclaw.io](https://webclaw.io) is the hosted web extraction API for webclaw. This repo contains the open-source CLI, MCP server, extraction engine, and self-hostable server.

webclaw turns a URL into clean content your tools can actually use.

```bash
webclaw https://example.com --format markdown
```

```md
# Example Domain

This domain is for use in illustrative examples in documents.

You may use this domain in literature without prior coordination or asking for permission.
```

Use it from the terminal, wire it into Claude/Cursor through MCP, call the hosted API from your app, or self-host the OSS server.

---

## Install

### Agent setup

The fastest way to connect webclaw to Claude Code, Claude Desktop, Cursor, Windsurf, OpenCode, Codex CLI, and other MCP-compatible tools:

```bash
npx create-webclaw
```

The installer detects supported clients and configures the MCP server for you.

### Homebrew

```bash
brew tap 0xMassi/webclaw
brew install webclaw
```

### Prebuilt binaries

Download macOS, Linux, and Windows binaries from [GitHub Releases](https://github.com/0xMassi/webclaw/releases).

#### Windows (x64)

The prebuilt ZIP is the easiest installation and does not require Rust. On the
[v0.6.22 release page](https://github.com/0xMassi/webclaw/releases/tag/v0.6.22),
download `webclaw-v0.6.22-x86_64-pc-windows-msvc.zip`. Extract it in File Explorer,
open PowerShell in the extracted folder containing `webclaw.exe`, and run:

```powershell
.\webclaw.exe --version
.\webclaw.exe https://example.com --format markdown
```

PowerShell requires the `.\` prefix for programs in the current directory.
Add that directory to your user PATH only if you want to run `webclaw` from
other folders. If Windows reports a missing Visual C++ runtime DLL, install the
Microsoft Visual C++ Redistributable for x64. The ZIP contains x64 binaries;
Windows ARM64 is not covered by the native installation check.

To build from source on Windows, install Rust with the `x86_64-pc-windows-msvc`
toolchain, Visual Studio Build Tools with **Desktop development with C++** and
the Windows SDK, Git, CMake 3.22 or newer, LLVM (including `libclang.dll`), and
NASM. Open a fresh Developer PowerShell for Visual Studio after installation.
Ensure CMake and NASM are on PATH and set `LIBCLANG_PATH` to your LLVM `bin`
directory if bindgen cannot find it, for example:

```powershell
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
cargo install --git https://github.com/0xMassi/webclaw.git --tag v0.6.22 --locked webclaw-cli
webclaw --version
```

The Cargo package is `webclaw-cli`; the executable is `webclaw`. These packages
are installed from Git, not crates.io. A first source build can take several
minutes. The Windows CI workflow checks the Git installation and published ZIP
on a hosted runner with preinstalled build tools; it does not model a clean PC.
Linux containers, including Docker on macOS, do not validate Windows support.

### Docker

```bash
docker run --rm ghcr.io/0xmassi/webclaw https://example.com
```

### Cargo

```bash
cargo install --git https://github.com/0xMassi/webclaw.git --tag v0.6.22 --locked webclaw-cli
cargo install --git https://github.com/0xMassi/webclaw.git --tag v0.6.22 --locked webclaw-mcp
```

If building from source fails because native build tools are missing, install the platform prerequisites:

| OS | Command |
| --- | --- |
| Debian / Ubuntu | `sudo apt install -y pkg-config libssl-dev cmake clang git build-essential` |
| Fedora / RHEL | `sudo dnf install -y pkg-config openssl-devel cmake clang git make gcc` |
| Arch | `sudo pacman -S pkg-config openssl cmake clang git base-devel` |
| macOS | `xcode-select --install` |

---

## Quick Start

### Scrape one page

```bash
webclaw https://stripe.com --format markdown
```

### Return LLM-optimized text

```bash
webclaw https://docs.anthropic.com --format llm
```

### Keep only the main content

```bash
webclaw https://example.com/blog/post --only-main-content
```

### Include or exclude selectors

```bash
webclaw https://example.com \
  --include "article, main, .content" \
  --exclude "nav, footer, .sidebar, .ad"
```

### Pick values with CSS selectors or XPath

```bash
webclaw https://books.toscrape.com --select "h3 a::attr(href)"
webclaw https://books.toscrape.com --select "article.product_pod" \
  --field "title=h3 a::attr(title)" --field price=.price_color
```

`--select` prints a JSON array of what the selector matches: each element's text, or an attribute or the element's HTML when the selector ends in `::attr(NAME)` or `::html`. Links in `href` and `src` come back absolute. With `--field NAME=SELECTOR`, each match becomes a JSON object whose fields run inside it, taking the first value or `null`. Pages that need JavaScript render first, as with extraction.

```bash
webclaw https://books.toscrape.com --xpath "//li[@class='next']/a/@href"
webclaw https://books.toscrape.com --xpath "//article[@class='product_pod']" \
  --field "title=.//h3/a/@title" --field "price=.//p[@class='price_color']"
```

`--xpath` takes an XPath 1.0 expression instead, over the same parsed page: elements and text nodes give their text, attributes their value, and `count()` or `string()` style expressions one value. With `--xpath`, each `--field` is an expression evaluated from the match, so it usually starts with `.`. The MCP `select` tool takes `xpath` in place of `selector`.

### Data tables as JSON

```bash
webclaw "https://en.wikipedia.org/wiki/List_of_S%26P_500_companies" --tables
```

`--tables` prints the page's data tables as a JSON array, each with its `caption` (when it has one), `headers` and `rows` of cell text. A cell that spans several rows or columns repeats its text in each one, so every row has one value per column. Stacked header rows are joined per column with ` / `, as in `Population / % of world`. The MCP `select` tool takes `tables: true` in place of a selector.

Header rows are the rows of `<thead>` and the leading rows made only of `<th>`; a leading row that is one cell across the whole table becomes the caption. These tables are skipped as page layout:

- tables marked `role="presentation"` or `role="none"`;
- tables with fewer than two columns of text;
- tables without a header row that have a single body row, wrap another table, hold only links, or hold paragraphs, lists or other blocks in a row of more than one cell.

Text hidden with `hidden`, an inline `display:none` or `<template>` is left out. Text hidden by a stylesheet is kept, because pages are not styled. A cell keeps its first 64 KB of text, and a very large table is cut short with `"truncated": true`. Once the tables on a page reach 64 MB, the tables after that are left out.

### Find elements by text, or everything like one example

```bash
webclaw https://books.toscrape.com --find-text "tipping the"
webclaw https://books.toscrape.com --find-regex '^£5\d\.\d\d$'
webclaw https://books.toscrape.com --find-text "Tipping the Velvet" --similar
```

`--find-text` prints the elements in the page body whose visible text contains the text, any case, as `{"selector", "text"}` objects. Each match is the deepest element holding the text, so text split by inline tags still matches, and its `selector` is the element's path from the nearest ancestor with a unique id, or from the root. That path picks the element alone, except on pages nested more than 64 levels deep with no such id, where it keeps only the last 64 steps and can pick more. `--exact` matches the whole text, `--case-sensitive` matches letter case, and `--find-regex` takes a regular expression instead.

`--similar` takes the first element that `--find-text`, `--find-regex`, `--select` or `--xpath` picks as an example and returns every element like it on the page, such as the other items of a list, grid or table, with one CSS selector that picks them all. When the example selector ends in `::attr(NAME)` or `::text`, or the XPath picks a text or attribute node, the element holding it is the example:

```json
{
  "selector": "article.product_pod > h3 > a",
  "matches": [
    { "selector": "html > body > ... > li:nth-of-type(1) > article > h3 > a", "text": "A Light in the ..." },
    ...
  ]
}
```

Reuse the selector with `--select`, or add `--field` to get records straight away; with `--similar` the fields are CSS and the output is `{"selector", "records"}`. Pick the item itself as the example (for instance `--select "article.product_pod" --similar`) when the fields live inside it. The MCP `select` tool takes `text` or `regex` (with `exact` and `case_sensitive`) and `similar` the same way. One call takes one picker: `--select`, `--xpath`, `--tables`, `--find-text` or `--find-regex` (in MCP: `selector`, `xpath`, `tables`, `text` or `regex`), and `--similar` and `--field` do not apply to tables.

### Keep only what answers a question

```bash
webclaw "https://en.wikipedia.org/wiki/Rust_(programming_language)" -f llm \
  --query "Who began the language as a personal project?"
```

`--query` (and `query` on the MCP `scrape` tool) scores each paragraph, list, table and code block against the question and keeps the best ones under their headings, so an agent reads the answer instead of the whole page. The page comes back unchanged when no block shares a word with the question.

### Reuse fetched pages

```bash
webclaw https://example.com --cache          # fetch once, answer later runs from disk
```

`--cache` keeps every successful page in `~/.cache/webclaw` (or `WEBCLAW_CACHE_DIR`), readable by you only, and answers later fetches of the same URL with the same browser profile and headers from there, with no network request. Crawls and batches use it page by page. Entries never expire; delete the directory to refresh. Error responses, challenge pages and `Set-Cookie` headers are never stored. The cache keeps the HTTP response only: a page that needs JavaScript still renders in the browser, which loads its scripts over the network.

### Render JavaScript pages

Build with the `browser` feature (`cargo build --release --features browser`) and webclaw renders pages in your local Chrome when the plain HTTP response has no content yet:

```bash
webclaw https://hn.algolia.com                    # --render auto: HTTP first, Chrome only when needed
webclaw https://example.com --render always --wait-for "#app li"
webclaw https://example.com --cdp-url ws://localhost:9222/devtools/browser/<id>   # use a running browser
```

`auto` renders when the page loads scripts and the HTTP response has almost no text, or little text next to an empty app root or a "please enable JavaScript" notice, and it keeps the rendered result only when it has more content.

What a rendered page can reach, in the browser webclaw launches:

- no HTTP request from the page, its frames or its workers, redirects included, reaches a private, loopback or link-local address;
- WebSockets to those addresses are refused, though the TCP connection opens first. Behind a proxy, WebSockets to anything but loopback go through the proxy, so they are only as safe as the proxy;
- speculative prefetching and preconnects are off;
- WebRTC sends no UDP, though a TURN server over TCP can still open a connection to a local port;
- without a proxy, a render fails when the page's document came from a private address, so DNS rebinding cannot return local content. The request itself still goes out, and popups are not checked.

Service workers are not tested. A browser you connect with `--cdp-url` is checked page by page and your other tabs are left alone, but the frames, workers, WebSockets and WebRTC of the pages webclaw renders there are not covered.

`--block images,fonts,media` skips those resources while a page renders for its content. Stylesheets always load, since many apps fetch CSS on demand and break without it. Scripts and the data they fetch still load, so most pages keep all their text and download about 40% less. A page that shows some text only after a font or an image has loaded (through `document.fonts.load()`, `FontFace.load()`, an image's `onload` or `decode()`) loses that text when you block it. We found no such page among the real sites we tested, but compare with and without `--block` on yours. Rendering is not much faster, since webclaw already stops waiting once the text settles, so blocking stays off unless you ask. Screenshots always load everything. The MCP `scrape` and `crawl` tools take the same list as `block`.

`--screenshot page.png` saves the rendered page's 1280x720 viewport, `--full-page` the whole page, and a `.jpg` name gives a JPEG. The MCP `screenshot` tool returns the same image to the agent.

Crawls, batches, `--watch` and the MCP server render the same way and share one browser, started only when a page needs it. Per site, webclaw keeps count: once at least three renders show that rendering adds content (at least four in five did), it renders every page of that site that runs scripts, and once rendering stops adding content (at most one in five did), it stops rendering there.

### Crawl a documentation site

```bash
webclaw https://docs.rust-lang.org --crawl --depth 2 --max-pages 50
```

The crawler follows the site's robots.txt: it skips disallowed URLs and honours `Crawl-delay`. Pass `--ignore-robots` to turn that off.

For long crawls, stream pages as JSON lines and checkpoint the crawl so a crash or kill resumes where it stopped:

```bash
webclaw https://docs.rust-lang.org --crawl --max-pages 0 -f jsonl --crawl-state crawl.json >> pages.jsonl
```

Give the crawl a question and it fetches the links most likely to answer it first, instead of level by level:

```bash
webclaw https://docs.python.org/3/ --crawl --depth 4 --max-pages 25 \
  --query "How do I sort a list of dictionaries by a key?"
```

Each link is scored by the question's words in its text, its URL path and the paragraph or list item around it, plus how well the page that links to it answers the question; words that most pages share count for little. Every page then keeps only the parts that answer the question, carries a `relevance` from 0 to 1, and the output lists pages best match first (`-f jsonl` streams them in crawl order instead). `--max-pages` is the budget. A resumed `--crawl-state` keeps the order when it gets the same `--query`. The MCP `crawl` tool takes the same `query`, up to 1,000 characters.

The scoring matches words, so it works best on English questions; questions in other languages reach their page later (Japanese, French, Spanish and Chinese questions in the benchmark: 3 of 16 by page 10 and a median of 18 pages, against 33 of 48 and 6 pages for English questions on sites the weights were not tuned on). Only the first 32 distinct words of a question count.

### Workflow examples

- [HTML to Markdown for RAG](examples/html-to-markdown-rag/)
- [Firecrawl-compatible API](examples/firecrawl-compatible-api/)
- [MCP web scraping](examples/mcp-web-scraping/)
- [Proxy-backed crawling with ColdProxy](examples/proxy-backed-crawling/)
- [Cloudflare diagnostics](examples/cloudflare-diagnostics/)

### Extract brand assets

```bash
webclaw https://github.com --brand
```

### Compare a page over time

```bash
webclaw https://example.com/pricing --format json > pricing-old.json
webclaw https://example.com/pricing --diff-with pricing-old.json
```

---

## MCP Server

webclaw ships with an MCP server for AI agents.

Zero-install — point any MCP client at the npx launcher:

```json
{
  "mcpServers": {
    "webclaw": {
      "command": "npx",
      "args": ["-y", "@webclaw/mcp"]
    }
  }
}
```

Or run `npx create-webclaw` to auto-detect your AI tools and write their configs for you.

### Over HTTP

To share one server between agents, or reach it from another machine, serve it over streamable HTTP:

```bash
WEBCLAW_MCP_TOKEN=$(openssl rand -hex 16) webclaw-mcp --http 127.0.0.1:8000
```

Clients connect to `http://127.0.0.1:8000/mcp` and send `Authorization: Bearer <token>`:

```json
{
  "mcpServers": {
    "webclaw": {
      "type": "http",
      "url": "http://127.0.0.1:8000/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

The server refuses to start over HTTP without a token, because anyone who reaches it can make the machine fetch any URL; `--no-auth` serves it open anyway. On a loopback address it also rejects requests whose `Host` header names another host, which blocks DNS-rebinding attacks from web pages.

Then ask your agent things like:

```text
Scrape these competitor pricing pages and summarize the differences.
```

```text
Crawl this documentation site and prepare clean context for a RAG index.
```

```text
Extract the brand colors, fonts, and logos from this company website.
```

---

## Use as an agent skill

Add webclaw to Claude Code, Cursor, Windsurf, and other MCP agents in one command:

```bash
npx skills add 0xMassi/webclaw-skill
```

Your agent gets scrape, crawl, map, extract, summarize, diff, brand, and search
as native tools. Most sites extract locally with no API key. Set `WEBCLAW_API_KEY`
to handle bot-protected and JavaScript-rendered pages.

Find it on [skills.sh](https://www.skills.sh/0xMassi/webclaw-skill/webclaw).

---

## Tools

| Tool | What it does | Local |
| --- | --- | :-: |
| `scrape` | Extract one URL as markdown, text, JSON, LLM format, or HTML | Yes |
| `select` | Pick values or records from a page with CSS selectors or XPath, its data tables, elements found by text, or every element like an example, as JSON | Yes |
| `screenshot` | Capture a page's viewport or the whole page as PNG or JPEG | Yes, with a `browser` build |
| `crawl` | Follow same-origin links and extract discovered pages, most relevant first with a `query` | Yes |
| `map` | Discover URLs without extracting every page | Yes |
| `batch` | Scrape multiple URLs in parallel | Yes |
| `extract` | Convert page content into structured data | Yes, with local or configured LLM |
| `summarize` | Summarize a page | Yes, with local or configured LLM |
| `diff` | Compare page content snapshots | Yes |
| `brand` | Extract colors, fonts, logos, and metadata | Yes |
| `search` | Search the web and scrape results | Hosted API |
| `research` | Multi-source research workflow | Hosted API |

---

## SDKs

```bash
npm install @webclaw/sdk
pip install webclaw
go get github.com/0xMassi/webclaw-go
```

<details>
<summary>TypeScript</summary>

```ts
import { Webclaw } from "@webclaw/sdk";

const client = new Webclaw({ apiKey: process.env.WEBCLAW_API_KEY! });

const page = await client.scrape({
  url: "https://example.com",
  formats: ["markdown"],
  only_main_content: true,
});

console.log(page.markdown);
```

</details>

<details>
<summary>Python</summary>

```python
from webclaw import Webclaw

client = Webclaw(api_key="wc_your_key")

page = client.scrape(
    "https://example.com",
    formats=["markdown"],
    only_main_content=True,
)

print(page.markdown)
```

</details>

<details>
<summary>cURL</summary>

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

</details>

---

## Output Formats

| Format | Use it when you need |
| --- | --- |
| `markdown` | Clean page content with structure preserved |
| `llm` | Compact context for agents and RAG pipelines |
| `text` | Plain text with minimal formatting |
| `json` | Structured metadata, links, images, and extracted fields |
| `html` | Cleaned HTML for custom processing |

---

## Local First, Hosted When Needed

The CLI and MCP server work locally without an account for the core extraction path.

Use the hosted API at [webclaw.io](https://webclaw.io) when you need:

- protected-site access without managing infrastructure
- JavaScript rendering
- async crawl and research jobs
- web search
- watches and production usage tracking
- SDKs for application code

```bash
export WEBCLAW_API_KEY=wc_your_key

webclaw https://example.com --cloud
```

---

## What You Can Build

| Use case | Example |
| --- | --- |
| AI agent web access | Give Claude, Cursor, or another MCP client clean page context |
| RAG ingestion | Crawl docs, help centers, blogs, and knowledge bases |
| Competitor monitoring | Track pricing pages, changelogs, docs, and product pages |
| Structured extraction | Turn messy pages into typed JSON for automations |
| Research workflows | Search, scrape, summarize, and cite multiple sources |
| Brand intelligence | Extract logos, colors, fonts, and social metadata |

## Architecture

```text
webclaw/
  crates/
    webclaw-core     HTML to markdown, text, JSON, and LLM-ready output
    webclaw-fetch    Fetching, crawling, batching, and mapping
    webclaw-llm      Local and hosted LLM provider support
    webclaw-pdf      PDF text extraction
    webclaw-mcp      MCP server for AI agents
    webclaw-cli      Command-line interface
```

`webclaw-core` is pure extraction logic: no network I/O, small surface area, and usable independently from the fetching layer.

---

## Configuration

| Variable | Description |
| --- | --- |
| `WEBCLAW_API_KEY` | Hosted API key |
| `OLLAMA_HOST` | Ollama URL for local LLM features |
| `OPENAI_API_KEY` | OpenAI-compatible LLM provider key |
| `OPENAI_BASE_URL` | OpenAI-compatible base URL |
| `ANTHROPIC_API_KEY` | Anthropic-compatible LLM provider key |
| `ANTHROPIC_BASE_URL` | Anthropic-compatible base URL |
| `ORCAROUTER_API_KEY` | OrcaRouter LLM provider key |
| `ORCAROUTER_BASE_URL` | OrcaRouter base URL (defaults to https://api.orcarouter.ai/v1) |
| `WEBCLAW_PROXY` | Single proxy URL |
| `WEBCLAW_PROXY_FILE` | Proxy pool file |
| `WEBCLAW_RENDER` | MCP server rendering: `auto` (default), `always` or `never`; needs a `browser` build |
| `WEBCLAW_CDP_URL` | MCP server renders with the browser at this DevTools endpoint instead of launching Chrome |
| `WEBCLAW_CHROME` | Chrome executable to launch for rendering |
| `WEBCLAW_MCP_TOKEN` | Bearer token `webclaw-mcp --http` requires from every client |

---

## Contributing

The most useful contributions right now are practical and small:

- add examples for real agent and RAG workflows
- improve SDK snippets
- report pages that extract poorly
- add failing fixtures for messy HTML
- improve docs for MCP clients and local setup
- test the CLI on more Linux/macOS environments

Good first places to start:

- [Good first issues](https://github.com/0xMassi/webclaw/issues?q=label%3A%22good+first+issue%22)
- [Open a bug report](https://github.com/0xMassi/webclaw/issues/new)
- [Start a discussion](https://github.com/0xMassi/webclaw/discussions)

If a page extracts badly, include:

```text
URL:
Command or API request:
Expected output:
Actual output:
Format used: markdown / llm / text / json / html
CLI, MCP, SDK, or API:
```

Please remove secrets, cookies, private tokens, and customer data from logs before posting.

---

## Strategic Partner

<table>
  <tr>
    <td align="center">
      <!-- GitHub renders the README on light OR dark depending on the
           viewer's theme, and SerpApi's media kit ships a mark per
           background: gradient-icon-with-black-text for light, all-white for
           dark. <picture> picks the right one instead of leaving a white
           wordmark invisible on light mode. Colours unmodified, per their
           media kit. -->
      <a href="https://serpapi.com/use-cases/web-search-api?utm_source=webclaw">
        <picture>
          <source
            media="(prefers-color-scheme: dark)"
            srcset="./assets/sponsors/serpapi-white.png"
          />
          <img src="./assets/sponsors/serpapi.png" alt="SerpApi" width="720" />
        </picture>
      </a>
    </td>
  </tr>
  <tr>
    <td>
      <a href="https://serpapi.com/use-cases/web-search-api?utm_source=webclaw"><strong>SerpApi, the Web Search API</strong></a>.
      Give real-time data to your AI agents and enhance their responses with SerpApi’s
      structured search engine results. SerpApi supports webclaw as a Strategic Partner.
    </td>
  </tr>
</table>

---

## Infrastructure Partner

<table>
  <tr>
    <td align="center">
      <a href="https://coldproxy.com/?utm_source=github&utm_medium=sponsorship&utm_campaign=webclaw-sponsor">
        <img src="./assets/sponsors/coldproxy-banner.png" alt="ColdProxy" width="720" />
      </a>
    </td>
  </tr>
  <tr>
    <td>
      <strong>ColdProxy</strong> supports webclaw as an Infrastructure Partner, providing residential IPv4,
      residential IPv6, and datacenter IPv6 proxy infrastructure across 195+ countries for public data
      collection, regional testing, monitoring, and web scraping workflows. Explore
      <a href="https://coldproxy.com/?utm_source=github&utm_medium=sponsorship&utm_campaign=webclaw-sponsor">ColdProxy</a>'s latest plans and available offers directly on the website.
      Use code <code>webclaw8Off</code> for 8% off your first payment.
      See the <a href="examples/proxy-backed-crawling/#using-coldproxy">proxy-backed crawling guide</a>
      for a hands-on walkthrough of wiring ColdProxy into webclaw.
    </td>
  </tr>
</table>

---

## Studio Partners

<table>
  <tr>
    <td width="340" align="center">
      <a href="https://go.nodemaven.com/webclawGHsept">
        <img src="./assets/sponsors/nodemaven-banner-20260923.png" alt="NodeMaven" width="300" />
      </a>
    </td>
    <td>
      <p><a href="https://go.nodemaven.com/webclawGHsept"><strong>NodeMaven</strong></a>: The most efficient proxy provider for Web Scraping and Automation with the Highest Quality IP on the market.</p>
      <p><a href="https://go.nodemaven.com/webclawGHsept"><strong>Why NodeMaven?</strong></a></p>
      <ul>
        <li>ZIP targeting</li>
        <li>99.9% uptime</li>
        <li>IP filtering: all proxies have fraud score &lt;97%</li>
        <li>No KYC required</li>
        <li>Unique free tools: Proxy Bandwidth Checker, Meta Tag Checker, IP Lookup and others!</li>
      </ul>
      <p><strong>Special codes for Webclaw users:</strong></p>
      <ul>
        <li><code>WEBCLAW35</code> - 35% off to Mobile and Residential Proxies</li>
        <li><code>WEBCLAW40</code> - 40% off to ISP (Static) Proxies</li>
      </ul>
    </td>
  </tr>
  <tr>
    <td width="340" align="center">
      <a href="https://mangoproxy.com/?utm_source=github&utm_medium=partner&utm_campaign=0xmassi">
        <img src="./assets/sponsors/mangoproxy-banner.png" alt="MangoProxy" width="300" />
      </a>
    </td>
    <td>
      <strong>MangoProxy</strong> provides residential, ISP, datacenter, and mobile proxies across 200+ locations, backed by a 90M+ IP pool with HTTP and SOCKS5 support and high stability for web scraping and data collection at scale.
      Use code <code>0XMASSI</code> for 8% off ISP (Static) proxies at
      <a href="https://mangoproxy.com/?utm_source=github&utm_medium=partner&utm_campaign=0xmassi">mangoproxy.com</a>.
    </td>
  </tr>
  <tr>
    <td width="340" align="center">
      <a href="https://www.thordata.com/?ls=dww&lk=dww">
        <img src="./assets/sponsors/thordata-banner.png" alt="Thordata" width="300" />
      </a>
    </td>
    <td>
      <p><a href="https://www.thordata.com/?ls=dww&lk=dww"><strong>Thordata</strong></a>: Premium Residential Proxies for Developers. Build reliable crawlers, AI agents, and automation workflows with clean residential IPs and stable proxy infrastructure.</p>
      <p><a href="https://www.thordata.com/?ls=dww&lk=dww"><strong>Why Thordata?</strong></a></p>
      <ul>
        <li>100M+ IPs across 195+ GEOs</li>
        <li>Unlimited concurrent connections</li>
        <li>99.99% uptime &amp; stable connections</li>
        <li>Rotating + Sticky Sessions</li>
      </ul>
      <p><strong>Webclaw Special Offer:</strong> Free 3-Day Trial</p>
      <ul>
        <li><code>WEBCLAW</code> - 10% OFF</li>
      </ul>
    </td>
  </tr>
</table>

---

## Community Plugins

Third-party plugins that integrate webclaw with AI agent platforms:

| Plugin | Platform | What it does |
|---|---|---|
| [openclaw-webclaw](https://github.com/jal-co/openclaw-webclaw) | [OpenClaw](https://openclaw.ai) | Native webclaw v1 API plugin with 9 tools: scrape, search, crawl, extract, summarize, diff, map, batch, brand |
| [hermes-webclaw](https://github.com/jal-co/hermes-webclaw) | [Hermes Agent](https://github.com/NousResearch/hermes-agent) | Web search provider and 9 dedicated tools for the full v1 API surface. Install with `hermes plugins install jal-co/hermes-webclaw` |

Built a webclaw integration? [Open a PR](https://github.com/0xMassi/webclaw/pulls) to add it here.

---

## Contributors

Thanks to everyone improving webclaw through issues, examples, docs, bug reports, and pull requests.

<a href="https://github.com/0xMassi/webclaw/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=0xMassi/webclaw" alt="webclaw contributors" />
</a>

---

## Star History

<a href="https://github.com/0xMassi/webclaw/stargazers">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://shieldcn.dev/chart/github/stars/0xMassi/webclaw.svg?mode=dark&theme=zinc&bg=transparent&border=false&logo=false&icon=Star" />
   <source media="(prefers-color-scheme: light)" srcset="https://shieldcn.dev/chart/github/stars/0xMassi/webclaw.svg?mode=light&theme=zinc&bg=transparent&border=false&logo=false&icon=Star" />
   <img alt="Star History Chart" src="https://shieldcn.dev/chart/github/stars/0xMassi/webclaw.svg?mode=light&theme=zinc&bg=transparent&border=false&logo=false&icon=Star" />
 </picture>
</a>

---

## License

[AGPL-3.0](LICENSE)
