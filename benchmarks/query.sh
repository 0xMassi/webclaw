#!/usr/bin/env bash
set -euo pipefail
[ -n "${2:-}" ] && set -- "$1" "$(cd "$(dirname "$2")" && pwd)/$(basename "$2")" "${@:3}"
cd "$(dirname "$0")"

label=${1:-query-$(date +%F)}
bin=${2:-../target/release/webclaw}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bin" = ../target/release/webclaw ]; then
  (cd .. && cargo build --release -p webclaw-cli --features browser)
fi
[ -x "$bin" ] || { echo "no webclaw binary at $bin" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")

source ./envs.sh
[ -f .work/cases-html.jsonl ] || .work/venv-base/bin/python scripts/run.py prepare
.work/venv-base/bin/python scripts/query.py "$label"
