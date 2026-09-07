#!/usr/bin/env python3
"""Write deterministic offline workloads and a manifest; no downloads or dependencies."""
import hashlib
import json
import re
import sys
from pathlib import Path

destination = Path(sys.argv[1]).resolve()
destination.mkdir(parents=True, exist_ok=True)
testdata = Path(__file__).resolve().parents[2] / "crates/webclaw-core/testdata"
cases = {}
for path in sorted(testdata.rglob("*.html")):
    html = path.read_text()
    url = re.search(r'<link rel="canonical" href="([^"]+)"', html)[1]
    cases[path.stem] = {"file": str(path), "url": url, "synthetic": False}

paragraph = "Text extraction preserves readable content, links, and structured metadata. "
article = "<html><head><title>Extraction workload</title></head><body><article>"
for i in range(160):
    article += f'<section><h2>Section {i}</h2><p>{paragraph * 8}'
    article += f'<a href="/reference/{i}">Reference {i}</a></p></section>'
article += "</article></body></html>"
synthetic = {"article": article}
for blocks in (0, 1, 10, 100):
    markup = "<html><head><title>Catalog workload</title>"
    for i in range(blocks):
        product = {"@type": "Product", "name": f"Product {i}", "offers": {"price": i + 1}}
        markup += '<script type="application/ld+json">' + json.dumps(product) + '</script>'
    markup += "</head><body><main><h1>Catalog</h1><p>" + paragraph * 12 + "</p></main>"
    # A large trailing inline bundle exposes repeated scans of the remaining page.
    markup += '<script>/*' + 'bundle padding ' * 35000 + '*/</script></body></html>'
    synthetic[f"catalog_{blocks}"] = markup
synthetic["unterminated"] = (
    '<script type="application/ld+json">{"name":"missing end tag"}' + "padding " * 65000
)
for name, html in synthetic.items():
    path = destination / f"{name}.html"
    path.write_text(html)
    cases[name] = {"file": str(path), "url": "https://example.com/page", "synthetic": True}
for case in cases.values():
    data = Path(case["file"]).read_bytes()
    case.update(bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
(destination / "manifest.json").write_text(json.dumps(cases, indent=2) + "\n")
print(destination / "manifest.json")
