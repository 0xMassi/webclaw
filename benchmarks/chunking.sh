#!/usr/bin/env bash
set -euo pipefail
[ -f "${2:-}" ] && set -- "$1" "$(dirname "$2")" "${@:3}"
if [ -n "${2:-}" ]; then
  [ -d "$2" ] || { echo "no bins dir at $2" >&2; exit 1; }
  set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
fi
cd "$(dirname "$0")"

label=${1:-chunking-$(date +%F)}
bins=${2:-../target/release}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli -p webclaw-mcp --features webclaw-cli/browser,webclaw-mcp/browser)
fi
[ -x "$bins/webclaw" ] || { echo "no webclaw binary in $bins" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$bins" && pwd)/webclaw

source ./envs.sh
[ -f .work/cases-html.jsonl ] || .work/venv-base/bin/python scripts/run.py prepare
.work/venv-base/bin/python scripts/chunking.py "$label"
