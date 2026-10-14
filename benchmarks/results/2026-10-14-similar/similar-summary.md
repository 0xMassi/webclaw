# Find-similar benchmark 2026-10-14-similar

23 list pages frozen into `.work/similar-corpus/`. Each page has a hand-written CSS selector for its items (titles, names or headlines); the items it picks are the ground truth. The example is the second item's text. `text found` counts pages where the first element the tool's exact, case-insensitive text search returns has that text. `from text` runs find-similar on that element; `from css` runs it on the first item picked by the ground-truth selector. Precision and recall compare the texts of the returned elements (the example included) with the ground truth, averaged over pages; `exact` counts pages with both at 1. `reusable selector` counts pages where the selector the tool suggests picks the same elements again. Seconds are wall time for the `from text` run over all pages; webclaw runs its CLI once per page, and `of which start` is the time the same number of `webclaw --version` runs takes.

| arm | text found | from text P | from text R | from text exact | from css P | from css R | from css exact | reusable selector | seconds | of which start |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 23/23 | 0.991 | 0.958 | 19/23 | 0.991 | 1.000 | 20/23 | 23/23 | 0.25 | 0.09 |
| scrapling | 18/23 | 0.911 | 0.819 | 9/23 | 0.899 | 0.886 | 13/23 | 0/23 | 0.14 | - |
| crawl4ai | no equivalent (its schema generator needs an LLM call) | - | - | - | - | - | - | - | - | - |

Per page (precision/recall from text, then from css):

| page | items | webclaw | scrapling |
|---|---:|---|---|
| books | 20 | 1.00/1.00, 1.00/1.00 | 1.00/0.80, 1.00/0.75 |
| books-travel | 11 | 1.00/1.00, 1.00/1.00 | 1.00/0.64, 1.00/0.45 |
| quotes | 10 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| quotes-love | 10 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| hn | 30 | 1.00/1.00, 1.00/1.00 | 0.28/0.87, 0.28/0.87 |
| hn-newest | 30 | 1.00/1.00, 1.00/1.00 | 0.97/1.00, 0.97/1.00 |
| lobsters | 25 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| gh-trending | 12 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |
| gh-trending-rust | 11 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |
| wiki-proglang | 678 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| wiki-countries | 240 | 1.00/0.04, 1.00/1.00 | 1.00/0.04, 1.00/1.00 |
| countries | 250 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |
| hockey | 25 | 1.00/1.00, 1.00/1.00 | 0.16/1.00, 0.16/1.00 |
| ws-laptops | 117 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| ws-phones | 6 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| scrapeme | 16 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| scrapingcourse | 16 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| arxiv | 50 | 1.00/1.00, 1.00/1.00 | -, 0.28/1.00 |
| devto | 19 | 1.00/1.00, 1.00/1.00 | 1.00/0.05, 1.00/0.05 |
| cnn-lite | 100 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| npr-text | 20 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| bbc | 87 | 0.96/1.00, 0.95/1.00 | -, 0.99/0.92 |
| guardian | 117 | 0.84/1.00, 0.84/1.00 | 1.00/0.34, 1.00/0.34 |

Suggested selectors:

| page | ground truth | suggested |
|---|---|---|
| books | `article.product_pod h3 a` | `article.product_pod > h3 > a` |
| books-travel | `article.product_pod h3 a` | `article.product_pod > h3 > a` |
| quotes | `div.quote span.text` | `span.text` |
| quotes-love | `small.author` | `small.author` |
| hn | `span.titleline > a` | `span.titleline > a` |
| hn-newest | `span.titleline > a` | `span.titleline > a` |
| lobsters | `a.u-url` | `a.u-url` |
| gh-trending | `article.Box-row h2 a` | `h2.h3.lh-condensed > a.Link` |
| gh-trending-rust | `article.Box-row h2 a` | `h2.h3.lh-condensed > a.Link` |
| wiki-proglang | `div.div-col li > a` | `div.mw-content-ltr.mw-parser-output > section > div > ul > li > a` |
| wiki-countries | `table.wikitable > tbody > tr > td:first-child > a` | `span.l-label > a` |
| countries | `h3.country-name` | `h3.country-name` |
| hockey | `tr.team td.name` | `td.name` |
| ws-laptops | `a.title` | `a.title` |
| ws-phones | `a.title` | `a.title` |
| scrapeme | `h2.woocommerce-loop-product__title` | `h2.woocommerce-loop-product__title` |
| scrapingcourse | `h2.product-name` | `h2.product-name.woocommerce-loop-product__title` |
| arxiv | `div.list-title` | `div.list-title.mathjax` |
| devto | `h2.crayons-story__title a` | `a.crayons-story__hidden-navigation-link` |
| cnn-lite | `li.card--lite a` | `li.card--lite > a` |
| npr-text | `a.topic-title` | `a.topic-title` |
| bbc | `[data-testid=promo] h3` | `a.exn3ah94 > span` |
| guardian | `.card-headline .headline-text` | `span.headline-text.show-underline` |
