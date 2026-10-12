#!/usr/bin/env bash
# usage: ./crawl.sh [label] [webclaw-binary] [rounds]
set -euo pipefail
[ -n "${2:-}" ] && set -- "$1" "$(cd "$(dirname "$2")" && pwd)/$(basename "$2")" "${@:3}"
cd "$(dirname "$0")"

label=${1:-crawl-$(date +%F)}
bin=${2:-../target/release/webclaw}
rounds=${3:-3}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ ! -x "$bin" ] && [ "$bin" = ../target/release/webclaw ]; then
  (cd .. && cargo build --release -p webclaw-cli)
fi
[ -x "$bin" ] || { echo "no webclaw binary at $bin" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")

source ./envs.sh
.work/venv-base/bin/python scripts/crawl.py "$label" "$rounds"
