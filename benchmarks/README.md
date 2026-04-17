# Benchmarks

Reproducible benchmarks comparing `webclaw` against open-source and commercial
web extraction tools. Every number here ships with the script that produced it.
Run `./run.sh` to regenerate.

## Headline

**webclaw preserves more page content than any other tool tested, at 2.4× the
speed of the closest competitor.**

Across 18 production sites (SPAs, documentation, long-form articles, news,
enterprise marketing), measured over 3 runs per site with OpenAI's
`cl100k_base` tokenizer. Last run: 2026-04-17, webclaw v0.3.18.

| Tool | Fidelity (facts preserved) | Token reduction vs raw HTML | Mean latency |
|---|---:|---:|---:|
| **webclaw `--format llm`** | **76 / 90  (84.4 %)** | 92.5 % | **0.41 s** |
| Firecrawl API (v2, hosted) | 70 / 90  (77.8 %) | 92.4 % | 0.99 s |
| Trafilatura 2.0 | 45 / 90  (50.0 %) | 97.8 % (by dropping content) | 0.21 s |

**webclaw matches or beats both competitors on fidelity on all 18 sites.**

## Why webclaw wins

- **Speed.** 2.4× faster than Firecrawl's hosted API. Firecrawl defaults to
  browser rendering for everything; webclaw's in-process TLS-fingerprinted
  fetch plus deterministic extractor reaches comparable-or-better content
  without that overhead.
- **Fidelity.** Trafilatura's higher token reduction comes from dropping
  content. On the 18 sites tested it missed 45 of 90 key facts — entire
  customer-story sections, release dates, product names. webclaw keeps them.
- **Deterministic.** Same URL → same output. No LLM post-processing, no
  paraphrasing, no hallucination risk.

## Per-site results

Numbers are median of 3 runs. `raw` = raw fetched HTML token count.
`facts` = hand-curated visible facts preserved out of 5 per site.

| Site | raw HTML | webclaw | Firecrawl | Trafilatura | wc facts | fc facts | tr facts |
|---|---:|---:|---:|---:|:---:|:---:|:---:|
| openai.com | 170 K | 1,238 | 3,139 | 0 | **3/5** | 2/5 | 0/5 |
| vercel.com | 380 K | 1,076 | 4,029 | 585 | **3/5** | 3/5 | 3/5 |
| anthropic.com | 103 K | 672 | 560 | 96 | **5/5** | 5/5 | 4/5 |
| notion.com | 109 K | 13,416 | 5,261 | 91 | **5/5** | 5/5 | 2/5 |
| stripe.com | 243 K | 81,974 | 8,922 | 2,418 | **5/5** | 5/5 | 0/5 |
| tavily.com | 30 K | 1,361 | 1,969 | 182 | **5/5** | 4/5 | 3/5 |
| shopify.com | 184 K | 1,939 | 5,384 | 595 | **3/5** | 3/5 | 3/5 |
| docs.python.org | 5 K | 689 | 1,623 | 347 | **4/5** | 4/5 | 4/5 |
| react.dev | 107 K | 3,332 | 4,959 | 763 | **5/5** | 5/5 | 3/5 |
| tailwindcss.com/docs/installation | 113 K | 779 | 813 | 430 | **4/5** | 4/5 | 2/5 |
| nextjs.org/docs | 228 K | 968 | 885 | 631 | **4/5** | 4/5 | 4/5 |
| github.com | 234 K | 1,438 | 3,058 | 486 | **5/5** | 4/5 | 3/5 |
| en.wikipedia.org/wiki/Rust | 189 K | 47,823 | 59,326 | 37,427 | **5/5** | 5/5 | 5/5 |
| simonwillison.net/…/latent-reasoning | 3 K | 724 | 525 | 0 | **4/5** | 2/5 | 0/5 |
| paulgraham.com/essays.html | 2 K | 169 | 295 | 0 | **2/5** | 1/5 | 0/5 |
| techcrunch.com | 143 K | 7,265 | 11,408 | 397 | **5/5** | 5/5 | 5/5 |
| databricks.com | 274 K | 2,001 | 5,471 | 311 | **4/5** | 4/5 | 4/5 |
| hashicorp.com | 109 K | 1,501 | 4,289 | 0 | **5/5** | 5/5 | 0/5 |

## Reproducing this benchmark

```bash
cd benchmarks/
./run.sh
```

Requirements:
- Python 3.9+
- `pip install tiktoken trafilatura firecrawl-py`
- `webclaw` release binary at `../target/release/webclaw` (or set `$WEBCLAW`)
- Firecrawl API key (free tier: 500 credits/month, enough for many runs) —
  export as `FIRECRAWL_API_KEY`. If omitted, the benchmark runs with webclaw
  and Trafilatura only.

One run of the full suite burns ~60 Firecrawl credits (18 sites × 3 runs,
plus Firecrawl's scrape costs 1 credit each).

## Methodology

See [methodology.md](methodology.md) for:
- Tokenizer rationale (`cl100k_base` → covers GPT-4 / GPT-3.5 /
  `text-embedding-3-*`)
- Fact selection procedure and how to propose additions
- Why median of 3 runs (CDN / cache / network noise)
- Raw data schema (`results/*.json`)
- Notes on site churn (news aggregators, release pages)

## Raw data

Per-run results are committed as JSON at `results/YYYY-MM-DD.json` so the
history of measurements is auditable. Diff two runs to see regressions or
improvements across webclaw versions.
