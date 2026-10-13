venv() {
  local name=$1; shift
  [ -x ".work/venv-$name/bin/python" ] || uv venv -q -p 3.12 ".work/venv-$name"
  uv pip install -q -p ".work/venv-$name" "$@"
}
