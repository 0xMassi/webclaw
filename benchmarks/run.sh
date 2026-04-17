#!/usr/bin/env bash
# Reproduce the webclaw benchmark.
# Requires: python3, tiktoken, trafilatura. Optional: firecrawl-py + FIRECRAWL_API_KEY.

set -euo pipefail
cd "$(dirname "$0")"

# Build webclaw if not present
if [ ! -x "../target/release/webclaw" ]; then
    echo "→ building webclaw..."
    (cd .. && cargo build --release)
fi

# Install python deps if missing
missing=""
python3 -c "import tiktoken"     2>/dev/null || missing+=" tiktoken"
python3 -c "import trafilatura"  2>/dev/null || missing+=" trafilatura"
if [ -n "${FIRECRAWL_API_KEY:-}" ]; then
    python3 -c "import firecrawl" 2>/dev/null || missing+=" firecrawl-py"
fi
if [ -n "$missing" ]; then
    echo "→ installing python deps:$missing"
    python3 -m pip install --quiet $missing
fi

# Run
python3 scripts/bench.py
