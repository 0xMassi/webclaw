import asyncio, base64, hashlib, json, os, re, statistics, subprocess, sys, time, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
OUT = WORK / "archive-out"
URLS = [
    "https://example.com/",
    "https://quotes.toscrape.com/js/",
    "https://books.toscrape.com/",
    "https://en.wikipedia.org/wiki/Web_scraping",
    "https://news.ycombinator.com/",
    "https://www.rust-lang.org/",
    "https://docs.python.org/3/library/asyncio.html",
    "https://github.com/rust-lang/rust",
    "https://developer.mozilla.org/en-US/docs/Web/HTML",
    "https://www.bbc.com/news",
]
CRAWL = ("https://books.toscrape.com/", 30)
PYTHON = {"webclaw": {"cold": "venv-base", "warm": "venv-mcp"},
          "crawl4ai": {"cold": "venv-crawl4ai", "warm": "venv-crawl4ai"}}
UNSUPPORTED_MHTML = {
    "scrapling": "no MHTML or other page archive: its fetchers, sessions and MCP tools return pages, text and screenshots only",
}
UNSUPPORTED_WARC = {
    "crawl4ai": "no WARC output: it archives a page only as MHTML (`capture_mhtml`), and its crawls return results, not HTTP records",
    "scrapling": "no WARC output: its spider writes items, and responses are not kept as HTTP records",
}
ONLY = os.environ.get("ARCHIVE_ONLY")
WARM_ROUNDS = 2
WORD = re.compile(r"[^\W\d_]{4,}")
MEASURE = """() => ({text: document.body ? document.body.innerText : '',
  images: [...document.images].filter(i => i.complete && i.naturalWidth > 0).length})"""


def words(text):
    return set(WORD.findall(text.lower()))


async def live_reference(urls):
    from playwright.async_api import async_playwright
    ref = {}
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        for url in urls:
            context = await browser.new_context(viewport={"width": 1280, "height": 720})
            page = await context.new_page()
            try:
                await page.goto(url, wait_until="load", timeout=60000)
                try:
                    await page.wait_for_load_state("networkidle", timeout=10000)
                except Exception:
                    pass
                await page.wait_for_timeout(1000)
                seen = await page.evaluate(MEASURE)
                ref[url] = {"words": sorted(words(seen["text"])), "images": seen["images"]}
            except Exception as e:
                ref[url] = {"words": [], "images": 0, "error": str(e)[-200:]}
            await context.close()
        await browser.close()
    return ref


def reference(label):
    path = OUT / f"reference-{label}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        return json.loads(path.read_text())
    ref = asyncio.run(live_reference(URLS))
    path.write_text(json.dumps(ref))
    return ref


async def load_offline(path):
    from playwright.async_api import async_playwright
    async def gate(route):
        await (route.continue_() if route.request.url.startswith("file:") else route.abort())

    async with async_playwright() as p:
        browser = await p.chromium.launch()
        context = await browser.new_context(offline=True, viewport={"width": 1280, "height": 720})
        await context.route("**/*", gate)
        page = await context.new_page()
        await page.goto(path.resolve().as_uri(), wait_until="load", timeout=30000)
        await page.wait_for_timeout(500)
        seen = await page.evaluate(MEASURE)
        await browser.close()
    return seen


def score_mhtml(line, ref):
    got = json.loads(line)
    if not got["file"]:
        return {**got, "ok": False}
    path = Path(got["file"])
    data = path.read_bytes()
    try:
        if b"multipart/related" not in data[:2000]:
            raise ValueError("not an MHTML file")
        seen = asyncio.run(load_offline(path))
    except Exception as e:
        path.unlink()
        return {**got, "ok": False, "error": got["error"] or f"unreadable MHTML: {str(e)[-200:]}"}
    path.unlink()
    live = ref[got["url"]]
    want, found = live["words"], words(seen["text"])
    return {**got, "ok": bool(found), "kb": len(data) / 1024,
            "recall": sum(w in found for w in want) / len(want) if want else None,
            "images": seen["images"], "live_images": live["images"]}


def run_mhtml(arm, mode, ref):
    python = WORK / PYTHON[arm][mode] / "bin/python"
    cmd = [str(python), str(ROOT / "scripts/archive_arms.py"), arm, mode, str(OUT / f"{arm}-{mode}")]
    rounds = WARM_ROUNDS if mode == "warm" else 1
    rows = []
    for _ in range(rounds):
        r = subprocess.run(cmd + URLS, capture_output=True, text=True, timeout=1800)
        done = [score_mhtml(l, ref) for l in r.stdout.splitlines() if l.startswith("{")]
        seen = {p["url"] for p in done}
        missing = [u for u in URLS if u not in seen]
        if missing:
            print(f"{arm} {mode}: exit {r.returncode}, {len(missing)} pages missing\n{r.stderr[-2000:]}",
                  file=sys.stderr)
        rows += done + [{"url": u, "ok": False, "error": f"no result, exit {r.returncode}"} for u in missing]
    return rows


B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"


def sha1_b32(data):
    n = int.from_bytes(hashlib.sha1(data).digest(), "big")
    return "sha1:" + "".join(B32[(n >> (5 * i)) & 31] for i in reversed(range(32)))


def read_warc(path):
    data = path.read_bytes()
    members = None
    if path.name.endswith(".gz"):
        members, plain, rest = 0, [], data
        while rest:
            d = zlib.decompressobj(31)
            plain.append(d.decompress(rest))
            if not d.eof:
                raise ValueError("a gzip member is cut short")
            rest = d.unused_data
            members += 1
        data = b"".join(plain)
    records, pos = [], 0
    while pos < len(data):
        end = data.index(b"\r\n\r\n", pos)
        lines = data[pos:end].decode("utf-8").split("\r\n")
        if lines[0] != "WARC/1.1":
            raise ValueError(f"record at byte {pos} starts with {lines[0]!r}")
        fields = {}
        for line in lines[1:]:
            name, sep, value = line.partition(":")
            if not sep:
                raise ValueError(f"bad header line {line!r}")
            fields[name.strip().lower()] = value.strip()
        length = int(fields["content-length"])
        block = data[end + 4:end + 4 + length]
        if len(block) != length or data[end + 4 + length:end + 8 + length] != b"\r\n\r\n":
            raise ValueError(f"record at byte {pos} is cut short")
        records.append((fields, block))
        pos = end + 8 + length
    return records, members


UUID = re.compile(r"^<urn:uuid:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}>$")
DATE = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
STATUS = re.compile(rb"^HTTP/1\.[01] \d{3}( .*)?$")


def validate_warc(path):
    """Problems with a WARC file: structure, required fields, and every block and payload digest."""
    records, members = read_warc(path)
    problems, ids, responses, digests = [], set(), [], 0
    if members is not None and members != len(records):
        problems.append(f"{members} gzip members for {len(records)} records")
    if not records or records[0][0].get("warc-type") != "warcinfo":
        problems.append("the first record is not warcinfo")
    for fields, block in records:
        kind = fields.get("warc-type")
        for name in ("warc-record-id", "warc-date", "warc-type", "content-length"):
            if name not in fields:
                problems.append(f"{kind} record without {name}")
        rid = fields.get("warc-record-id", "")
        if not UUID.match(rid) or rid in ids:
            problems.append(f"bad or repeated record id {rid}")
        ids.add(rid)
        if not DATE.match(fields.get("warc-date", "")):
            problems.append(f"bad date {fields.get('warc-date')}")
        if fields.get("warc-block-digest") != sha1_b32(block):
            problems.append(f"{kind} block digest mismatch")
        digests += 1
        if kind in ("request", "response"):
            if not fields.get("warc-target-uri", "").startswith("http"):
                problems.append(f"{kind} without a target URI")
            if fields.get("content-type") != f"application/http; msgtype={kind}":
                problems.append(f"{kind} content type {fields.get('content-type')}")
        if kind == "response":
            head, sep, payload = block.partition(b"\r\n\r\n")
            if not sep or not STATUS.match(head.split(b"\r\n")[0]):
                problems.append("response without an HTTP status line")
            if fields.get("warc-payload-digest") != sha1_b32(payload):
                problems.append("payload digest mismatch")
            digests += 1
            responses.append((fields["warc-target-uri"], int(head.split(b" ")[1]), len(payload)))
    response_ids = {f["warc-record-id"] for f, _ in records if f.get("warc-type") == "response"}
    info_ids = {f["warc-record-id"] for f, _ in records if f.get("warc-type") == "warcinfo"}
    for fields, _ in records:
        if fields.get("warc-type") != "warcinfo" and fields.get("warc-warcinfo-id") not in info_ids:
            problems.append("record not tied to a warcinfo record")
    for fields, _ in records:
        if fields.get("warc-type") == "request" and fields.get("warc-concurrent-to") not in response_ids:
            problems.append("request record not tied to a response")
    return {"records": len(records), "responses": responses, "digests": digests, "problems": problems}


def timed(cmd):
    t0 = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    return r, 1000 * (time.perf_counter() - t0)


def run_warc():
    bin = os.environ["WEBCLAW_BIN"]
    rows = []
    for i, url in enumerate(URLS):
        file = OUT / f"warc-{i:02d}.warc.gz"
        file.unlink(missing_ok=True)
        _, plain_ms = timed([bin, url, "-f", "text"])
        r, ms = timed([bin, url, "-f", "text", "--warc", str(file)])
        row = {"url": url, "ms": ms, "plain_ms": plain_ms, "ok": False}
        if r.returncode != 0 or not file.exists():
            rows.append({**row, "error": r.stderr.strip()[-200:]})
            continue
        try:
            check = validate_warc(file)
        except Exception as e:
            rows.append({**row, "error": f"unreadable WARC: {e}"})
            continue
        page = [s for _, s, n in check["responses"] if s < 300 and n > 0]
        row.update(kb=file.stat().st_size / 1024, records=check["records"], digests=check["digests"],
                   problems=check["problems"][:3], ok=not check["problems"] and bool(page))
        file.unlink()
        rows.append(row)
    seed, pages = CRAWL
    file = OUT / "warc-crawl.warc.gz"
    file.unlink(missing_ok=True)
    r, ms = timed([bin, seed, "--crawl", "--depth", "2", "--max-pages", str(pages), "-f", "text",
                   "--warc", str(file)])
    crawled = re.search(r"Crawled (\d+) pages \((\d+) ok", r.stderr)
    crawl = {"url": seed, "ms": ms, "ok": False, "pages": int(crawled.group(2)) if crawled else 0}
    if r.returncode == 0 and file.exists():
        check = validate_warc(file)
        targets = {u for u, s, _ in check["responses"] if s < 400}
        crawl.update(kb=file.stat().st_size / 1024, records=check["records"], digests=check["digests"],
                     responses=len(check["responses"]), page_responses=len(targets),
                     problems=check["problems"][:3],
                     ok=not check["problems"] and len(targets) >= crawl["pages"])
    else:
        crawl["error"] = r.stderr.strip()[-200:]
    return rows, crawl


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    ref = reference(label)
    rows, per_page = [], {}
    for mode in ("cold", "warm"):
        for arm in PYTHON:
            if ONLY and ONLY != f"{arm}-{mode}":
                continue
            got = run_mhtml(arm, mode, ref)
            ok = [p for p in got if p["ok"]]
            ms = sorted(p["ms"] for p in ok) or [0]
            recalls = [p["recall"] for p in ok if p["recall"] is not None]
            row = {"arm": arm, "mode": mode, "files": len(got), "ok": len(ok),
                   "ms": statistics.median(ms), "p95_ms": ms[int(0.95 * (len(ms) - 1))],
                   "kb": statistics.median([p["kb"] for p in ok] or [0]),
                   "recall": statistics.mean(recalls) if recalls else 0,
                   "images": sum(p["images"] for p in ok), "live_images": sum(p["live_images"] for p in ok),
                   "errors": [p["error"] for p in got if not p["ok"]][:3]}
            rows.append(row)
            print(json.dumps(row), flush=True)
            if mode == "cold":
                per_page[arm] = {p["url"]: p for p in got}
    warc, crawl = ([], None) if ONLY and ONLY != "warc" else run_warc()
    for w in warc + ([crawl] if crawl else []):
        print(json.dumps(w), flush=True)
    (res / "archive-rows.jsonl").write_text(
        "".join(json.dumps(r) + "\n" for r in rows + [{"warc": w} for w in warc] + [{"crawl": crawl}]))

    lines = [f"# Page archives {label}", "", "## MHTML", "",
             f"Each tool saves {len(URLS)} pages as MHTML. `cold` starts a fresh browser per page, as one command "
             "per archive does: `webclaw --save-mhtml`, and Crawl4AI's `capture_mhtml=True` with a new crawler. "
             "`warm` keeps one browser open for every page: webclaw through its MCP server's `screenshot` tool "
             f"with format `mhtml`, Crawl4AI with one crawler, {WARM_ROUNDS} rounds. Every archive is then opened "
             "from disk (file://) in a fresh headless Chromium with the network off (an offline context that "
             "also aborts every non-file request) and compared with the live page as the same Chromium shows "
             "it at 1280x720 after the load event, network idle (at most 10 s) and 1 s, measured once at the "
             "start. `recall` is the share of the live page's distinct visible words (4+ letters, from "
             "`innerText`) that the offline copy shows. `images` counts loaded `<img>` elements offline against "
             "live, summed over pages.", "",
             "| tool | mode | ok | ms (median) | ms (p95) | KB (median) | recall | images offline / live |",
             "|---|---|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        lines.append(f"| {r['arm']} | {r['mode']} | {r['ok']}/{r['files']} | {r['ms']:.0f} | {r['p95_ms']:.0f} | "
                     f"{r['kb']:.0f} | {r['recall']:.3f} | {r['images']} / {r['live_images']} |")
    lines.append("")
    lines += [f"{arm}: {why}." for arm, why in UNSUPPORTED_MHTML.items()]
    lines += ["", "Cold run, per page: KB, recall, images offline / live:", "",
              "| page | live words | live images | " + " | ".join(per_page) + " |",
              "|---|---:|---:|" + "---|" * len(per_page)]
    for url in URLS:
        cells = []
        for arm in per_page:
            p = per_page[arm].get(url, {})
            cells.append(f"{p['kb']:.0f} KB, {p['recall']:.2f}, {p['images']}/{p['live_images']}"
                         if p.get("ok") else "fail")
        lines.append(f"| {url} | {len(ref[url]['words'])} | {ref[url]['images']} | " + " | ".join(cells) + " |")
    if warc:
        lines += ["", "## WARC", "",
                  "`webclaw URL -f text --warc FILE.warc.gz` on each page, and a crawl of "
                  f"{CRAWL[0]} (`--crawl --depth 2 --max-pages {CRAWL[1]}`). Each file is checked by a WARC reader "
                  "written for this benchmark (Python standard library only): every gzip member holds one "
                  "record, the first record is `warcinfo`, every record has a unique `urn:uuid` id, a UTC date "
                  "and a matching `Content-Length`, every block digest and every response payload digest "
                  "(SHA-1, base32) matches, each response starts with an HTTP status line, each request "
                  "record points at its response, and every record points at a `warcinfo` record. `ok` also needs a non-empty 2xx response for the page. "
                  "`ms without` is the same command without `--warc`, run just before.", "",
                  "| page | ok | ms | ms without | KB | records | digests checked |", "|---|---|---:|---:|---:|---:|---:|"]
        for w in warc:
            lines.append(f"| {w['url']} | {'yes' if w['ok'] else 'no: ' + str(w.get('problems') or w.get('error'))} | "
                         f"{w['ms']:.0f} | {w['plain_ms']:.0f} | {w.get('kb', 0):.0f} | {w.get('records', 0)} | "
                         f"{w.get('digests', 0)} |")
        ok = [w for w in warc if w["ok"]]
        lines += ["", f"Pages: {len(ok)}/{len(warc)} ok, median {statistics.median([w['ms'] for w in ok] or [0]):.0f} ms "
                      f"with `--warc` against {statistics.median([w['plain_ms'] for w in ok] or [0]):.0f} ms without."]
        if crawl:
            lines += ["", f"Crawl: {'ok' if crawl['ok'] else 'failed: ' + str(crawl.get('problems') or crawl.get('error'))}, "
                          f"{crawl['pages']} pages crawled in {crawl['ms'] / 1000:.1f} s, "
                          f"{crawl.get('responses', 0)} response records ({crawl.get('page_responses', 0)} distinct "
                          f"URLs answered under 400, the rest robots.txt and redirects), {crawl.get('records', 0)} records, "
                          f"{crawl.get('digests', 0)} digests checked, {crawl.get('kb', 0):.0f} KB."]
        lines += [""] + [f"{arm}: {why}." for arm, why in UNSUPPORTED_WARC.items()]
    (res / "archive-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
