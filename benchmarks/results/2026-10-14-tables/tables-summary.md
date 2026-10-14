# Table benchmark 2026-10-14-tables

Each tool reads the same 35 frozen pages and returns the data tables it finds. 121 tables are labelled as data tables by hand; the rest are layout tables, except infoboxes and similar key/value boxes, which count neither way. Cell text is what a browser shows: hidden elements are left out, and a cell spanning several rows or columns fills each slot it covers. Header rows are the rows of `<thead>` and the leading rows made only of `<th>`; a header row that is one cell spanning the whole table is a title, not column names, and columns named only by a title are not scored. A returned table is matched to the page table whose cell texts it overlaps most. Precision is matched data tables over returned tables; recall is matched data tables over expected ones. `body cells` counts non-empty body cells equal to the reference after whitespace collapsing, rows aligned at the best offset; `header cells` counts columns whose header equals one header row's text, or the levels joined by a space or ` / `. `exact` counts tables with every body and header cell right. Seconds are wall time for all pages; webclaw runs its CLI once per page, and `of which start` is the time the same number of `webclaw --version` runs takes.

| arm | returned | data tables found | precision | recall | body cells | header cells | exact | seconds | of which start |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 160 | 120/121 | 0.98 | 0.99 | 26551/26563 (100.0%) | 632/632 | 120 | 0.37 | 0.17 |
| crawl4ai | 234 | 111/121 | 0.86 | 0.92 | 23967/26563 (90.2%) | 563/632 | 93 | 1.31 | - |
| scrapling | 441 | 120/121 | 0.37 | 0.99 | 23244/26563 (87.5%) | 560/632 | 43 | 0.31 | - |
