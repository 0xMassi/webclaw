#!/usr/bin/env bash
set -euo pipefail
[ -f "${2:-}" ] && set -- "$1" "$(dirname "$2")" "${@:3}"
[ -n "${2:-}" ] && set -- "$1" "$(cd "$2" && pwd)" "${@:3}"
cd "$(dirname "$0")"

label=${1:-profiles-$(date +%F)}
bin=${2:-../target/release}/webclaw
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ "$bin" = ../target/release/webclaw ]; then
  (cd .. && cargo build --release -p webclaw-cli)
fi
[ -x "$bin" ] || { echo "no webclaw binary at $bin" >&2; exit 1; }
export WEBCLAW_BIN=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")

source ./envs.sh
.work/venv-scrapling/bin/python scripts/profiles.py "$label"
