#!/usr/bin/env bash
set -euo pipefail
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
cd "$(dirname "$0")"

label=${1:-actions-$(date +%F)}
bins=${2:-../target/release}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli --features webclaw-cli/browser)
fi
[ -x "$bins/webclaw" ] || { echo "no webclaw binary in $bins" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$bins" && pwd)/webclaw

source ./envs.sh
.work/venv-crawl4ai/bin/python scripts/actions.py "$label"
