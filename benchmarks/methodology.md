# Methodology

## The command

```bash
./benchmarks/run.sh [label] [webclaw-binary] [url-runs]
```

You need `uv` installed. Without a binary argument, the script builds `target/release/webclaw`. Each competitor gets its own pinned environment under `benchmarks/.work/`. The script runs every tool over every case and writes `results/<label>/summary.md`, `rows.jsonl` and `versions.txt`. Defaults: label is today's date, 3 live rounds.

`WEBCLAW_API_KEY` is unset for the run, so webclaw never calls its hosted API. Every number comes from the open-source CLI on your machine.

## Tools and settings

| Arm | Call |
|---|---|
| `webclaw-llm` | `webclaw <url> -f llm` |
| `webclaw-md` | `webclaw <url> -f markdown` |
| `trafilatura` | `fetch_url`, then `extract(html, output_format="markdown", include_links=True, include_tables=True, favor_recall=True)` |
| `scrapling` | `Fetcher.get(url).markdown()` |
| `crawl4ai-raw` | `AsyncWebCrawler.arun(url)` with `DefaultMarkdownGenerator()`, `raw_markdown` |
| `crawl4ai-fit` | same crawl with `DefaultMarkdownGenerator(content_filter=PruningContentFilter())`, `fit_markdown` |

Versions are pinned in `run.sh` and recorded in each run's `versions.txt`. Every tool runs with default options plus the flags above. Nobody tunes per site.

## Two modes

- **url**: each tool fetches the page itself. This is what you get when you hand each tool a URL: fetching, blocking and extraction all count. Every URL runs `url-runs` times.
- **html**: every tool gets the same saved HTML, so only extraction differs. `run.py prepare` saves each non-PDF case once with `webclaw --raw-html` into `.work/corpus/`. It never overwrites a saved file, so later runs on your machine compare against the same input. The corpus is third-party content and stays out of git; your first run creates yours.

## Cases

`cases.jsonl` lists 78 public URLs in 8 categories: news, ecommerce, docs, spa, pdf, nonenglish, paywalled (public previews only) and homepage. Each case lists facts that a reader sees on the page: names, figures, product terms. The 18 homepage cases have 5 facts each; the others have 2. Single-token facts match on word boundaries, so `API` does not hit `apiece`. Other facts match as substrings.

Propose new cases or facts by pull request. Every arm runs against them on the next run.

## Metrics

- **ok %**: share of runs that returned non-empty output without an error.
- **fact recall %**: facts found over facts expected, with failed runs counted as zero.
- **median tokens**: `cl100k_base` tokens of the output, median over successful runs. Fewer tokens at the same recall means cheaper LLM input.
- **p50 / p95 s**: wall time per URL, including process start-up for the CLI arms.

## Limits

- Two facts per case is a coarse signal. News, docs and ecommerce come out near 100% for most tools, so homepage, spa, pdf and paywalled carry most of the difference between tools.
- Live pages change. When a fact disappears from the page, every tool misses it together.
- `spa` pages need JavaScript. Crawl4AI renders them in a browser; webclaw, Scrapling's `Fetcher` and Trafilatura do not.
- Results depend on your network and location. Compare runs from the same machine.
