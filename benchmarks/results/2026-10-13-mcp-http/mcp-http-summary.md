# MCP over HTTP 2026-10-13-mcp-http

Each server runs its streamable-HTTP transport on loopback with a bearer token. The official MCP Python client opens a fresh session per call (initialize, then one fetch-and-extract tool call) for 3 pages x 5 rounds. `ok` counts calls that returned page text without a tool error. `refuses no token` checks that a client without the token is turned away. Crawl4AI has no standalone MCP server to compare.

| server | calls | ok | handshake ms (median) | tool call ms (median) | tool call ms (p95) | refuses no token |
|---|---:|---:|---:|---:|---:|---|
| webclaw | 15 | 15 | 4 | 283 | 593 | yes |
| scrapling | 15 | 15 | 4 | 556 | 904 | yes |
