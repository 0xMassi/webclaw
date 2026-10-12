"""Benchmark orchestrator, driven by ../run.sh. Runs under .work/venv-base (needs tiktoken).

  run.py prepare                      freeze each non-PDF case's HTML into .work/corpus/
  run.py run <label> [runs] [arm...]  every arm: url mode x runs, html mode x 1
  run.py summary <label>              results/<label>/summary.md
"""
import json, os, re, statistics, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CASES = ROOT / "cases.jsonl"
HTML_CASES = WORK / "cases-html.jsonl"
ARM_VENV = {"webclaw-llm": "base", "webclaw-md": "base", "trafilatura": "base",
            "scrapling": "scrapling", "crawl4ai-raw": "crawl4ai", "crawl4ai-fit": "crawl4ai"}
WEBCLAW = os.environ["WEBCLAW_BIN"]


def prepare():
    corpus = WORK / "corpus"
    corpus.mkdir(parents=True, exist_ok=True)
    html_cases = []
    for c in map(json.loads, open(CASES)):
        if c["category"] == "pdf":
            continue
        path = corpus / f"{c['id']}.html"
        if not path.exists():
            probe = subprocess.run([WEBCLAW, c["url"], "-f", "llm", "-t", "30"], capture_output=True, text=True, timeout=60)
            http_error = "returned HTTP" in probe.stderr or "request failed" in probe.stderr
            r = None if http_error else subprocess.run([WEBCLAW, c["url"], "--raw-html", "-t", "30"], capture_output=True, text=True, timeout=60)
            if r and r.returncode == 0 and r.stdout.strip():
                path.write_text(r.stdout)
        if path.exists():
            html_cases.append({**c, "html_path": f"corpus/{c['id']}.html"})
    HTML_CASES.write_text("".join(json.dumps(c) + "\n" for c in html_cases))
    print(f"froze {len(html_cases)} html cases into {corpus}")


def run(label, runs, arms):
    res = ROOT / "results" / label
    out = WORK / "out" / label
    res.mkdir(parents=True, exist_ok=True)
    def arm_worker(arm):
        py = str(WORK / f"venv-{ARM_VENV[arm]}" / "bin/python")
        for mode, cases, n in (("url", CASES, runs), ("html", HTML_CASES, 1)):
            code = subprocess.run([py, str(ROOT / "scripts/arms.py"), arm, mode, str(cases), str(res / "rows.jsonl"),
                                   str(out / mode / arm), str(n)]).returncode
            if code:
                return f"{arm} {mode} exited {code}"

    with ThreadPoolExecutor(len(arms)) as pool:
        failures = [f for f in pool.map(arm_worker, arms) if f]
    if failures:
        sys.exit("benchmark workers failed: " + "; ".join(failures))
    subprocess.run([WEBCLAW, "--version"], stdout=open(res / "webclaw-version.txt", "w"))


def hits(text, facts):
    low = text.lower()
    n = 0
    for f in facts:
        fl = f.lower()
        if " " in f or not f.isalpha():
            n += fl in low
        else:
            n += bool(re.search(r"\b" + re.escape(fl) + r"\b", low))
    return n


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p * (len(xs) - 1))))] if xs else 0


def summary(label):
    import tiktoken
    enc = tiktoken.get_encoding("cl100k_base")
    res = ROOT / "results" / label
    out = WORK / "out" / label
    cases = {c["id"]: c for c in map(json.loads, open(CASES))}
    groups = {}
    rows = list(map(json.loads, open(res / "rows.jsonl")))
    for row in rows:
        text = (out / row["mode"] / row["arm"] / f"{row['id']}-{row['run']}.md").read_text()
        c = cases[row["id"]]
        row.update(hits=hits(text, c["facts"]), nfacts=len(c["facts"]),
                   tokens=len(enc.encode(text, disallowed_special=())))
        for cat in (c["category"], "ALL"):
            groups.setdefault((row["mode"], cat, row["arm"]), []).append(row)
    lines = [f"# Benchmark {label}", "", f"webclaw: {(res / 'webclaw-version.txt').read_text().strip()}",
             "Tool versions: [versions.txt](versions.txt)", "",
             "Fact recall counts failed runs as 0 hits. Tokens: cl100k median over successful runs. Seconds: wall time per URL.", ""]
    for mode in ("url", "html"):
        lines += [f"## {mode} mode", "", "| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |",
                  "|---|---|---:|---:|---:|---:|---:|---:|"]
        for (m, cat, arm), group in sorted(groups.items(), key=lambda kv: (kv[0][1] != "ALL", kv[0][1], kv[0][2])):
            if m != mode:
                continue
            ok = [r for r in group if r["ok"]]
            secs = [r["seconds"] for r in group]
            lines.append(f"| {cat} | {arm} | {len(group)} | {100 * len(ok) / len(group):.0f} | "
                         f"{100 * sum(r['hits'] for r in group) / max(1, sum(r['nfacts'] for r in group)):.1f} | "
                         f"{int(statistics.median([r['tokens'] for r in ok])) if ok else 0} | "
                         f"{pct(secs, .5):.2f} | {pct(secs, .95):.2f} |")
        lines.append("")
    (res / "rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    (res / "summary.md").write_text("\n".join(lines))
    print("\n".join(lines))


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "prepare":
        prepare()
    elif cmd == "run":
        run(sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 3, sys.argv[4:] or list(ARM_VENV))
    elif cmd == "summary":
        summary(sys.argv[2])
