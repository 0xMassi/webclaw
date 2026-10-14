# Table benchmark 2026-10-14-tables

Each tool reads the same frozen pages and returns the data tables it finds: 35 pages in the corpus and 19 held-out pages that were not looked at while writing webclaw's rules. Every table is labelled by hand in `tables-labels.json` and `tables-held-labels.json` as a data table, a layout table, or neither (infoboxes and key/value boxes), which counts neither way. A returned table is matched to the page table whose cell texts it overlaps most. Precision is matched data tables over returned tables; recall is matched data tables over expected ones. `body cells` counts non-empty body cells equal to the truth after whitespace collapsing, rows aligned in order; `header cells` counts columns whose header holds each header row's text, joined with a space or ` / `. `exact` counts tables with every body and header cell right.

Two truths. Chrome: the page loaded in headless Chrome with scripts off and stylesheets on, each visible cell's `innerText` laid out on the grid by its spans; tables a stylesheet hides count neither way. lxml: the raw HTML laid out the same way with lxml, hidden meaning `hidden`, inline `display:none` or `<template>`. In both a spanning cell fills every slot it covers, and a header row that is one cell across the table is a title, not column names.

## Corpus against Chrome (35 pages)

Seconds are wall time for all pages; webclaw runs its CLI once per page, and `of which start` is the time the same number of `webclaw --version` runs takes. Crawl4AI's time is its whole scraping pass, which is how its users get tables.

| arm | returned | data tables found | precision | recall | body cells | header cells | exact | seconds | of which start |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 163 | 113/114 | 0.92 | 0.99 | 24573/24787 (99.1%) | 524/543 | 102 | 0.34 | 0.14 |
| crawl4ai | 234 | 104/114 | 0.81 | 0.91 | 21989/24787 (88.7%) | 449/543 | 82 | 1.24 | - |
| scrapling | 441 | 113/114 | 0.35 | 0.99 | 21360/24787 (86.2%) | 448/543 | 34 | 0.29 | - |

## Held-out pages against Chrome (19 pages)

| arm | returned | data tables found | precision | recall | body cells | header cells | exact |
|---|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 89 | 79/79 | 1.00 | 1.00 | 24936/25218 (98.9%) | 451/562 | 54 |
| crawl4ai | 86 | 72/79 | 0.91 | 0.91 | 19785/25218 (78.5%) | 317/562 | 30 |
| scrapling | 112 | 79/79 | 0.78 | 1.00 | 22136/25218 (87.8%) | 402/562 | 21 |

## Corpus against the lxml reference

| arm | returned | data tables found | precision | recall | body cells | header cells | exact |
|---|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 163 | 120/121 | 0.98 | 0.99 | 26551/26563 (100.0%) | 632/632 | 120 |
| crawl4ai | 234 | 111/121 | 0.86 | 0.92 | 23967/26563 (90.2%) | 549/632 | 91 |
| scrapling | 441 | 120/121 | 0.37 | 0.99 | 23258/26563 (87.6%) | 556/632 | 43 |

Tables cut short by webclaw's size limits: corpus 0, held 0.
