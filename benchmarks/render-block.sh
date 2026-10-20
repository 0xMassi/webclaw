#!/usr/bin/env bash
set -euo pipefail
[ -f "${2:-}" ] && set -- "$1" "$(dirname "$2")" "${@:3}"
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
cd "$(dirname "$0")"

label=${1:-render-block-$(date +%F)}
bins=${2:-../target/release}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli -p webclaw-mcp --features webclaw-cli/browser,webclaw-mcp/browser)
fi
[ -x "$bins/webclaw-mcp" ] || { echo "no webclaw-mcp binary in $bins" >&2; exit 1; }
export WEBCLAW_MCP_BIN=$(cd "$bins" && pwd)/webclaw-mcp

source ./envs.sh
venv mcp "scrapling[ai] @ git+https://github.com/D4Vinci/Scrapling@7a68493"
.work/venv-base/bin/python scripts/render_block.py "$label"
