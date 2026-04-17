#!/bin/sh
# webclaw docker entrypoint.
#
# Behaves like the real binary when the first arg looks like a webclaw arg
# (URL or flag), so `docker run ghcr.io/0xmassi/webclaw https://example.com`
# still works. But gets out of the way when the first arg looks like a
# different command (e.g. `./setup.sh`, `bash`, `sh -c ...`), so this image
# can be used as a FROM base in downstream Dockerfiles with a custom CMD.
#
# Test matrix:
#   docker run IMAGE https://example.com          → webclaw https://example.com
#   docker run IMAGE --help                       → webclaw --help
#   docker run IMAGE --file page.html             → webclaw --file page.html
#   docker run IMAGE --stdin < page.html          → webclaw --stdin
#   docker run IMAGE bash                         → bash
#   docker run IMAGE ./setup.sh                   → ./setup.sh
#   docker run IMAGE                              → webclaw --help (default CMD)
#
# Root cause fixed: v0.3.13 switched CMD→ENTRYPOINT to make the first use
# case work, which trapped the last four. This shim restores all of them.

set -e

# If the first arg starts with `-`, `http://`, or `https://`, treat the
# whole arg list as webclaw flags/URL.
if [ "$#" -gt 0 ] && {
    [ "${1#-}" != "$1" ] || \
    [ "${1#http://}" != "$1" ] || \
    [ "${1#https://}" != "$1" ]; }; then
    set -- webclaw "$@"
fi

exec "$@"
