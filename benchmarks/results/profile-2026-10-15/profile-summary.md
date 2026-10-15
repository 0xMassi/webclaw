# Browser profile profile-2026-10-15

A login in one process, then the logged-in page in a second process on the same profile.

| tool | site | login cookie | logged in | still logged in next run | logged out without the profile |
| --- | --- | --- | --- | --- | --- |
| webclaw | quotes.toscrape.com | session | yes | yes | yes |
| webclaw | practice.expandtesting.com | session | yes | yes | yes |
| scrapling | quotes.toscrape.com | session | yes | no | yes |
| scrapling | practice.expandtesting.com | session | yes | no | yes |
| crawl4ai | quotes.toscrape.com | session | yes | no | yes |
| crawl4ai | practice.expandtesting.com | session | yes | no | yes |
| scrapling-restore | quotes.toscrape.com | session | yes | yes | yes |
| scrapling-restore | practice.expandtesting.com | session | yes | yes | yes |
| scrapling-cookies | quotes.toscrape.com | session | yes | yes | yes |
| scrapling-cookies | practice.expandtesting.com | session | yes | yes | yes |
| crawl4ai-state | quotes.toscrape.com | session | yes | yes | yes |
| crawl4ai-state | practice.expandtesting.com | session | yes | yes | yes |

Values stored by a script in one process, read in the next.

| tool | stored | localStorage kept | IndexedDB kept |
| --- | --- | --- | --- |
| webclaw | yes | yes | yes |
| scrapling | yes | yes | yes |
| crawl4ai | yes | yes | yes |
| scrapling-restore | yes | yes | yes |
| scrapling-cookies | yes | no | no |
| crawl4ai-state | yes | yes | no |

Median wall time of one process, start to exit, rendering https://quotes.toscrape.com/js/, 5 runs each, and the profile size.

| tool | no profile | new profile | warm profile | new profile size | size after 6 renders |
| --- | --- | --- | --- | --- | --- |
| webclaw | 2233 ms | 2124 ms | 1700 ms | 8.8 MB | 8.9 MB |
| scrapling | 2088 ms | 1983 ms | 1636 ms | 8.7 MB | 8.8 MB |
| crawl4ai | 1636 ms | 1596 ms | 1272 ms | 1.9 MB | 2.1 MB |

Median wall time of `webclaw https://books.toscrape.com/ --crawl --depth 2 --max-pages 50`, 3 runs: with a profile every page loads in the browser.

| no profile | new profile | warm profile | pages |
| --- | --- | --- | --- |
| 3592 ms | 12202 ms | 7958 ms | 50 / 50 / 50 |

A second process on a profile while the first still has it open.

| tool | second process | message |
| --- | --- | --- |
| webclaw | browser refused, HTTP page kept |  |
| scrapling | refused | ill force kill> - [pid=98919] exception while trying to kill process: Error: kill EPERM - [pid=98919] <process did exit: exitCode=21, signal=null> - [pid=98919] starting temporary directories cleanup - [pid=98919] finished temporary directo |
| crawl4ai | ran on the profile in use |  |
