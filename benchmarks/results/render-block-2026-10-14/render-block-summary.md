# Rendering with resources blocked render-block-2026-10-14

Each tool renders 20 script-heavy pages in one browser it keeps open, 3 rounds, with its resource blocking off and on, every request going through a local proxy that counts the bytes it carries (TLS bytes for HTTPS). webclaw renders through its MCP `scrape` tool with render `always` (an HTTP fetch first, then Chrome with that response); Scrapling through `AsyncDynamicSession(network_idle=True)`; Crawl4AI through one `AsyncWebCrawler`. `facts` counts the golden facts found in the output; `words` is the median word count. `lost` lists pages whose median word count fell below 90% of the same tool with blocking off, or that lost a fact.

| tool | option | setting | ok | ms (median) | ms (p95) | KB/page (median) | KB total | facts | words (median) |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| webclaw | `block` | none | 60/60 | 1291 | 2722 | 1038 | 95688 | 137/180 | 425 |
| webclaw | `block` | images,fonts,media | 60/60 | 1416 | 3813 | 619 | 76431 | 136/180 | 425 |
| scrapling | `disable_resources` | off | 60/60 | 1589 | 31091 | 1279 | 291153 | 156/180 | 614 |
| scrapling | `disable_resources` | on | 60/60 | 1384 | 30769 | 861 | 380470 | 156/180 | 614 |
| crawl4ai | `text_mode` | off | 60/60 | 708 | 1142 | 957 | 68710 | 150/180 | 622 |
| crawl4ai | `text_mode` | on | 60/60 | 762 | 1142 | 572 | 53367 | 150/180 | 622 |

Pages that lost content against the same tool with blocking off:

- webclaw images,fonts,media: https://www.shopify.com (words 635 -> 623, facts 11 -> 10)
- scrapling on: none
- crawl4ai on: none

Median ms per page:

| page | webclaw none | webclaw images,fonts,media | scrapling off | scrapling on | crawl4ai off | crawl4ai on |
|---|---:|---:|---:|---:|---:|---:|
| https://quotes.toscrape.com/js/ | 1086 | 1099 | 1043 | 1161 | 774 | 1057 |
| https://quotes.toscrape.com/js/page/2/ | 1085 | 1103 | 1032 | 1191 | 725 | 942 |
| https://quotes.toscrape.com/js/page/3/ | 1076 | 1112 | 1049 | 1179 | 721 | 926 |
| https://quotes.toscrape.com/js/page/4/ | 1078 | 1125 | 1044 | 1181 | 729 | 1053 |
| https://demo.playwright.dev/todomvc/ | 976 | 805 | 654 | 790 | 418 | 337 |
| https://todomvc.com/examples/react/dist/ | 822 | 756 | 1166 | 1027 | 414 | 292 |
| https://todomvc.com/examples/vue/dist/ | 865 | 890 | 739 | 935 | 292 | 291 |
| https://hn.algolia.com/ | 1271 | 1381 | 1601 | 1549 | 775 | 959 |
| https://angular.dev/tutorials/learn-angular | 1096 | 1012 | 31061 | 30746 | 587 | 585 |
| https://vercel.com | 1838 | 1872 | 3213 | 1434 | 602 | 726 |
| https://www.notion.com | 1613 | 1997 | 2617 | 1583 | 686 | 690 |
| https://stripe.com | 2647 | 2168 | 2280 | 2732 | 1181 | 1045 |
| https://react.dev | 1178 | 1278 | 1092 | 1018 | 553 | 524 |
| https://nextjs.org/docs | 1783 | 1770 | 2114 | 2597 | 586 | 517 |
| https://techcrunch.com | 2045 | 1745 | 3486 | 2923 | 781 | 794 |
| https://www.shopify.com | 1311 | 1434 | 1883 | 1840 | 534 | 505 |
| https://anthropic.com | 1501 | 1438 | 1562 | 1415 | 804 | 806 |
| https://books.toscrape.com/ | 2103 | 1933 | 1398 | 1313 | 1142 | 994 |
| https://www.bbc.com/news | 1678 | 1562 | 1872 | 1901 | 577 | 668 |
| https://www.theverge.com/ | 5453 | 5230 | 31399 | 31378 | 786 | 995 |
