#!/usr/bin/env bash
set -euo pipefail
[ -f "${2:-}" ] && set -- "$1" "$(dirname "$2")" "${@:3}"
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
cd "$(dirname "$0")"

label=${1:-citations-$(date +%F)}
bins=${2:-../target/release}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli --features webclaw-cli/browser)
fi
[ -x "$bins/webclaw" ] || { echo "no webclaw binary in $bins" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$bins" && pwd)/webclaw

source ./envs.sh
.work/venv-base/bin/python scripts/run.py prepare
.work/venv-base/bin/python scripts/citations.py "$label"
for v in base crawl4ai; do uv pip list -p ".work/venv-$v" 2>/dev/null; done \
  | grep -iE '^(tiktoken|crawl4ai) ' | awk '{print $1, $2}' | sort -u > "results/$label/versions.txt"
