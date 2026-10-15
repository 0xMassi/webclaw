#!/usr/bin/env bash
set -euo pipefail
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
[ -n "${3:-}" ] && set -- "$1" "$2" "$(cd "$3" && pwd)" "${@:4}"
cd "$(dirname "$0")"

label=${1:-politeness-$(date +%F)}
bins=${2:-../target/release}
before=${3:-}
rounds=${4:-2}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bins" = ../target/release ]; then
  (cd .. && cargo build --release -p webclaw-cli -p webclaw-mcp --features webclaw-cli/browser,webclaw-mcp/browser)
fi
[ -x "$bins/webclaw" ] || { echo "no webclaw binary in $bins" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$bins" && pwd)/webclaw
if [ -n "$before" ]; then
  [ -x "$before/webclaw" ] || { echo "no webclaw binary in $before" >&2; exit 1; }
  export WEBCLAW_BEFORE_BIN=$before/webclaw
fi

source ./envs.sh
.work/venv-base/bin/python scripts/politeness.py "$label" "$rounds"
