import asyncio, json, os, secrets, signal, socket, statistics, subprocess, sys, time
from pathlib import Path

import httpx2
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
URLS = ["https://books.toscrape.com/", "https://quotes.toscrape.com/", "https://www.scrapethissite.com/pages/simple/"]
ROUNDS = 5


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def servers(token):
    wc, sc = free_port(), free_port()
    return {
        "webclaw": {"cmd": [os.environ["WEBCLAW_MCP_BIN"], "--http", f"127.0.0.1:{wc}"],
                    "env": {"WEBCLAW_MCP_TOKEN": token}, "url": f"http://127.0.0.1:{wc}/mcp",
                    "tool": "scrape", "args": lambda u: {"url": u, "format": "markdown"}},
        "scrapling": {"cmd": [str(WORK / "venv-mcp/bin/scrapling"), "mcp", "--http", "--host", "127.0.0.1",
                              "--port", str(sc), "--auth-token", token], "env": {}, "url": f"http://127.0.0.1:{sc}/mcp",
                      "tool": "make_request", "args": lambda u: {"url": u, "extraction_type": "markdown"}},
    }


async def call(server, token, url):
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    t0 = time.perf_counter()
    async with httpx2.AsyncClient(headers=headers, timeout=60) as http, \
            streamable_http_client(server["url"], http_client=http) as streams:
        async with ClientSession(streams[0], streams[1]) as session:
            await session.initialize()
            t1 = time.perf_counter()
            result = await session.call_tool(server["tool"], server["args"](url))
            t2 = time.perf_counter()
    text = "".join(getattr(c, "text", "") for c in result.content)
    return {"handshake_ms": 1000 * (t1 - t0), "call_ms": 1000 * (t2 - t1), "ok": not result.is_error and len(text) > 200}


def refused(server):
    initialize = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                  "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "b", "version": "0"}}}
    response = httpx2.post(server["url"], json=initialize, headers={"Accept": "application/json, text/event-stream"})
    return response.status_code == 401


def wait_up(url):
    host, port = url.split("//")[1].split("/")[0].split(":")
    for _ in range(100):
        with socket.socket() as s:
            if s.connect_ex((host, int(port))) == 0:
                return
        time.sleep(0.2)
    sys.exit(f"server at {url} did not start")


def main(label):
    token = secrets.token_urlsafe(16)
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for name, server in servers(token).items():
        proc = subprocess.Popen(server["cmd"], env={**os.environ, **server["env"]},
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            wait_up(server["url"])
            calls = [asyncio.run(call(server, token, url)) for _ in range(ROUNDS) for url in URLS]
            rows.append({"server": name, "calls": len(calls), "ok": sum(c["ok"] for c in calls),
                         "handshake_ms": statistics.median(c["handshake_ms"] for c in calls),
                         "call_ms": statistics.median(c["call_ms"] for c in calls),
                         "call_p95_ms": sorted(c["call_ms"] for c in calls)[int(0.95 * (len(calls) - 1))],
                         "refuses_no_token": refused(server)})
            print(json.dumps(rows[-1]), flush=True)
        finally:
            proc.send_signal(signal.SIGINT)
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
    (res / "mcp-http-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# MCP over HTTP {label}", "",
             f"Each server runs its streamable-HTTP transport on loopback with a bearer token. The official MCP "
             f"Python client opens a fresh session per call (initialize, then one fetch-and-extract tool call) for "
             f"{len(URLS)} pages x {ROUNDS} rounds. `ok` counts calls that returned page text without a tool error. "
             "`refuses no token` checks that a client without the token is turned away. Crawl4AI has no standalone "
             "MCP server to compare.", "",
             "| server | calls | ok | handshake ms (median) | tool call ms (median) | tool call ms (p95) | refuses no token |",
             "|---|---:|---:|---:|---:|---:|---|"]
    for r in rows:
        lines.append(f"| {r['server']} | {r['calls']} | {r['ok']} | {r['handshake_ms']:.0f} | {r['call_ms']:.0f} | "
                     f"{r['call_p95_ms']:.0f} | {'yes' if r['refuses_no_token'] else 'no'} |")
    (res / "mcp-http-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
