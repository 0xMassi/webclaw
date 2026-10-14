#!/usr/bin/env bash
# usage: ./query-crawl.sh [label] [bins-dir]
set -euo pipefail
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
cd "$(dirname "$0")"

label=${1:-query-crawl-$(date +%F)}
bins=${2:-../target/release}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli -p webclaw-mcp --features webclaw-cli/browser,webclaw-mcp/browser)
fi
[ -x "$bins/webclaw" ] || { echo "no webclaw binary in $bins" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$bins" && pwd)/webclaw

source ./envs.sh
.work/venv-base/bin/python scripts/query_crawl.py "$label"
