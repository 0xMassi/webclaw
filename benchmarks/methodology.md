# Methodology

## What is measured

Three metrics per site:

1. **Token efficiency** — tokens of the extractor's output vs tokens of the
   raw fetched HTML. Lower tokens = cheaper to feed into an LLM. But lower
   tokens *only matters if the content is preserved*, so tokens are always
   reported alongside fidelity.
2. **Fidelity** — how many hand-curated "visible facts" the extractor
   preserved. Per site we list 5 strings that any reader would say are
   meaningfully on the page (customer names, headline stats, product names,
   release information). Matched case-insensitively with word boundaries
   where the fact is a single alphanumeric token (`API` does not match
   `apiece`).
3. **Latency** — wall-clock time from URL submission to markdown output.
   Includes fetch + extraction. Network-dependent, so reported as the
   median of 3 runs.

## Tokenizer

`cl100k_base` via OpenAI's `tiktoken` crate. This is the encoding used by
GPT-4, GPT-3.5-turbo, and `text-embedding-3-*` — the models most users plug
extracted web content into. Pinned in `scripts/bench.py`.

## Tool versions

Listed at the top of each run's `results/YYYY-MM-DD.json` file. The run
published at launch used:

- `webclaw 0.3.18` (release build, default options, `--format llm`)
- `trafilatura 2.0.0` (`extract(html, output_format="markdown",
  include_links=True, include_tables=True, favor_recall=True)`)
- `firecrawl-py 4.x` against Firecrawl's hosted `v2` API
  (`scrape(url, formats=["markdown"])`)

## Fact selection

Facts for each site were chosen by manual inspection of the live page in a
browser on 2026-04-17. Selection criteria:

- must be **visibly present** (not in `<head>`, `<script>`, or hidden
  sections)
- must be **specific** — customer names, headline stats, product names,
  release dates. Not generic words like "the", "platform", "we".
- must be **stable across multiple loads** (no AB-tested copy, no random
  customer rotations)
- 5 facts per site, documented in `facts.json`

Facts are committed as data, not code, so **new facts can be proposed via
pull request**. Any addition runs against all three tools automatically.

Known limitation: sites change. News aggregators, release pages, and
blog indexes drift. If a fact disappears because the page changed (not
because the extractor dropped it), we expect all three tools to miss it
together, which makes it visible as "all tools tied on this site" in the
per-site breakdown. Facts on churning pages are refreshed on each published
run.

## Why median of 3 runs

Single-run numbers are noisy:

- **Latency** varies ±30% from run to run due to network jitter, CDN cache
  state, and the remote server's own load.
- **Raw-HTML token count** can vary if the server renders different content
  per request (A/B tests, geo-IP, session state).
- **Tool-specific flakiness** exists at the long tail. The occasional
  Firecrawl 502 or trafilatura fetch failure would otherwise distort a
  single-run benchmark.

We run each site 3 times, take the median per metric. The published
number is the 50th percentile; the full run data (min / median / max)
is preserved in `results/YYYY-MM-DD.json`.

## Fair comparison notes

- **Each tool fetches via its own preferred path.** webclaw uses its
  in-process primp HTTP client. Trafilatura uses `requests`. Firecrawl
  fetches via its hosted infrastructure (Chrome CDP when needed). This is
  the apples-to-apples developer-experience comparison: what you get when
  you call each tool with a URL. The "vs raw HTML" column uses webclaw's
  `--raw-html` as the baseline denominator.
- **Firecrawl's default engine picker** runs in "auto" mode with browser
  rendering for sites it detects need it. No flags tuned, no URLs
  cherry-picked.
- **No retries**, no fallbacks, no post-processing on top of any tool's
  output. If a tool returns `""` or errors, that is the measured result
  for that run. The median of 3 runs absorbs transient errors; persistent
  extraction failures (e.g. trafilatura on `simonwillison.net`, which
  returned `""` on all 3 runs) show up as 0 tokens and 0 facts.

## Raw data schema

`results/YYYY-MM-DD.json`:

```json
{
  "timestamp": "2026-04-17 ...",
  "webclaw_version": "0.3.18",
  "trafilatura_version": "2.0.0",
  "tokenizer": "cl100k_base",
  "runs_per_site": 3,
  "site_count": 18,
  "total_facts": 90,
  "aggregates": {
    "webclaw":     { "reduction_mean": 92.5, "fidelity_pct": 84.4, ... },
    "trafilatura": { "reduction_mean": 97.8, "fidelity_pct": 50.0, ... },
    "firecrawl":   { "reduction_mean": 92.4, "fidelity_pct": 77.8, ... }
  },
  "per_site": [
    {
      "url": "https://openai.com",
      "facts_count": 5,
      "raw_tokens": 170508,
      "webclaw":     { "tokens_med": 1238, "facts_med": 3, "seconds_med": 0.49 },
      "trafilatura": { "tokens_med": 0,    "facts_med": 0, "seconds_med": 0.17 },
      "firecrawl":   { "tokens_med": 3139, "facts_med": 2, "seconds_med": 1.08 }
    },
    ...
  ]
}
```

## What's not here (roadmap)

These measurements are intentionally out of scope for this initial
benchmark. Each deserves its own harness and its own run.

- **n-gram content overlap** — v2 metric to replace curated-fact matching.
  Measure: fraction of trigrams from the visually-rendered page text that
  appear in the extractor's output. Harder to curate, easier to scale.
- **Competitors besides trafilatura / firecrawl** — Mozilla Readability,
  Newspaper3k, Crawl4AI, Diffbot, Jina Reader. Require either JS ports or
  wrapper subprocess runners. PRs welcome.
- **Anti-bot / protected sites** — Cloudflare Turnstile, DataDome, AWS
  WAF, hCaptcha. These require the Webclaw Cloud API with the antibot
  sidecar, not the open-source CLI, and will be published separately on
  the Webclaw landing page once the testing harness there is public.
- **Crawl throughput** — pages-per-second under concurrent load. Different
  axis from single-page extraction; lives in its own benchmark.
