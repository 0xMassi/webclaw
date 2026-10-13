#!/usr/bin/env bash
# usage: ./run.sh [label] [webclaw-binary] [url-runs]
set -euo pipefail
[ -n "${2:-}" ] && set -- "$1" "$(cd "$(dirname "$2")" && pwd)/$(basename "$2")" "${@:3}"
cd "$(dirname "$0")"

label=${1:-$(date +%F)}
bin=${2:-../target/release/webclaw}
runs=${3:-3}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bin" = ../target/release/webclaw ]; then
  (cd .. && cargo build --release -p webclaw-cli --features browser)
fi
[ -x "$bin" ] || { echo "no webclaw binary at $bin" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")

source ./envs.sh

.work/venv-base/bin/python scripts/run.py prepare
rm -rf "results/$label/rows.jsonl" ".work/out/$label"
.work/venv-base/bin/python scripts/run.py run "$label" "$runs"
for v in base scrapling crawl4ai; do uv pip list -p ".work/venv-$v" 2>/dev/null; done \
  | grep -iE '^(trafilatura|tiktoken|scrapling|curl.cffi|markdownify|crawl4ai|playwright) ' | awk '{print $1, $2}' | sort -u > "results/$label/versions.txt"
.work/venv-base/bin/python scripts/run.py summary "$label"
