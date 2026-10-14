# Find-similar benchmark 2026-10-14-similar-v2

43 list pages frozen into `.work/similar-corpus/`: the 23 `tuned` pages the widening rule was developed on, and 20 `held-out` pages chosen and scored by a separate review, never used to tune it. Each page has a hand-written CSS selector for its items (titles, names or headlines); the items it picks are the ground truth. The example is the second item's text. `text found` counts pages where the first element the tool's exact, case-insensitive text search returns has that text. `from text` runs find-similar on that element; `from css` runs it on the first item picked by the ground-truth selector. Precision and recall compare the texts of the returned elements (the example included) with the ground truth, averaged over pages, a page with no result counting as 0; `exact` counts pages with both at 1. `reusable selector` counts pages where the selector the tool suggests picks the same elements again. Seconds are wall time for the `from text` runs; webclaw runs its CLI once per page, and `of which start` is the time the same number of `webclaw --version` runs takes.

## tuned pages

| arm | text found | from text P | from text R | from text exact | from css P | from css R | from css exact | reusable selector | seconds | of which start |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 23/23 | 0.991 | 0.958 | 20/23 | 0.991 | 1.000 | 21/23 | 23/23 | 0.24 | 0.10 |
| scrapling | 18/23 | 0.713 | 0.641 | 9/23 | 0.899 | 0.886 | 13/23 | 0/23 | 0.14 | - |
| crawl4ai | no equivalent (its schema generator needs an LLM call) | - | - | - | - | - | - | - | - | - |

## held-out pages

| arm | text found | from text P | from text R | from text exact | from css P | from css R | from css exact | reusable selector | seconds | of which start |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 20/20 | 0.914 | 0.922 | 13/20 | 0.968 | 0.957 | 15/20 | 20/20 | 0.25 | 0.09 |
| scrapling | 18/20 | 0.793 | 0.852 | 11/20 | 0.952 | 0.877 | 14/20 | 0/20 | 0.14 | - |
| crawl4ai | no equivalent (its schema generator needs an LLM call) | - | - | - | - | - | - | - | - | - |

## all pages

| arm | text found | from text P | from text R | from text exact | from css P | from css R | from css exact | reusable selector | seconds | of which start |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 43/43 | 0.955 | 0.941 | 33/43 | 0.980 | 0.980 | 36/43 | 43/43 | 0.49 | 0.19 |
| scrapling | 36/43 | 0.750 | 0.739 | 20/43 | 0.924 | 0.882 | 27/43 | 0/43 | 0.28 | - |
| crawl4ai | no equivalent (its schema generator needs an LLM call) | - | - | - | - | - | - | - | - | - |

## Per page

Precision/recall from text, then from css.

| page | set | items | webclaw | scrapling |
|---|---|---:|---|---|
| books | tuned | 20 | 1.00/1.00, 1.00/1.00 | 1.00/0.80, 1.00/0.75 |
| books-travel | tuned | 11 | 1.00/1.00, 1.00/1.00 | 1.00/0.64, 1.00/0.45 |
| quotes | tuned | 10 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| quotes-love | tuned | 10 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| hn | tuned | 30 | 1.00/1.00, 1.00/1.00 | 0.28/0.87, 0.28/0.87 |
| hn-newest | tuned | 30 | 1.00/1.00, 1.00/1.00 | 0.97/1.00, 0.97/1.00 |
| lobsters | tuned | 25 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| gh-trending | tuned | 12 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |
| gh-trending-rust | tuned | 11 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |
| wiki-proglang | tuned | 678 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| wiki-countries | tuned | 240 | 1.00/0.04, 1.00/1.00 | 1.00/0.04, 1.00/1.00 |
| countries | tuned | 250 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |
| hockey | tuned | 25 | 1.00/1.00, 1.00/1.00 | 0.16/1.00, 0.16/1.00 |
| ws-laptops | tuned | 117 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| ws-phones | tuned | 6 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| scrapeme | tuned | 16 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| scrapingcourse | tuned | 16 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| arxiv | tuned | 50 | 1.00/1.00, 1.00/1.00 | -, 0.28/1.00 |
| devto | tuned | 19 | 1.00/1.00, 1.00/1.00 | 1.00/0.05, 1.00/0.05 |
| cnn-lite | tuned | 100 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| npr-text | tuned | 20 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| bbc | tuned | 87 | 0.96/1.00, 0.95/1.00 | -, 0.99/0.92 |
| guardian | tuned | 117 | 0.84/1.00, 0.84/1.00 | 1.00/0.34, 1.00/0.34 |
| librs-search | held-out | 146 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| ddg-html | held-out | 10 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| py-docs-lib | held-out | 34 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| mdn-elements | held-out | 135 | 0.65/1.00, 0.57/1.00 | 0.65/1.00, 0.58/0.99 |
| go-std | held-out | 323 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| docsrs-regex-all | held-out | 44 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| phpbb-forum | held-out | 52 | 1.00/1.00, 1.00/1.00 | 0.96/1.00, 0.96/1.00 |
| wwr-jobs | held-out | 55 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| python-jobs | held-out | 25 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| iana-root | held-out | 1595 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| worldometers | held-out | 234 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| de-wiki-staedte | held-out | 80 | 0.33/1.00, 1.00/1.00 | 0.33/1.00, 1.00/1.00 |
| lemonde | held-out | 60 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |
| repubblica | held-out | 188 | 0.93/0.29, 1.00/1.00 | 0.91/0.74, 1.00/0.01 |
| elpais | held-out | 14 | 1.00/0.14, 1.00/0.14 | 0.50/0.36, 1.00/0.50 |
| gh-releases | held-out | 10 | 0.50/1.00, 0.91/1.00 | 1.00/1.00, 1.00/1.00 |
| slashdot | held-out | 15 | 1.00/1.00, 1.00/1.00 | 1.00/1.00, 1.00/1.00 |
| craigslist | held-out | 353 | 1.00/1.00, 1.00/1.00 | 0.50/1.00, 0.50/1.00 |
| habr | held-out | 21 | 0.88/1.00, 0.88/1.00 | 1.00/0.95, 1.00/0.05 |
| zh-douban | held-out | 10 | 1.00/1.00, 1.00/1.00 | -, 1.00/1.00 |

## Suggested selectors

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
| wiki-proglang | `div.div-col li > a` | `div.div-col > ul > li > a` |
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
| librs-search | `li h4` | `div.h > h4` |
| ddg-html | `a.result__a` | `a.result__a` |
| py-docs-lib | `div.toctree-wrapper > ul > li > a` | `li.toctree-l1 > a.internal.reference` |
| mdn-elements | `table td:first-child a` | `figure.table-container > table > tbody > tr > td > a > code` |
| go-std | `div.UnitDirectories-subdirectory a` | `div.UnitDirectories-subdirectory > span > a` |
| docsrs-regex-all | `ul.all-items li a` | `ul.all-items > li > a` |
| phpbb-forum | `a.topictitle` | `a.topictitle` |
| wwr-jobs | `span.new-listing__header__title__text` | `span.new-listing__header__title__text` |
| python-jobs | `span.listing-company-name > a` | `span.listing-company-name > a` |
| iana-root | `table#tld-table tbody tr td:first-child a` | `span.domain > a` |
| worldometers | `table tbody tr td:nth-child(2) a` | `td.border-b.border-e.border-zinc-200.font-bold.px-2.py-1\.5.text-start > a.hover\:text-primary\/80.text-primary.transition-colors` |
| de-wiki-staedte | `table.wikitable tbody tr td:nth-child(2) a` | `table.wikitable > tbody > tr > td > a` |
| lemonde | `.teaser__title` | `h3.teaser__title` |
| repubblica | `.entry__title` | `section.block.block__layout-A-8.is-hard-news > div.block__grid > div.block__item > article.entry > div.entry__content > h2.entry__title > a` |
| elpais | `article h2 a` | `div.b-d_a > article.c.c--m-n.c-d > header.c_h > h2.c_t > a` |
| gh-releases | `section h2.sr-only` | `span.ActionListItem-label` |
| slashdot | `span.story-title > a:first-child` | `span.story-title > a` |
| craigslist | `li.cl-static-search-result div.title` | `div.title` |
| habr | `h2.tm-title a span` | `a.tm-title__link > span` |
| zh-douban | `div.pl2 > a` | `div.pl2 > a` |
