"""One URL discovery run by another tool. Runs inside that tool's venv.

usage: url_seeding_arms.py <arm> <host> <out.json>
Writes {"urls": [...], "seconds": s, "error": str|null}: every URL the tool returned, in its order, duplicates kept.
Crawl4AI logs a failed Common Crawl lookup and carries on with what it has, so the arm wraps its index reader to
record that error.
"""
import asyncio, json, sys, tempfile, time

ARM, HOST, OUT = sys.argv[1:4]
INDEX_ERRORS = []


async def crawl4ai(source):
    from crawl4ai import AsyncUrlSeeder, SeedingConfig
    base = tempfile.mkdtemp(prefix="url-seeding-c4ai-")
    async with AsyncUrlSeeder(base_directory=base, cache_root=base) as seeder:
        from_cc = seeder._from_cc

        async def watched(*args, **kwargs):
            try:
                async for url in from_cc(*args, **kwargs):
                    yield url
            except Exception as e:
                INDEX_ERRORS.append(f"{type(e).__name__}: {e}"[:300])
                raise

        seeder._from_cc = watched
        rows = await seeder.urls(HOST, SeedingConfig(source=source, verbose=False, force=True))
    return [r["url"] for r in rows]


def scrapling_sitemap():
    from scrapling.spiders import SitemapSpider
    urls = []

    class Sitemaps(SitemapSpider):
        name = "bench"
        sitemap_urls = [f"https://{HOST}/robots.txt", f"https://{HOST}/sitemap.xml"]
        logging_level = 40

        @staticmethod
        def _dispatch(response, url, rules):
            urls.append(url)
            return None

    Sitemaps().start()
    return urls


ARMS = {
    "crawl4ai-sitemap": lambda: asyncio.run(crawl4ai("sitemap")),
    "crawl4ai-cc": lambda: asyncio.run(crawl4ai("cc")),
    "crawl4ai-sitemap+cc": lambda: asyncio.run(crawl4ai("sitemap+cc")),
    "scrapling-sitemap": scrapling_sitemap,
}

start = time.time()
urls, error = [], None
try:
    urls = ARMS[ARM]()
except BaseException as e:
    error = f"{type(e).__name__}: {e}"[:300]
error = error or (INDEX_ERRORS[0] if INDEX_ERRORS else None)
with open(OUT, "w") as f:
    json.dump({"urls": urls, "seconds": time.time() - start, "error": error}, f)
