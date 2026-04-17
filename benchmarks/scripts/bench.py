#!/usr/bin/env python3
"""
webclaw benchmark — webclaw vs trafilatura vs firecrawl.

Produces results/YYYY-MM-DD.json matching the schema in methodology.md.
Sites and facts come from ../sites.txt and ../facts.json.
Tokenizer: cl100k_base (GPT-4 / GPT-3.5 / text-embedding-3-*).

Usage:
    FIRECRAWL_API_KEY=fc-...  python3 bench.py
    python3 bench.py  # runs webclaw + trafilatura only

Optional env:
    WEBCLAW                 path to webclaw release binary (default: ../../target/release/webclaw)
    RUNS                    runs per site (default: 3)
    WEBCLAW_TIMEOUT         seconds (default: 30)
"""
from __future__ import annotations
import json, os, re, statistics, subprocess, sys, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent  # benchmarks/
REPO_ROOT = ROOT.parent  # core/

WEBCLAW = os.environ.get("WEBCLAW", str(REPO_ROOT / "target" / "release" / "webclaw"))
RUNS = int(os.environ.get("RUNS", "3"))
WC_TIMEOUT = int(os.environ.get("WEBCLAW_TIMEOUT", "30"))

try:
    import tiktoken
    import trafilatura
except ImportError as e:
    sys.exit(f"missing dep: {e}. run: pip install tiktoken trafilatura firecrawl-py")

ENC = tiktoken.get_encoding("cl100k_base")

FC_KEY = os.environ.get("FIRECRAWL_API_KEY")
FC = None
if FC_KEY:
    try:
        from firecrawl import Firecrawl
        FC = Firecrawl(api_key=FC_KEY)
    except ImportError:
        print("firecrawl-py not installed; skipping firecrawl column", file=sys.stderr)


def load_sites() -> list[str]:
    path = ROOT / "sites.txt"
    out = []
    for line in path.read_text().splitlines():
        s = line.split("#", 1)[0].strip()
        if s:
            out.append(s)
    return out


def load_facts() -> dict[str, list[str]]:
    return json.loads((ROOT / "facts.json").read_text())["facts"]


def run_webclaw_llm(url: str) -> tuple[str, float]:
    t0 = time.time()
    r = subprocess.run(
        [WEBCLAW, url, "-f", "llm", "-t", str(WC_TIMEOUT)],
        capture_output=True, text=True, timeout=WC_TIMEOUT + 15,
    )
    return r.stdout or "", time.time() - t0


def run_webclaw_raw(url: str) -> str:
    r = subprocess.run(
        [WEBCLAW, url, "--raw-html", "-t", str(WC_TIMEOUT)],
        capture_output=True, text=True, timeout=WC_TIMEOUT + 15,
    )
    return r.stdout or ""


def run_trafilatura(url: str) -> tuple[str, float]:
    t0 = time.time()
    try:
        html = trafilatura.fetch_url(url)
        out = ""
        if html:
            out = trafilatura.extract(
                html, output_format="markdown",
                include_links=True, include_tables=True, favor_recall=True,
            ) or ""
    except Exception:
        out = ""
    return out, time.time() - t0


def run_firecrawl(url: str) -> tuple[str, float]:
    if not FC:
        return "", 0.0
    t0 = time.time()
    try:
        r = FC.scrape(url, formats=["markdown"])
        return (r.markdown or ""), time.time() - t0
    except Exception:
        return "", time.time() - t0


def tok(s: str) -> int:
    return len(ENC.encode(s, disallowed_special=())) if s else 0


_WORD = re.compile(r"[A-Za-z][A-Za-z0-9]*")

def hit_count(text: str, facts: list[str]) -> int:
    """Case-insensitive; word-boundary for single-token alphanumeric facts,
    substring for multi-word or non-alpha facts (like '99.999')."""
    if not text:
        return 0
    low = text.lower()
    count = 0
    for f in facts:
        f_low = f.lower()
        if " " in f or not f.isalpha():
            if f_low in low:
                count += 1
        else:
            if re.search(r"\b" + re.escape(f_low) + r"\b", low):
                count += 1
    return count


def main() -> int:
    sites = load_sites()
    facts_by_url = load_facts()
    print(f"running {len(sites)} sites × {3 if FC else 2} tools × {RUNS} runs")
    if not FC:
        print("  (no FIRECRAWL_API_KEY — skipping firecrawl column)")
    print()

    per_site = []
    for i, url in enumerate(sites, 1):
        facts = facts_by_url.get(url, [])
        if not facts:
            print(f"[{i}/{len(sites)}] {url}  SKIPPED — no facts in facts.json")
            continue
        print(f"[{i}/{len(sites)}] {url}")
        raw_t = tok(run_webclaw_raw(url))

        def run_one(fn):
            out, seconds = fn(url)
            return {"tokens": tok(out), "facts": hit_count(out, facts), "seconds": seconds}

        runs = {"webclaw": [], "trafilatura": [], "firecrawl": []}
        for _ in range(RUNS):
            runs["webclaw"].append(run_one(run_webclaw_llm))
            runs["trafilatura"].append(run_one(run_trafilatura))
            if FC:
                runs["firecrawl"].append(run_one(run_firecrawl))
            else:
                runs["firecrawl"].append({"tokens": 0, "facts": 0, "seconds": 0.0})

        def med(tool, key):
            return statistics.median(r[key] for r in runs[tool])

        def med_ints(tool):
            return {
                "tokens_med":  int(med(tool, "tokens")),
                "facts_med":   int(med(tool, "facts")),
                "seconds_med": round(med(tool, "seconds"), 2),
            }

        per_site.append({
            "url": url,
            "facts_count": len(facts),
            "raw_tokens": raw_t,
            "webclaw":     med_ints("webclaw"),
            "trafilatura": med_ints("trafilatura"),
            "firecrawl":   med_ints("firecrawl"),
        })
        last = per_site[-1]
        print(f"   raw={raw_t}  wc={last['webclaw']['tokens_med']}/{last['webclaw']['facts_med']}"
              f"  tr={last['trafilatura']['tokens_med']}/{last['trafilatura']['facts_med']}"
              f"  fc={last['firecrawl']['tokens_med']}/{last['firecrawl']['facts_med']}")

    # aggregates
    total_facts = sum(r["facts_count"] for r in per_site)

    def agg(tool):
        red_vals = [
            (r["raw_tokens"] - r[tool]["tokens_med"]) / r["raw_tokens"] * 100
            for r in per_site
            if r["raw_tokens"] > 0 and r[tool]["tokens_med"] > 0
        ]
        return {
            "reduction_mean":   round(statistics.mean(red_vals), 1) if red_vals else 0.0,
            "reduction_median": round(statistics.median(red_vals), 1) if red_vals else 0.0,
            "facts_preserved":  sum(r[tool]["facts_med"] for r in per_site),
            "total_facts":      total_facts,
            "fidelity_pct":     round(sum(r[tool]["facts_med"] for r in per_site) / total_facts * 100, 1) if total_facts else 0,
            "latency_mean":     round(statistics.mean(r[tool]["seconds_med"] for r in per_site), 2),
        }

    result = {
        "timestamp":           time.strftime("%Y-%m-%d %H:%M:%S"),
        "webclaw_version":     subprocess.check_output([WEBCLAW, "--version"], text=True).strip().split()[-1],
        "trafilatura_version": trafilatura.__version__,
        "firecrawl_enabled":   FC is not None,
        "tokenizer":           "cl100k_base",
        "runs_per_site":       RUNS,
        "site_count":          len(per_site),
        "total_facts":         total_facts,
        "aggregates":          {t: agg(t) for t in ["webclaw", "trafilatura", "firecrawl"]},
        "per_site":            per_site,
    }

    out_path = ROOT / "results" / f"{time.strftime('%Y-%m-%d')}.json"
    out_path.parent.mkdir(exist_ok=True)
    out_path.write_text(json.dumps(result, indent=2))

    print()
    print("=" * 70)
    print(f"{len(per_site)} sites, {total_facts} facts, median of {RUNS} runs")
    print("=" * 70)
    for t in ["webclaw", "trafilatura", "firecrawl"]:
        a = result["aggregates"][t]
        print(f"  {t:14s}  reduction_mean={a['reduction_mean']:5.1f}%"
              f"  fidelity={a['facts_preserved']}/{a['total_facts']} ({a['fidelity_pct']}%)"
              f"  latency={a['latency_mean']}s")
    print()
    print(f"  results → {out_path.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
