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

Values stored by a script in one process, read in the next.

| tool | stored | localStorage kept | IndexedDB kept |
| --- | --- | --- | --- |
| webclaw | yes | yes | yes |
| scrapling | yes | yes | yes |
| crawl4ai | yes | yes | yes |

Median wall time of one process, start to exit, rendering https://quotes.toscrape.com/js/, 5 runs each, and the profile size.

| tool | no profile | new profile | warm profile | new profile size | size after 6 renders |
| --- | --- | --- | --- | --- | --- |
| webclaw | 1981 ms | 2087 ms | 1663 ms | 8.8 MB | 8.9 MB |
| scrapling | 2302 ms | 2287 ms | 1709 ms | 8.7 MB | 8.8 MB |
| crawl4ai | 1773 ms | 1692 ms | 1319 ms | 1.9 MB | 2.1 MB |

A second process on a profile while the first still has it open.

| tool | second process | message |
| --- | --- | --- |
| webclaw | refused | file takes one browser at a time fetch error: client build failed: client build failed: the browser profile DIR is in use by another browser, and a profile takes one browser at a time |
| scrapling | refused | 5] <will force kill> - [pid=1305] exception while trying to kill process: Error: kill EPERM - [pid=1305] <process did exit: exitCode=21, signal=null> - [pid=1305] starting temporary directories cleanup - [pid=1305] finished temporary direct |
| crawl4ai | ran on the profile in use |  |
