# Fetch-path latency benchmark

`crates/webclaw-fetch/examples/latency_bench.rs` measures where wall-clock time
actually goes on the fetch path, separating **network** from **CPU**:

- `fetch_ms` — `FetchClient::fetch`: DNS + TCP + TLS + request + TTFB + body.
- `extract_ms` — `webclaw_core::extract_with_options`: pure parse + extraction.

This is deliberately different from `benchmarks/methodology.md`, which measures
extraction *quality* (tokens and fidelity). This one measures *speed*, and its
job is to say which half is worth optimising before anyone optimises anything.

## Running

```bash
cargo build --release -p webclaw-fetch --example latency_bench

BENCH_URLS=/path/to/urls.txt \
BENCH_CONCURRENCY=8 \
BENCH_LIMIT=200 \
  ./target/release/examples/latency_bench > run.jsonl 2> run.summary
```

| Env | Default | Meaning |
|---|---|---|
| `BENCH_URLS` | *(required)* | File of URLs, one per line |
| `BENCH_CONCURRENCY` | 8 | In-flight requests |
| `BENCH_LIMIT` | all | Cap URLs read |
| `BENCH_TIMEOUT_SECS` | 12 | Per-request timeout |
| `WEBCLAW_PROXY` | unset | Optional proxy |

The URL list is passed in at runtime and is **never committed** — a realistic
corpus is drawn from whatever traffic you actually serve, which is not public
data. Lines may carry extra `|`-separated columns (only the first is read), so a
CSV/DB export can be fed in unchanged; `#` comments and blanks are skipped.

stdout is JSONL (one object per URL: status, timings, bytes, words, error) so
runs can be diffed over time. stderr carries the percentile summary.

## Interpreting a run

The summary prints `p50/p90/p99/max` for fetch, extract, and total, plus:

- **`extract share of measured time`** — if this is low single digits, the
  extractor is not the bottleneck and parser micro-optimisation is wasted
  effort. Attack the network path instead.
- **`200s that would trip the thin-body escalation rule`** — pages returning
  HTTP 200 with under 10 KB of HTML *or* under 50 extracted words. Downstream
  consumers treat these as "not the real page" and re-fetch them through a
  browser, which costs an order of magnitude more than the direct fetch. This
  count is the size of that exposure *before* any escalation runs.

That last number is worth splitting by cause when it looks high. A page tripping
on **word count with a large HTML body** is a client-rendered shell — the static
HTML genuinely has no content, and a browser render is the correct answer. A
page tripping on **byte size while carrying real words** would be a false
positive. The two demand opposite fixes, so measure before concluding.

## Comparing two egress paths

Two traps make A/B egress comparisons produce confident nonsense. Both were hit
on the first real run here.

**Fast failures win races.** A 403 challenge page arrives quickly, carries a
body, and raises no transport error. Score it as a success and the egress path
that gets *blocked* looks faster than the one served the real page. One such row
produced an apparent "28× speedup" that was a block page timed against a real
one. Latency percentiles here therefore cover 2xx only, and non-2xx is counted
separately — but that is a floor, not a ceiling. When comparing two runs, also
require **body sizes within ~10%** per URL before believing the pair; a soft
block returns 200 with a much smaller body and slips past a status check.

**The noise floor is bigger than most effects.** Four runs of an *unchanged*
configuration over the same 200 URLs spanned 1.8× in mean and 2.8× in max, with
per-URL max/min across identical runs reaching ~10× at p90. A single-run
difference under roughly 2× carries no information, and a single-URL difference
of any size carries none at all. To make a comparison mean something:

- **Interleave the arms** per URL (A/B/A/B within the same short window) instead
  of running all of A then all of B — sequential order alone can manufacture a
  result.
- **Include a null arm** — A vs a second instance of A. Any A-vs-B effect has to
  beat the A-vs-A effect to count.
- **Deduplicate to backends, not hostnames.** Distinct hostnames routinely share
  one origin or anycast endpoint, so four URLs behind one backend is one
  observation. Resolve first and keep one URL per backend.
- **Report the paired median ratio**, not the difference of aggregate p50s. A
  single multi-MB URL can supply more than the entire gap between two arms.

Note that a large-object transfer difference is a *bandwidth* question wearing a
latency costume; separate it rather than letting it drive a p99.

## Reading the tail

Latency here is heavily tail-weighted: on a representative corpus a low
single-digit percentage of URLs can account for roughly a third of total fetch
time, and `p99` tends to sit at whatever `BENCH_TIMEOUT_SECS` is set to — that
is the timeout wall, not a measurement. Compare `p50` for typical cost and count
requests at the timeout separately; averaging across both hides each.
