# Pinned tool environments shared by run.sh and crawl.sh. Source, don't run.
venv() {
  local name=$1; shift
  [ -x ".work/venv-$name/bin/python" ] || uv venv -q -p 3.12 ".work/venv-$name"
  uv pip install -q -p ".work/venv-$name" "$@"
}
venv base trafilatura==2.3.1 tiktoken==0.14.0 psutil==7.1.0
venv scrapling "scrapling[fetchers] @ git+https://github.com/D4Vinci/Scrapling@7a68493" markdownify==1.2.3
venv crawl4ai crawl4ai==0.9.4
[ -f .work/venv-crawl4ai/.setup-done ] || { .work/venv-crawl4ai/bin/crawl4ai-setup && touch .work/venv-crawl4ai/.setup-done; }
