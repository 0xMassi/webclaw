#!/usr/bin/env bash
# usage: ./run.sh [label] [webclaw-binary] [url-runs]
set -euo pipefail
cd "$(dirname "$0")"

label=${1:-$(date +%F)}
bin=${2:-../target/release/webclaw}
runs=${3:-3}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ ! -x "$bin" ] && [ "$bin" = ../target/release/webclaw ]; then
  (cd .. && cargo build --release -p webclaw-cli)
fi
[ -x "$bin" ] || { echo "no webclaw binary at $bin" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")

venv() {
  local name=$1; shift
  [ -x ".work/venv-$name/bin/python" ] && return
  uv venv -q -p 3.12 ".work/venv-$name"
  uv pip install -q -p ".work/venv-$name" "$@"
}
venv base trafilatura==2.3.1 tiktoken==0.14.0
venv scrapling "scrapling[fetchers] @ git+https://github.com/D4Vinci/Scrapling@7a68493" markdownify==1.2.3
venv crawl4ai crawl4ai==0.9.4
[ -f .work/venv-crawl4ai/.setup-done ] || { .work/venv-crawl4ai/bin/crawl4ai-setup && touch .work/venv-crawl4ai/.setup-done; }

[ -f .work/cases-html.jsonl ] || .work/venv-base/bin/python scripts/run.py prepare
rm -rf "results/$label" ".work/out/$label"
.work/venv-base/bin/python scripts/run.py run "$label" "$runs"
for v in base scrapling crawl4ai; do uv pip list -p ".work/venv-$v" 2>/dev/null; done \
  | grep -iE '^(trafilatura|tiktoken|scrapling|curl.cffi|markdownify|crawl4ai|playwright) ' | sort -u > "results/$label/versions.txt"
.work/venv-base/bin/python scripts/run.py summary "$label"
