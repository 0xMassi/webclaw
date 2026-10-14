"""A local HTTP proxy that counts the bytes it carries from the network to the tool.

`start()` runs it in a background thread and returns (proxy_url, counter, cut); `counter["down"]` is
the running total of response bytes (TLS bytes for CONNECT tunnels) from page hosts, leaving out the
browser's own background services (model and component downloads, autofill, push), which every tool's
Chrome fetches at random moments. `cut()` closes every open connection, so a page's late downloads
never count toward the next page.
"""
import asyncio, threading
from urllib.parse import urlsplit

_LIVE = set()
BROWSER_SERVICES = ("optimizationguide-pa.googleapis.com", "content-autofill.googleapis.com", "update.googleapis.com",
                    "safebrowsingohttpgateway.googleapis.com", "mtalk.google.com", "android.clients.google.com",
                    "clients1.google.com", "clients2.google.com", "accounts.google.com", "ogads-pa.clients6.google.com",
                    ".gvt1.com")


async def _pipe(reader, writer, counter=None, host=None):
    try:
        while data := await reader.read(65536):
            if counter is not None and not (host or "").endswith(BROWSER_SERVICES):
                counter["down"] += len(data)
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def _handle(client_r, client_w, counter):
    _LIVE.add(client_w)
    try:
        head = await client_r.readuntil(b"\r\n\r\n")
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, ConnectionError):
        client_w.close()
        return
    lines = head.decode("latin-1").split("\r\n")
    method, target, version = lines[0].split(" ", 2)
    try:
        if method == "CONNECT":
            host, port = target.rsplit(":", 1)
            up_r, up_w = await asyncio.open_connection(host, int(port))
            _LIVE.add(up_w)
            client_w.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            await client_w.drain()
        else:
            url = urlsplit(target)
            up_r, up_w = await asyncio.open_connection(url.hostname, url.port or 80)
            _LIVE.add(up_w)
            path = (url.path or "/") + (f"?{url.query}" if url.query else "")
            headers = [l for l in lines[1:] if l and not l.lower().startswith(("connection:", "proxy-connection:", "keep-alive:"))]
            up_w.write(f"{method} {path} {version}\r\n".encode("latin-1")
                       + "".join(h + "\r\n" for h in headers).encode("latin-1") + b"Connection: close\r\n\r\n")
            length = next((int(h.split(":", 1)[1]) for h in headers if h.lower().startswith("content-length:")), 0)
            if length:
                up_w.write(await client_r.readexactly(length))
            await up_w.drain()
            await _pipe(up_r, client_w, counter, url.hostname)
            up_w.close()
            return
    except (OSError, ValueError, asyncio.IncompleteReadError):
        client_w.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        client_w.close()
        return
    await asyncio.gather(_pipe(client_r, up_w), _pipe(up_r, client_w, counter, host))


def start():
    counter = {"down": 0}
    ready = threading.Event()
    box = {}

    def run():
        loop = asyncio.new_event_loop()
        server = loop.run_until_complete(
            asyncio.start_server(lambda r, w: _handle(r, w, counter), "127.0.0.1", 0, limit=1 << 20))
        box["port"] = server.sockets[0].getsockname()[1]
        box["loop"] = loop
        ready.set()
        loop.run_forever()

    def cut():
        def close_all():
            for writer in list(_LIVE):
                writer.transport.abort()
            _LIVE.clear()
        box["loop"].call_soon_threadsafe(close_all)

    threading.Thread(target=run, daemon=True).start()
    ready.wait()
    return f"http://127.0.0.1:{box['port']}", counter, cut
