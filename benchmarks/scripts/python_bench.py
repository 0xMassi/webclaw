"""Python package benchmark, driven by ../python.sh. Runs under .work/venv-base (needs psutil).

usage: python_bench.py <label> <webclaw wheel>
Puts every tool's wheels in its own local folder, installs each tool from that folder into fresh venvs with an
empty uv cache (offline, so the network is out of the timing), runs its documented post-install step into a fresh
browser directory, then calls it through its Python API on the static golden pages: a cold first call in a new
process, then a steady loop.
"""
import json, os, shlex, shutil, statistics, subprocess, sys, threading, time
from collections import Counter
from pathlib import Path

import psutil

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
BASE = WORK / "python-install"
HOUSE = WORK / "python-wheelhouse"
PINS = {l.split()[1]: shlex.split(l)[2:] for l in open(ROOT / "envs.sh") if l.startswith("venv ")}
STATIC = [c for c in map(json.loads, open(ROOT / "cases.jsonl")) if c["category"] not in ("spa", "pdf")]
FIRST = [STATIC[i * len(STATIC) // 5] for i in range(5)]
ROUNDS = 2
INSTALL_RUNS = 3
ARMS = ["webclaw", "scrapling", "crawl4ai"]
POST = {"scrapling": ["scrapling", "install"], "crawl4ai": ["crawl4ai-setup"]}


def env_for(arm):
    d = BASE / arm
    return {**os.environ, "UV_CACHE_DIR": str(d / "uv-cache"), "PLAYWRIGHT_BROWSERS_PATH": str(d / "browsers"),
            "CRAWL4_AI_BASE_DIRECTORY": str(d / "home")}


def size_mb(path):
    if not path.exists():
        return 0.0
    out = subprocess.run(["/usr/bin/du", "-sk", str(path)], capture_output=True, text=True).stdout
    return int(out.split()[0]) / 1024


def wheelhouse(arm, wheel):
    house = HOUSE / arm
    if arm == "webclaw":
        shutil.rmtree(house, ignore_errors=True)
        house.mkdir(parents=True)
        shutil.copy(wheel, house)
        return house, ["webclaw-cli"]
    names = [spec.split(" @ ")[0] for spec in PINS[arm]]
    if not any(house.glob("*.whl")):
        pip = HOUSE / "pip-venv"
        if not (pip / "bin/python").exists():
            subprocess.run(["uv", "venv", "-q", "--seed", "-p", "3.12", str(pip)], check=True)
        subprocess.run([str(pip / "bin/python"), "-m", "pip", "wheel", "-q", "-w", str(house), *PINS[arm]], check=True)
    return house, names


def install(arm, wheel):
    house, names = wheelhouse(arm, wheel)
    d = BASE / arm
    times = []
    for _ in range(INSTALL_RUNS):
        shutil.rmtree(d, ignore_errors=True)
        (d / "home").mkdir(parents=True)
        env = env_for(arm)
        t = time.time()
        subprocess.run(["uv", "venv", "-q", "-p", "3.12", str(d / "venv")], env=env, check=True)
        subprocess.run(["uv", "pip", "install", "-q", "--no-index", "--find-links", str(house), "-p", str(d / "venv"),
                        *names], env=env, check=True)
        times.append(time.time() - t)
        shutil.rmtree(d / "uv-cache")
    listed = subprocess.run(["uv", "pip", "list", "-p", str(d / "venv"), "--format", "json"], env=env,
                            capture_output=True, text=True, check=True).stdout
    packages = {p["name"].lower(): p["version"] for p in json.loads(listed)}
    row = {"arm": arm, "version": packages.get({"webclaw": "webclaw-cli"}.get(arm, arm), "?"),
           "install_s": round(statistics.median(times), 2), "download_mb": round(size_mb(house), 1),
           "packages": len(packages), "venv_mb": round(size_mb(d / "venv"), 1), "post_s": 0.0, "other_mb": 0.0}
    rows = [row]
    if arm == "scrapling":
        rows.insert(0, {**row, "arm": "scrapling-nobrowser"})
    if arm in POST:
        t = time.time()
        r = subprocess.run([str(d / "venv/bin" / POST[arm][0]), *POST[arm][1:]], env=env, capture_output=True,
                           text=True)
        if r.returncode:
            sys.exit(f"{arm} post-install failed:\n{r.stdout[-2000:]}\n{r.stderr[-2000:]}")
        row.update(post_s=round(time.time() - t, 1), other_mb=round(size_mb(d / "browsers") + size_mb(d / "home"), 1))
    return rows


def tree_rss(proc):
    try:
        procs = [proc, *proc.children(recursive=True)]
    except psutil.NoSuchProcess:
        return 0
    total = 0
    for p in procs:
        try:
            total += p.memory_info().rss
        except psutil.NoSuchProcess:
            pass
    return total


def run(arm, args, timeout):
    cmd = [str(BASE / arm / "venv/bin/python"), str(ROOT / "scripts/python_arms.py"), arm, *args]
    t = time.time()
    proc = subprocess.Popen(cmd, env=env_for(arm), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    peak, done = 0, threading.Event()

    def sample():
        nonlocal peak
        ps = psutil.Process(proc.pid)
        while not done.is_set():
            peak = max(peak, tree_rss(ps))
            time.sleep(0.02)

    sampler = threading.Thread(target=sample)
    sampler.start()
    try:
        out, err = proc.communicate(timeout=timeout)
    finally:
        if proc.poll() is None:
            proc.kill()
        done.set()
        sampler.join()
    rows = [json.loads(l) for l in out.splitlines() if l.startswith("{")]
    if proc.returncode or not rows:
        print(f"{arm} {args[0]}: exit {proc.returncode}\n{err[-2000:]}", file=sys.stderr)
    return rows, time.time() - t, peak / 2**20


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p * (len(xs) - 1))))] if xs else 0


def main(label, wheel):
    from run import hits
    facts = {c["url"]: c["facts"] for c in STATIC}
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    urls = WORK / "python-urls.json"
    urls.write_text(json.dumps([c["url"] for c in STATIC]))
    installs, latency, raw = [], [], []
    for arm in ARMS:
        installs += install(arm, wheel)
        print(json.dumps(installs[-1]), flush=True)
    for arm in ARMS:
        firsts = []
        for case in FIRST:
            rows, wall, peak = run(arm, ["first", case["url"]], 300)
            row = rows[0] if rows else {"url": case["url"], "ok": False, "import_s": 0, "call_s": 0, "text": ""}
            firsts.append({**row, "wall_s": round(wall, 3), "peak_mb": round(peak, 1)})
        rows, wall, peak = run(arm, ["steady", str(urls), str(ROUNDS)], 3600)
        for r in firsts + rows:
            r["hits"], r["nfacts"] = hits(r.pop("text"), facts[r["url"]]), len(facts[r["url"]])
            raw.append({"arm": arm, "mode": "steady" if "round" in r else "first", **r})
        secs = [r["call_s"] for r in rows]
        latency.append({
            "arm": arm,
            "first_wall_s": statistics.median(r["wall_s"] for r in firsts),
            "first_setup_s": statistics.median(r["import_s"] for r in firsts),
            "first_peak_mb": statistics.median(r["peak_mb"] for r in firsts),
            "calls": len(rows), "ok": sum(r["ok"] for r in rows),
            "p50_s": pct(secs, .5), "p95_s": pct(secs, .95),
            "recall": 100 * sum(r["hits"] for r in rows) / max(1, sum(r["nfacts"] for r in rows)),
            "steady_peak_mb": round(peak, 1)})
        print(json.dumps(latency[-1]), flush=True)
    (res / "python-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in installs + latency + raw))
    names = {"webclaw": "webclaw-cli (local wheel)", "scrapling-nobrowser": "Scrapling, no browser download",
             "scrapling": "Scrapling, with `scrapling install`", "crawl4ai": "Crawl4AI, with `crawl4ai-setup`"}
    lines = [f"# Python packages {label}", "",
             f"Each tool's wheels are first saved in a local folder (`download MB`). Then the tool is installed from "
             f"that folder with no network and an empty uv cache, into a fresh Python 3.12 venv, {INSTALL_RUNS} times "
             "(median). The documented post-install step runs once after that, over the network: `scrapling install` "
             "and `crawl4ai-setup` download Chromium into a fresh browser directory. Scrapling's plain `Fetcher`, "
             "used below, works without that download, so it also has a row without it. `other MB` is the browser "
             "directory plus Crawl4AI's home directory. webclaw renders with a Chrome or Chromium already installed "
             "on the machine, which is not counted here.", "",
             "| tool | version | download MB | install s | post-install s | packages | venv MB | other MB | total MB |",
             "|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for r in installs:
        lines.append(f"| {names[r['arm']]} | {r['version']} | {r['download_mb']:.0f} | {r['install_s']:.2f} | "
                     f"{r['post_s']} | {r['packages']} | {r['venv_mb']:.0f} | {r['other_mb']:.0f} | "
                     f"{r['venv_mb'] + r['other_mb']:.0f} |")
    names["scrapling"], names["crawl4ai"] = "Scrapling `Fetcher`", "Crawl4AI `AsyncWebCrawler`"
    lines += ["", f"Calls on the {len(STATIC)} static golden pages (`cases.jsonl` without `spa` and `pdf`). "
              f"`first call` starts a new Python process that imports the tool and scrapes one page, median of "
              f"{len(FIRST)} pages; `setup` is the import plus creating the client (Crawl4AI starts its browser "
              f"there). The steady loop keeps one process, makes one warm-up call, then calls each page "
              f"{ROUNDS} times. webclaw returns `llm` text, Scrapling `markdown()`, Crawl4AI `raw_markdown`. "
              "Peak RSS sums the Python process and all its children, sampled every 20 ms.", "",
              "| tool | first call s | setup s | first call peak MB | steady ok | steady p50 s | steady p95 s | "
              "fact recall % | steady peak MB |",
              "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for r in latency:
        lines.append(f"| {names[r['arm']]} | {r['first_wall_s']:.2f} | {r['first_setup_s']:.2f} | "
                     f"{r['first_peak_mb']:.0f} | {r['ok']}/{r['calls']} | {r['p50_s']:.2f} | {r['p95_s']:.2f} | "
                     f"{r['recall']:.1f} | {r['steady_peak_mb']:.0f} |")
    steady = [r for r in raw if r["mode"] == "steady"]
    by_call = {}
    for r in steady:
        by_call.setdefault((r["url"], r["round"]), {})[r["arm"]] = r
    common = [calls for calls in by_call.values() if all(calls[a]["ok"] for a in ARMS)]
    lines += ["", f"Fact recall on the {len(common)} steady calls every tool answered: " + ", ".join(
        f"{names[a]} {100 * sum(c[a]['hits'] for c in common) / max(1, sum(c[a]['nfacts'] for c in common)):.1f}%"
        for a in ARMS) + ".", "", "Failed steady calls (both rounds), with the error each tool reported:", ""]
    for a in ARMS:
        failed = Counter((r["url"], (r["error"] or "empty output")[:120]) for r in steady if r["arm"] == a and not r["ok"])
        lines += [f"- {names[a]}: " + ("none" if not failed else "; ".join(
            f"{url} ({n}x): {error}" for (url, error), n in sorted(failed.items())))]
    (res / "python-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    shutil.rmtree(BASE)


sys.path.insert(0, str(ROOT / "scripts"))
os.environ.setdefault("WEBCLAW_BIN", str(BASE / "webclaw/venv/bin/webclaw"))
main(sys.argv[1], sys.argv[2])
