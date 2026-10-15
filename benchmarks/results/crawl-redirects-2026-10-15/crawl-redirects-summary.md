# Crawl redirects benchmark crawl-redirects-2026-10-15

Each arm crawls each seed one level deep (cap 300 pages, concurrency 4), robots.txt obeyed. The expected pages are the seed page's same-host links, followed to their final URL, that robots.txt allows before and after the redirect. `found` counts expected pages the arm returned content for, under the final URL or a link that redirects to it. `duplicates` counts pages returned twice under such spellings. Medians over rounds.

| site | links on seed | redirecting | arm | rounds | found | pages with content | duplicates | seconds |
|---|---:|---:|---|---:|---:|---:|---:|---:|
| quotes.toscrape.com | 47 | 8 | webclaw-before | 3 | 38/46 | 39 | 0 | 2.6 |
| quotes.toscrape.com | 47 | 8 | webclaw | 3 | 46/46 | 47 | 0 | 3.0 |
| quotes.toscrape.com | 47 | 8 | scrapling | 3 | 46/46 | 47 | 0 | 2.8 |
| quotes.toscrape.com | 47 | 8 | crawl4ai | 3 | 46/46 | 48 | 1 | 12.1 |
| quotes.toscrape.com-http | 52 | 10 | webclaw-before | 3 | 42/52 | 43 | 0 | 2.5 |
| quotes.toscrape.com-http | 52 | 10 | webclaw | 3 | 52/52 | 53 | 0 | 2.8 |
| quotes.toscrape.com-http | 52 | 10 | scrapling | 3 | 52/52 | 53 | 0 | 3.0 |
| quotes.toscrape.com-http | 52 | 10 | crawl4ai | 3 | 52/52 | 53 | 0 | 11.9 |
| www.python.org | 65 | 5 | webclaw-before | 3 | 57/60 | 58 | 0 | 0.8 |
| www.python.org | 65 | 5 | webclaw | 3 | 59/60 | 61 | 0 | 0.8 |
| www.python.org | 65 | 5 | scrapling | 3 | 59/60 | 64 | 2 | 1.4 |
| www.python.org | 65 | 5 | crawl4ai | 3 | 59/60 | 96 | 4 | 45.3 |
| go.dev | 110 | 36 | webclaw-before | 3 | 86/94 | 87 | 0 | 9.5 |
| go.dev | 110 | 36 | webclaw | 3 | 94/94 | 95 | 0 | 9.9 |
| go.dev | 110 | 36 | scrapling | 3 | 94/94 | 111 | 7 | 12.6 |
| go.dev | 110 | 36 | crawl4ai | 3 | 93/94 | 116 | 8 | 37.6 |
