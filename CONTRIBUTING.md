# Contributing to Webclaw

Thanks for your interest in contributing. This document covers the essentials.

## Development Setup

1. Install Rust 1.85+ (edition 2024 required):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. Clone and build:
   ```bash
   git clone https://github.com/0xMassi/webclaw.git
   cd webclaw
   cargo build --release
   ```

   RUSTFLAGS are configured in `.cargo/config.toml` -- no manual flags needed.

3. Optional: run `./setup.sh` for environment bootstrapping.

## Running Tests

```bash
cargo test --workspace          # All crates
cargo test -p webclaw-core      # Single crate
```

## Linting

```bash
cargo clippy --all -- -D warnings
cargo fmt --check --all
```

Both must pass cleanly before submitting a PR.

## Code Style

- Rust edition 2024, formatted with `rustfmt` (see `rustfmt.toml`, `style_edition = "2024"`)
- `webclaw-core` has zero network dependencies -- keep it WASM-safe
- `webclaw-llm` uses plain `reqwest` — LLM APIs don't need TLS fingerprinting
- Prefer returning `Result` over panicking. No `.unwrap()` on untrusted input.
- Doc comments on all public items. Explain *why*, not *what*.

## Pull Request Process

1. Fork the repository and create a feature branch:
   ```bash
   git checkout -b feat/my-feature
   ```

2. Make your changes. Write tests for new functionality.

3. Ensure all checks pass:
   ```bash
   cargo test --workspace
   cargo clippy --all -- -D warnings
   cargo fmt --check --all
   ```

4. Push and open a pull request against `main`.

5. PRs require review before merging. Keep changes focused -- one concern per PR.

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add PDF table extraction
fix: handle malformed sitemap XML gracefully
refactor: simplify crawler BFS loop
docs: update MCP setup instructions
test: add glob_match edge cases
chore: bump dependencies
```

Use the imperative mood ("add", not "added"). Keep the subject under 72 characters.
Body is optional but encouraged for non-trivial changes.

## Reporting Issues

- Search existing issues before opening a new one
- Include: Rust version, OS, steps to reproduce, expected vs actual behavior
- For extraction bugs: include the URL (or HTML snippet) and the output format used
- Security issues: email directly instead of opening a public issue

## Architecture

```
webclaw (this repo)
├── crates/
│   ├── webclaw-core/    # Pure extraction engine (HTML → markdown/json/text)
│   ├── webclaw-fetch/   # HTTP client + crawler + sitemap + batch
│   ├── webclaw-llm/     # LLM provider chain (Ollama → OpenAI → Anthropic)
│   ├── webclaw-pdf/     # PDF text extraction
│   ├── webclaw-cli/     # CLI binary
│   └── webclaw-mcp/     # MCP server binary
│
└── [patch.crates-io]    # Points to webclaw-tls for TLS fingerprinting
```

TLS fingerprinting lives in a separate repo: [webclaw-tls](https://github.com/0xMassi/webclaw-tls). The `[patch.crates-io]` section in `Cargo.toml` overrides rustls, h2, hyper, hyper-util, and reqwest with our patched forks for browser-grade JA4 + HTTP/2 Akamai fingerprinting.

## Crate Boundaries

Changes that cross crate boundaries need extra care:

| Crate | Network? | Key constraint |
|-------|----------|----------------|
| webclaw-core | No | Zero network deps, WASM-safe |
| webclaw-fetch | Yes (webclaw-http) | Uses [webclaw-tls](https://github.com/0xMassi/webclaw-tls) for TLS fingerprinting |
| webclaw-llm | Yes (reqwest) | Plain reqwest — LLM APIs don't need TLS fingerprinting |
| webclaw-pdf | No | Minimal, wraps pdf-extract |
| webclaw-cli | Yes | Depends on all above |
| webclaw-mcp | Yes | MCP server via rmcp |
