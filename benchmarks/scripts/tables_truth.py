import asyncio, json, sys
from pathlib import Path

from playwright.async_api import async_playwright

CORPUS, OUT = Path(sys.argv[1]), Path(sys.argv[2])
URLS = json.loads((CORPUS / "urls.json").read_text())

GRID = r"""
() => {
  const norm = s => (s || '').replace(/[\s ]+/g, ' ').trim();
  const vis = el => el.checkVisibility();
  return [...document.querySelectorAll('table')].map(t => {
    const rows = [...t.rows].filter(vis);
    const g = rows.map(() => []);
    const head = [], single = [];
    rows.forEach((tr, r) => {
      const cells = [...tr.cells].filter(vis);
      head.push(cells.length > 0 && (tr.parentNode.tagName === 'THEAD' || cells.every(c => c.tagName === 'TH')));
      single.push(cells.length === 1 ? cells[0].colSpan : 0);
      const section = rows.filter(x => x.parentNode === tr.parentNode);
      const left = section.length - section.indexOf(tr);
      let c = 0;
      for (const cell of cells) {
        while (g[r][c] !== undefined) c++;
        const rs = cell.rowSpan === 0 ? left : Math.max(1, Math.min(cell.rowSpan, left));
        const text = norm(cell.innerText);
        for (let dr = 0; dr < rs; dr++) for (let dc = 0; dc < cell.colSpan; dc++) g[r + dr][c + dc] = text;
        c += cell.colSpan;
      }
    });
    const width = Math.max(0, ...g.map(r => r.length));
    return {grid: g.map(r => Array.from({length: width}, (_, i) => r[i] === undefined ? '' : r[i])), head, single};
  });
}
"""


def serve(body):
    served = []

    async def route(route):
        req = route.request
        if req.is_navigation_request() and not served:
            served.append(1)
            await route.fulfill(status=200, body=body, content_type="text/html; charset=utf-8")
        elif req.resource_type in ("image", "media", "font", "script"):
            await route.abort()
        else:
            await route.continue_()

    return route


async def main():
    out = {}
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        ctx = await browser.new_context(java_script_enabled=False, viewport={"width": 1280, "height": 900})
        for slug, url in URLS.items():
            body = (CORPUS / f"{slug}.html").read_bytes()
            page = await ctx.new_page()
            await page.route("**/*", serve(body))
            try:
                await page.goto(url, wait_until="load", timeout=45000)
            except Exception as e:
                print(f"{slug}: load did not finish, reading the page as it is ({str(e)[:80]})", file=sys.stderr)
            out[slug] = await page.evaluate(GRID)
            await page.close()
        await browser.close()
    OUT.write_text(json.dumps(out))


asyncio.run(main())
