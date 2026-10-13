import json, shutil, subprocess, sys, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
ARMS = {"webclaw": "venv-base", "scrapling": "venv-scrapling", "crawl4ai": "venv-crawl4ai"}
URLS = [json.loads(l)["url"] for l in (ROOT / "cases.jsonl").read_text().splitlines() if l.strip()]


def run(arm, cache, mode):
    out = Path(cache).parent / f"{arm}-{mode}.json"
    r = subprocess.run([str(WORK / ARMS[arm] / "bin/python"), str(ROOT / "scripts/cache_arms.py"), arm, cache, mode,
                        str(out), *URLS], capture_output=True, text=True, timeout=3600)
    if not out.exists():
        sys.exit(f"{arm} {mode} wrote nothing (exit {r.returncode}):\n{r.stderr[-3000:]}")
    return json.loads(out.read_text())


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for arm in ARMS:
        tmp = tempfile.mkdtemp(prefix=f"cache-{arm}-")
        cache = str(Path(tmp) / "cache")
        cold, warm = run(arm, cache, "cold"), run(arm, cache, "warm")
        fetched = [u for u in URLS if cold["pages"].get(u)]
        row = {"arm": arm, "pages": len(URLS), "cold_ok": len(fetched), "cold_s": cold["seconds"],
               "warm_s": warm["seconds"], "warm_served": sum(bool(warm["pages"].get(u)) for u in fetched),
               "identical": sum(warm["pages"].get(u) == cold["pages"][u] for u in fetched)}
        rows.append(row)
        print(json.dumps(row), flush=True)
        shutil.rmtree(tmp, ignore_errors=True)
    (res / "cache-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Response cache {label}", "",
             f"Each tool fetches the {len(URLS)} url-mode cases twice through its own disk cache, starting empty: "
             "webclaw `--cache -f llm --render never` (one process per page, 8 at a time), Scrapling's Spider in "
             "`development_mode`, Crawl4AI `CacheMode.ENABLED` with `arun_many`. The second run sends every "
             "request to a proxy that refuses connections, so `served from cache` counts pages answered without "
             "the network. `identical` counts second-run pages whose text matches the first run.", "",
             "| tool | first run ok | first run s | second run s | served from cache | identical |",
             "|---|---:|---:|---:|---:|---:|"]
    for r in rows:
        lines.append(f"| {r['arm']} | {r['cold_ok']}/{r['pages']} | {r['cold_s']:.1f} | {r['warm_s']:.1f} | "
                     f"{r['warm_served']}/{r['cold_ok']} | {r['identical']}/{r['cold_ok']} |")
    (res / "cache-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
