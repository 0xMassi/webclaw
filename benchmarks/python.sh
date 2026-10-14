#!/usr/bin/env bash
set -euo pipefail
[ -n "${2:-}" ] && set -- "$1" "$(cd "$(dirname "$2")" && pwd)/$(basename "$2")" "${@:3}"
cd "$(dirname "$0")"

label=${1:-python-$(date +%F)}
wheel=${2:-}
unset WEBCLAW_API_KEY

command -v uv >/dev/null || { echo "needs uv: https://docs.astral.sh/uv/getting-started/installation/" >&2; exit 1; }
if [ -z "$wheel" ]; then
  rm -rf .work/python-wheel
  (cd ../python && uv build --wheel -o ../benchmarks/.work/python-wheel)
  wheel=$(cd .work/python-wheel && pwd)/$(ls .work/python-wheel | grep '\.whl$')
fi
[ -f "$wheel" ] || { echo "no wheel at $wheel" >&2; exit 1; }

source ./envs.sh
.work/venv-base/bin/python -B scripts/python_bench.py "$label" "$wheel"
