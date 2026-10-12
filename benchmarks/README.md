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

## Methodology

See [methodology.md](methodology.md) for each tool's exact call, the url and html modes, how facts are matched, the metrics and the limits of this setup.

## Raw data

Each published run commits `results/<label>/rows.jsonl` (one row per tool, case, mode and round), `summary.md` and `versions.txt`. `results/2026-04-17.json` is the earlier 18-site run against webclaw v0.3.18, kept for history.
