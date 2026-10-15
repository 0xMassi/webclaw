import json, os, shutil, statistics, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
ARMS = {"webclaw": "venv-base", "scrapling": "venv-scrapling", "crawl4ai": "venv-crawl4ai"}
KEEPERS = {"scrapling-restore": "venv-scrapling", "scrapling-cookies": "venv-scrapling",
           "crawl4ai-state": "venv-crawl4ai"}
CRAWL_URL = "https://books.toscrape.com/"
CRAWL_PAGES = 50
RUNS = int(os.environ.get("PROFILE_RUNS", "5"))
TIMED_URL = "https://quotes.toscrape.com/js/"
STORAGE_URL = "https://example.com/"

LOGINS = [
    {"name": "quotes.toscrape.com", "cookie": "session",
     "login": "https://quotes.toscrape.com/login", "check": "https://quotes.toscrape.com/",
     "steps": [{"type": "type", "selector": "#username", "value": "webclaw"},
               {"type": "type", "selector": "#password", "value": "bench"},
               {"type": "click", "selector": "input[type=submit]"}],
     "marker": "Logout"},
    {"name": "practice.expandtesting.com", "cookie": "session",
     "login": "https://practice.expandtesting.com/login", "check": "https://practice.expandtesting.com/secure",
     "steps": [{"type": "type", "selector": "#username", "value": "practice"},
               {"type": "type", "selector": "#password", "value": "SuperSecretPassword!"},
               {"type": "click", "selector": "button[type=submit]"}],
     "marker": "Hi, practice!"}
]

SET_STORAGE = [{"type": "executeJavascript", "code": (
    "localStorage.setItem('webclaw-bench', 'local-kept'); "
    "await new Promise((done, fail) => { const r = indexedDB.open('webclaw-bench', 1); "
    "r.onupgradeneeded = () => r.result.createObjectStore('s'); "
    "r.onsuccess = () => { const t = r.result.transaction('s', 'readwrite'); "
    "t.objectStore('s').put('indexed-kept', 'k'); t.oncomplete = () => { r.result.close(); done(); }; "
    "t.onerror = fail; }; r.onerror = fail; }); return 'stored';")}]
GET_STORAGE = [{"type": "executeJavascript", "code": (
    "const indexed = await new Promise(done => { const r = indexedDB.open('webclaw-bench', 1); "
    "r.onupgradeneeded = () => r.result.createObjectStore('s'); "
    "r.onsuccess = () => { const db = r.result; if (!db.objectStoreNames.contains('s')) return done(null); "
    "const g = db.transaction('s').objectStore('s').get('k'); g.onsuccess = () => done(g.result ?? null); "
    "g.onerror = () => done(null); }; r.onerror = () => done(null); }); "
    "return [localStorage.getItem('webclaw-bench'), indexed];")}]


def page(arm, profile, url, steps=None, hold=0, background=False):
    cmd = [str(WORK / {**ARMS, **KEEPERS}[arm] / "bin/python"), str(ROOT / "scripts/browser_profile_arms.py"),
           arm, str(profile) if profile else "-", url, json.dumps(steps) if steps else "-"]
    cmd += [str(hold)] if hold else []
    if background:
        return subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    t0 = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
    return {**parse(r.stdout, r.stderr), "ms": 1000 * (time.perf_counter() - t0)}


def parse(stdout, stderr):
    for line in stdout.splitlines()[::-1]:
        if line.startswith("{"):
            return json.loads(line)
    return {"text": None, "values": [], "ms": None, "error": (stderr or "no output").strip()[-300:]}


def size_mb(path):
    total = sum(f.stat().st_size for f in Path(path).rglob("*") if f.is_file() and not f.is_symlink())
    return round(total / 1e6, 1)


def fresh(arm, what):
    path = WORK / "profile-out" / arm / what
    shutil.rmtree(path, ignore_errors=True)
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def has(row, marker):
    return bool(row["text"]) and marker in row["text"]


def logins(arm, rows):
    for case in LOGINS:
        profile = fresh(arm, "login-" + case["name"])
        login = page(arm, profile, case["login"], case["steps"])
        later = page(arm, profile, case["check"])
        control = page(arm, fresh(arm, "control"), case["check"])
        row = {"test": "login", "arm": arm, "site": case["name"], "cookie": case["cookie"],
               "logged_in": has(login, case["marker"]), "kept": has(later, case["marker"]),
               "control_logged_out": not has(control, case["marker"]) and not control["error"],
               "error": login["error"] or later["error"]}
        rows.append(row)
        print(json.dumps(row), flush=True)


def storage(arm, rows):
    profile = fresh(arm, "storage")
    stored = page(arm, profile, STORAGE_URL, SET_STORAGE)
    read = page(arm, profile, STORAGE_URL, GET_STORAGE)
    values = (read["values"] or [[None, None]])[0] or [None, None]
    row = {"test": "storage", "arm": arm, "stored": stored["values"] == ["stored"],
           "local_storage": values[0] == "local-kept", "indexed_db": values[1] == "indexed-kept",
           "error": stored["error"] or read["error"]}
    rows.append(row)
    print(json.dumps(row), flush=True)


def timing(arm, rows):
    warm = fresh(arm, "warm")
    page(arm, warm, TIMED_URL)
    times = {"fresh": [], "warm": [], "none": []}
    errors = []
    for i in range(RUNS):
        for kind in times:
            profile = fresh(arm, f"fresh-{i}") if kind == "fresh" else warm if kind == "warm" else None
            got = page(arm, profile, TIMED_URL)
            if got["error"] or not has(got, "Albert Einstein"):
                errors.append(f"{kind}: {got['error'] or 'no quotes'}")
            else:
                times[kind].append(got["ms"])
    row = {"test": "timing", "arm": arm, "runs": RUNS,
           **{f"{k}_ms": round(statistics.median(v)) if v else None for k, v in times.items()},
           "fresh_mb": size_mb(fresh_path(arm)), "warm_mb": size_mb(warm), "warm_renders": RUNS + 1,
           "error": "; ".join(errors[:3]) or None}
    rows.append(row)
    print(json.dumps(row), flush=True)


def crawl(rows):
    times = {"none": [], "fresh": [], "warm": []}
    warm = fresh("webclaw", "crawl-warm")
    pages = {}
    for i in range(3):
        for kind in times:
            cmd = [os.environ["WEBCLAW_BIN"], CRAWL_URL, "--crawl", "--depth", "2", "--max-pages", str(CRAWL_PAGES),
                   "-f", "jsonl"]
            if kind != "none":
                cmd += ["--browser-profile", str(fresh("webclaw", f"crawl-{i}") if kind == "fresh" else warm)]
            t0 = time.perf_counter()
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=900)
            times[kind].append(1000 * (time.perf_counter() - t0))
            pages[kind] = sum(1 for line in r.stdout.splitlines() if line.startswith("{"))
    row = {"test": "crawl", "arm": "webclaw", "pages": pages,
           **{f"{k}_ms": round(statistics.median(v)) for k, v in times.items()}}
    rows.append(row)
    print(json.dumps(row), flush=True)


def fresh_path(arm):
    return WORK / "profile-out" / arm / "fresh-0"


def lock(arm, rows):
    profile = fresh(arm, "lock")
    holder = page(arm, profile, STORAGE_URL, hold=20, background=True)
    time.sleep(10)
    second = page(arm, profile, STORAGE_URL)
    out, err = holder.communicate(timeout=300)
    first = parse(out, err)
    row = {"test": "lock", "arm": arm, "first_ok": not first["error"],
           "second": "refused" if second["error"] else
                     "browser refused, HTTP page kept" if "in use by another browser" in second.get("log", "") else
                     "ran on the profile in use",
           "message": " ".join((second["error"] or "").replace(str(profile), "DIR").split())[:240],
           "second_ms": round(second["ms"] or 0)}
    rows.append(row)
    print(json.dumps(row), flush=True)


def yes(v):
    return "yes" if v else "no"


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for arm in ARMS:
        logins(arm, rows)
        storage(arm, rows)
        timing(arm, rows)
        lock(arm, rows)
    for arm in KEEPERS:
        logins(arm, rows)
        storage(arm, rows)
    crawl(rows)
    (res / "profile-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    by = lambda test: [r for r in rows if r["test"] == test]
    lines = [f"# Browser profile {label}", "",
             "A login in one process, then the logged-in page in a second process on the same profile.", "",
             "| tool | site | login cookie | logged in | still logged in next run | logged out without the profile |",
             "| --- | --- | --- | --- | --- | --- |"]
    lines += [f"| {r['arm']} | {r['site']} | {r['cookie']} | {yes(r['logged_in'])} | {yes(r['kept'])} | "
              f"{yes(r['control_logged_out'])} |" for r in by("login")]
    lines += ["", "Values stored by a script in one process, read in the next.", "",
              "| tool | stored | localStorage kept | IndexedDB kept |", "| --- | --- | --- | --- |"]
    lines += [f"| {r['arm']} | {yes(r['stored'])} | {yes(r['local_storage'])} | {yes(r['indexed_db'])} |"
              for r in by("storage")]
    lines += ["", f"Median wall time of one process, start to exit, rendering {TIMED_URL}, {RUNS} runs each, and the profile size.",
              "", "| tool | no profile | new profile | warm profile | new profile size | size after "
              f"{RUNS + 1} renders |", "| --- | --- | --- | --- | --- | --- |"]
    lines += [f"| {r['arm']} | {r['none_ms']} ms | {r['fresh_ms']} ms | {r['warm_ms']} ms | {r['fresh_mb']} MB | "
              f"{r['warm_mb']} MB |" for r in by("timing")]
    for r in by("crawl"):
        lines += ["", f"Median wall time of `webclaw {CRAWL_URL} --crawl --depth 2 --max-pages {CRAWL_PAGES}`, 3 runs: "
                  "with a profile every page loads in the browser.", "",
                  "| no profile | new profile | warm profile | pages |", "| --- | --- | --- | --- |",
                  f"| {r['none_ms']} ms | {r['fresh_ms']} ms | {r['warm_ms']} ms | "
                  f"{r['pages']['none']} / {r['pages']['fresh']} / {r['pages']['warm']} |"]
    lines += ["", "A second process on a profile while the first still has it open.", "",
              "| tool | second process | message |", "| --- | --- | --- |"]
    lines += [f"| {r['arm']} | {r['second']} | {r['message'].replace('|', '/')} |" for r in by("lock")]
    errors = [r for r in rows if r.get("error")]
    if errors:
        lines += ["", "Errors:", ""] + [f"- {r['arm']} {r['test']} {r.get('site', '')}: {r['error']}" for r in errors]
    (res / "profile-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
