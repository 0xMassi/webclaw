#!/usr/bin/env bash
set -euo pipefail
[ -f "${2:-}" ] && set -- "$1" "$(dirname "$2")" "${@:3}"
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
cd "$(dirname "$0")"

label=${1:-pdf-$(date +%F)}
bins=${2:-../target/release}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli -p webclaw-mcp --features webclaw-cli/browser,webclaw-mcp/browser)
fi
(cd .. && cargo build --release -p webclaw-pdf --example pdf_text)
for b in webclaw webclaw-mcp; do
  [ -x "$bins/$b" ] || { echo "no $b binary in $bins" >&2; exit 1; }
done
export WEBCLAW_BIN=$(cd "$bins" && pwd)/webclaw WEBCLAW_MCP_BIN=$(cd "$bins" && pwd)/webclaw-mcp
export PDF_TEXT_BIN=$(cd ../target/release/examples && pwd)/pdf_text

source ./envs.sh
venv mcp "scrapling[ai] @ git+https://github.com/D4Vinci/Scrapling@7a68493"
.work/venv-crawl4ai/bin/python scripts/pdf.py "$label"
