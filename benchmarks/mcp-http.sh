#!/usr/bin/env bash
set -euo pipefail
[ -f "${2:-}" ] && set -- "$1" "$(dirname "$2")" "${@:3}"
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
cd "$(dirname "$0")"

label=${1:-mcp-http-$(date +%F)}
bin=${2:-../target/release}/webclaw-mcp
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bin" = ../target/release/webclaw-mcp ]; then
  (cd .. && cargo build --release -p webclaw-mcp)
fi
[ -x "$bin" ] || { echo "no webclaw-mcp binary at $bin" >&2; exit 1; }
export WEBCLAW_MCP_BIN=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")

source ./venv.sh
venv mcp "scrapling[ai] @ git+https://github.com/D4Vinci/Scrapling@7a68493"
.work/venv-mcp/bin/python scripts/mcp_http.py "$label"
