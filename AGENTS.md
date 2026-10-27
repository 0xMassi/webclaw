# Repository Guidelines

## Project Structure & Module Organization

Webclaw is a Rust workspace with seven crates under `crates/`. Each crate keeps implementation in `src/`: `webclaw-core` handles HTML extraction, `webclaw-fetch` handles HTTP and crawling, `webclaw-llm` integrates LLM providers, and `webclaw-pdf` extracts PDF text. `webclaw-cli`, `webclaw-mcp`, and `webclaw-server` expose the CLI, MCP, and REST interfaces.

Unit tests live beside implementation; integration tests include `crates/webclaw-fetch/tests/`. `packages/` contains JavaScript launchers, `examples/` contains usage recipes, `benchmarks/` contains benchmark tooling, `assets/` holds images, and `deploy/` holds deployment configuration.

## Build, Test, and Development Commands

Use Rust edition 2024; `CONTRIBUTING.md` specifies Rust 1.85+. No local build flags are needed: `.cargo/config.toml` holds none.

- `cargo build --release`: build optimized workspace binaries.
- `cargo run -p webclaw-cli -- --help`: inspect local CLI commands.
- `cargo run -p webclaw-server`: start the REST server (default port 3000).
- `cargo test --workspace`: run workspace tests.
- `cargo test -p webclaw-core`: test extraction changes independently.
- `cargo fmt --all`: format Rust sources.
- `cargo fmt --check --all`: verify formatting.
- `cargo clippy --all -- -D warnings`: lint with warnings treated as errors.
- `cargo doc --no-deps --workspace`: build API documentation.

## Coding Style & Naming Conventions

Use four-space indentation and the repository's `rustfmt.toml`. Follow Rust conventions: `snake_case` functions/modules, `PascalCase` types, and `SCREAMING_SNAKE_CASE` constants. Document public items. Return `Result` for failures; never unwrap untrusted input.

Keep `webclaw-core` network-free and WASM-safe. Route fetching through `webclaw-fetch::FetchClient`; LLM providers use plain `reqwest`.

## Testing Guidelines

Use Rust's `#[test]` and Tokio's `#[tokio::test]`, with descriptive `snake_case` test names. Add tests for new behavior and regressions. No numerical coverage threshold is configured. CI checks tests, formatting, linting, documentation, and `webclaw-core` WASM builds with default features enabled and disabled.

## Commit & Pull Request Guidelines

Follow Conventional Commits with imperative subjects under 72 characters; history includes `deps(fetch): ...` and `perf(sitemap): ...`. Use focused branches such as `feat/my-feature`. Open PRs against `main`, describe behavior changes and validation, and link relevant issues. Keep one concern per PR; checks and review are required before merging.

## Security & Configuration

Use `env.example` and `proxies.example.txt` as configuration references. Never commit credentials, local `.env` files, or private infrastructure details. Inspect staged diffs for sensitive material before committing.

## Verification

- Prove user-facing CLI and MCP behaviour with `.claude/skills/verify-webclaw/SKILL.md`; its `features/` map lists every entry point.
