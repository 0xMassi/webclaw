# Adaptive selector benchmark adaptive-2026-10-15

21 pages, each frozen twice from the Wayback Machine years apart into `.work/adaptive-corpus/`, 60 target fields. Each field has a CSS selector written on the old page and a hand-checked selector for the same field on the new page (`adaptive-cases.json`). An answer is right when its first element has the tag, attributes and text of the field's element on the new page.

Baseline drift: the old selector picks the right element on the new page for 34/60 fields. It picks nothing for 22 and a different element for 4 (silent drift). 13/21 pages have at least one drifted field.

| arm | right | wrong | none | right on drifted | wrong on drifted | right on stable | seconds |
|---|---:|---:|---:|---:|---:|---:|---:|
| selector | 34/60 | 4 | 22 | 0/26 | 4 | 34/34 | 0.56 |
| text-fallback | 40/60 | 8 | 12 | 6/26 | 8 | 34/34 | 0.96 |
| scrapling-adaptive | 41/60 | 16 | 3 | 7/26 | 16 | 34/34 | 3.76 |
| scrapling-relocate | 39/60 | 18 | 3 | 8/26 | 15 | 31/34 | 7.76 |
| crawl4ai | no equivalent (no saved-element relocation) | - | - | - | - | - | - |

Scrapling's relocation keeps its best-scoring element when the score is at least `percentage` (default 40). The same `adaptive=True` run with a higher threshold, using the scores of the `scrapling-relocate` arm for the fields whose old selector picks nothing:

| threshold | right | wrong | none | right on drifted | wrong on drifted |
|---:|---:|---:|---:|---:|---:|
| 40 | 41/60 | 16 | 3 | 7/26 | 16 |
| 50 | 40/60 | 12 | 8 | 6/26 | 12 |
| 60 | 39/60 | 8 | 13 | 5/26 | 8 |
| 70 | 37/60 | 5 | 18 | 3/26 | 5 |
| 80 | 36/60 | 4 | 20 | 2/26 | 4 |
| 90 | 34/60 | 4 | 22 | 0/26 | 4 |

Arms:

- `selector`: webclaw `--select` with the old selector, nothing else
- `text-fallback`: old selector; when it matches nothing, webclaw `--find-text` for the saved text, exact
- `scrapling-adaptive`: Scrapling `auto_save=True` on the old page, `adaptive=True` on the new one
- `scrapling-relocate`: Scrapling `relocate()` with the saved element, selector ignored

## Per field

`relocate score` is Scrapling's score for its best element and whether that element is right.

| page | field | selector | text-fallback | scrapling-adaptive | scrapling-relocate | relocate score |
|---|---|---|---|---|---|---:|
| hn | top story title | none | none | wrong | wrong | 57.9 wrong |
| hn | top story points | right | right | right | wrong | 74.8 wrong |
| hn | top story site | right | right | right | wrong | 87.1 wrong |
| wiki-python | title | right | right | right | right | 51.8 right |
| wiki-python | license | wrong | wrong | wrong | wrong | 81.2 wrong |
| wiki-python | first toc entry | none | wrong | right | right | 81.8 right |
| wiki-berlin | mayor | wrong | wrong | wrong | wrong | 81.2 wrong |
| wiki-berlin | elevation | wrong | wrong | wrong | right | 83.8 right |
| wiki-berlin | population | none | none | wrong | wrong | 79.1 wrong |
| gh-rust | description | none | right | wrong | wrong | 61.6 wrong |
| gh-rust | stars | none | none | wrong | wrong | 51.8 wrong |
| gh-rust | repo name | right | right | right | right | 86.9 right |
| gh-torvalds | full name | right | right | right | right | 91.0 right |
| gh-torvalds | followers | none | none | wrong | wrong | 60.1 wrong |
| gh-torvalds | location | right | right | right | right | 92.8 right |
| pypi-requests | name and version | right | right | right | right | 84.0 right |
| pypi-requests | summary | right | right | right | right | 98.2 right |
| pypi-requests | release date | right | right | right | right | 73.3 right |
| py-json | title | right | right | right | right | 75.8 right |
| py-json | json.dumps signature | right | right | right | right | 85.2 right |
| so-sorted | question title | right | right | right | right | 96.9 right |
| so-sorted | question votes | right | right | right | right | 76.9 right |
| so-sorted | asker | right | right | right | right | 100.0 right |
| imdb-shawshank | title | none | none | wrong | wrong | 56.9 wrong |
| imdb-shawshank | rating | none | right | right | right | 48.3 right |
| imdb-shawshank | year | none | right | none | none | 38.8 wrong |
| imdb-shawshank | director | none | right | wrong | wrong | 63.8 wrong |
| goodreads-hobbit | title | none | none | right | right | 51.8 right |
| goodreads-hobbit | author | none | right | right | right | 78.7 right |
| goodreads-hobbit | rating | none | none | wrong | wrong | 48.1 wrong |
| arxiv-attention | title | right | right | right | right | 94.5 right |
| arxiv-attention | first author | right | right | right | right | 90.0 right |
| arxiv-attention | subjects | right | right | right | right | 98.8 right |
| mdn-div | title | right | right | right | right | 66.2 right |
| mdn-div | summary | none | none | wrong | wrong | 50.4 wrong |
| mdn-div | content categories | right | right | right | right | 95.2 right |
| steam-portal2 | name | right | right | right | right | 92.0 right |
| steam-portal2 | price | wrong | wrong | wrong | wrong | 94.0 wrong |
| steam-portal2 | release date | right | right | right | right | 100.0 right |
| adafruit-pi4 | name | right | right | right | right | 99.6 right |
| adafruit-pi4 | price | right | right | right | right | 91.8 right |
| adafruit-pi4 | product id | right | right | right | right | 93.2 right |
| rustdoc-vec | title | none | none | wrong | wrong | 45.5 wrong |
| rustdoc-vec | Vec::new signature | none | none | none | none | 37.6 wrong |
| rustdoc-vec | summary | right | right | right | right | 93.1 right |
| npm-react | name | none | right | right | right | 66.6 right |
| npm-react | version | none | none | right | right | 83.8 right |
| librs-serde | description | right | right | right | right | 98.9 right |
| librs-serde | latest version | right | right | right | right | 93.1 right |
| rt-shawshank | title | none | wrong | none | none | 39.7 right |
| rt-shawshank | tomatometer | none | wrong | wrong | wrong | 42.9 wrong |
| rt-shawshank | synopsis | none | none | wrong | wrong | 47.5 wrong |
| w3s-div | title | right | right | right | right | 95.6 right |
| w3s-div | first section heading | right | right | right | right | 95.6 right |
| python-org | latest release | right | right | right | right | 91.6 right |
| python-org | latest news | right | right | right | wrong | 77.2 wrong |
| python-org | introduction | right | right | right | right | 100.0 right |
| amazon-clrs | title | right | right | right | right | 90.1 right |
| amazon-clrs | first author | none | wrong | right | right | 63.3 right |
| amazon-clrs | review count | right | right | right | right | 89.2 right |
