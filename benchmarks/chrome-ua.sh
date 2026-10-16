#!/usr/bin/env bash
set -euo pipefail
[ -f "${2:-}" ] && set -- "$1" "$(dirname "$2")" "${@:3}"
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
[ -n "${3:-}" ] && set -- "$1" "$2" "$(cd "$3" && pwd)" "${@:4}"
cd "$(dirname "$0")"

label=${1:-chrome-ua-$(date +%F)}
bins=${2:-../target/release}
baseline=${3:-}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli -p webclaw-mcp --features webclaw-cli/browser,webclaw-mcp/browser)
fi
for b in webclaw webclaw-mcp; do
  [ -x "$bins/$b" ] || { echo "no $b binary in $bins" >&2; exit 1; }
done
export WEBCLAW_BIN=$(cd "$bins" && pwd)/webclaw WEBCLAW_MCP_BIN=$(cd "$bins" && pwd)/webclaw-mcp
if [ -n "$baseline" ]; then
  [ -x "$baseline/webclaw" ] || { echo "no webclaw binary in $baseline" >&2; exit 1; }
  export WEBCLAW_BASELINE_BIN=$baseline/webclaw
fi

source ./envs.sh
venv mcp "scrapling[ai] @ git+https://github.com/D4Vinci/Scrapling@7a68493"
.work/venv-base/bin/python scripts/chrome_ua.py "$label"
