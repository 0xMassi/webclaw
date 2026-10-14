# Rendering with resources blocked render-block-2026-10-14

Each tool renders 20 script-heavy pages in one browser it keeps open, 3 rounds, with its resource blocking off and on, every request going through a local proxy that counts the bytes it carries (TLS bytes for HTTPS). webclaw renders through its MCP `scrape` tool with render `always` (an HTTP fetch first, then Chrome with that response); Scrapling through `AsyncDynamicSession(network_idle=True)`; Crawl4AI through one `AsyncWebCrawler`. `facts` counts the golden facts found in the output; `words` is the median word count. `lost` lists pages whose median word count fell below 90% of the same tool with blocking off, or that lost a fact.

| tool | option | setting | ok | ms (median) | ms (p95) | KB/page (median) | KB total | facts | words (median) |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| webclaw | `block` | none | 60/60 | 1276 | 2832 | 1047 | 91602 | 136/180 | 425 |
| webclaw | `block` | images,fonts,media | 60/60 | 1410 | 2367 | 622 | 78581 | 135/180 | 425 |
| webclaw | `block` | images,fonts,media,stylesheets | 60/60 | 1284 | 2265 | 603 | 76024 | 135/180 | 425 |
| scrapling | `disable_resources` | off | 60/60 | 1499 | 31073 | 1282 | 291907 | 156/180 | 614 |
| scrapling | `disable_resources` | on | 60/60 | 1399 | 31142 | 860 | 383149 | 156/180 | 614 |
| crawl4ai | `text_mode` | off | 60/60 | 702 | 1153 | 955 | 68305 | 150/180 | 621 |
| crawl4ai | `text_mode` | on | 60/60 | 677 | 1054 | 580 | 53046 | 150/180 | 621 |

Pages that lost content against the same tool with blocking off:

- webclaw images,fonts,media: https://www.shopify.com (words 623 -> 623, facts 10 -> 9)
- webclaw images,fonts,media,stylesheets: https://stripe.com (words 2061 -> 1825, facts 15 -> 15); https://www.shopify.com (words 623 -> 623, facts 10 -> 9)
- scrapling on: none
- crawl4ai on: none

Median ms per page:

| page | webclaw none | webclaw images,fonts,media | webclaw images,fonts,media,stylesheets | scrapling off | scrapling on | crawl4ai off | crawl4ai on |
|---|---:|---:|---:|---:|---:|---:|---:|
| https://quotes.toscrape.com/js/ | 1102 | 1053 | 1062 | 1051 | 1179 | 702 | 947 |
| https://quotes.toscrape.com/js/page/2/ | 1113 | 1089 | 1091 | 1050 | 1182 | 721 | 936 |
| https://quotes.toscrape.com/js/page/3/ | 1099 | 1060 | 1071 | 1029 | 1167 | 719 | 962 |
| https://quotes.toscrape.com/js/page/4/ | 1101 | 1050 | 1065 | 1042 | 1168 | 723 | 1049 |
| https://demo.playwright.dev/todomvc/ | 879 | 789 | 771 | 824 | 663 | 395 | 415 |
| https://todomvc.com/examples/react/dist/ | 854 | 564 | 626 | 1266 | 1063 | 418 | 521 |
| https://todomvc.com/examples/vue/dist/ | 990 | 861 | 844 | 731 | 1015 | 400 | 296 |
| https://hn.algolia.com/ | 1215 | 1198 | 1225 | 1556 | 1532 | 786 | 728 |
| https://angular.dev/tutorials/learn-angular | 1131 | 1137 | 1005 | 30809 | 31103 | 562 | 500 |
| https://vercel.com | 1667 | 1641 | 1637 | 2913 | 1382 | 618 | 564 |
| https://www.notion.com | 1508 | 2138 | 1448 | 2578 | 1464 | 730 | 646 |
| https://stripe.com | 2588 | 2314 | 1945 | 2199 | 1961 | 1178 | 1037 |
| https://react.dev | 1158 | 1145 | 1051 | 1003 | 991 | 528 | 539 |
| https://nextjs.org/docs | 1804 | 1814 | 2307 | 2377 | 2515 | 520 | 555 |
| https://techcrunch.com | 2067 | 1811 | 1682 | 3285 | 2913 | 753 | 669 |
| https://www.shopify.com | 1391 | 1419 | 1468 | 1960 | 1978 | 540 | 598 |
| https://anthropic.com | 1422 | 1479 | 1289 | 1400 | 1417 | 890 | 787 |
| https://books.toscrape.com/ | 1983 | 1931 | 1729 | 1424 | 1335 | 1063 | 1022 |
| https://www.bbc.com/news | 1721 | 1498 | 1691 | 1887 | 1902 | 561 | 565 |
| https://www.theverge.com/ | 5250 | 5356 | 1802 | 31355 | 31338 | 751 | 742 |
