# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs` is a thin shim over the library: it parses the CLI, resolves settings, seeds data and serves the router. Every module lives in the library (`config`, `dataset`, `http`, `live`, `model`, `models`, `service`, `sim`, `store`, `tokenizer`) and is re-exported by `src/lib.rs`, so tests and the `recorder`/`regenerate_dataset` binaries reuse them. Never re-declare a module in `main.rs`: that compiles the crate twice and runs every unit test twice.
- `src/sim/` is the simulation engine: `stream.rs` (token pieces, pacing, faults, terminal frames), `select.rs` (deterministic fixture selection), `digest.rs` (FNV-1a; never `DefaultHasher`), `identity.rs` (plan-derived ids), `scenario.rs` (behaviour profiles), `directive.rs` (per-request overrides). `src/http/` holds `routes.rs`, `error.rs` (the single error envelope), `auth.rs` and `control.rs`. Streaming changes belong in `sim`, not in `routes.rs`.
- Live proxying is behind the `live` feature. The default build links no HTTP client, so `cargo tree --edges normal` must stay free of `reqwest`, `rustls` and `async-openai`; keep it that way.
- Fixture conversations live in `fixtures/*.yaml` and compile to parquet with `cargo run --bin fixtures -- build --force`; the linter rejects unknown fields, so a typo fails the build. Behaviour profiles live in `scenarios/*.yaml` and are embedded with `include_str!`. Adding one means adding it to `sim::scenario::BUILTINS` and to the README table, which `tests/docs.rs` enforces.
- Module-level unit tests live in inline `#[cfg(test)]` blocks. Cross-module and client-contract tests live in `tests/`; `tests/support/` holds the shared fixture builder and the SSE transcript harness.
- Browser integration tests live in `e2e/` and run separately with `npm --prefix e2e test`; they are never part of `cargo test`.
- Generated parquet fixtures reside under `data/` (created on demand). Keep additional scripted datasets there to avoid polluting `src/`.
- Plans and design notes live in `docs/`; start at `docs/plans/00-roadmap.md`. The distilled Chat Completions contract is `docs/spec/chat-completions-scope.md`.
- The tracked spec reference is `docs/spec/chat-completions.openapi.yaml`, a generated 144 KB extract of the paths this mock implements plus their schemas. Regenerate it with `cargo run --bin spec_extract` after refreshing the full upstream document with `scripts/fetch-openapi.ps1` (or `.sh`); `openapi.yaml` itself is gitignored. Never hand-edit either file, and check `openapi.provenance.json` rather than upstream's `info.version`, which never moves. See `docs/spec/upstream-openapi.md`.

## Build, Test, and Development Commands
- `cargo build` compiles the API server with the current profile.
- `cargo run` launches the mock service; override runtime options via env vars such as `BIND_ADDRESS`, `TOKENS_PER_SECOND`, and `TOKENIZER_MODEL`.
- `cargo test` executes the dataset and service test suites.
- `cargo check` performs a fast type-check; run this before larger refactors.
- `cargo fmt` enforces formatting across the workspace.
- `cargo clippy --all-targets --all-features` surfaces lint findings; treat warnings as failures before merging.

## Coding Style & Naming Conventions
- Follow idiomatic Rust 2024 style with `rustfmt` defaults (4-space indentation, trailing commas where appropriate) and keep code KISS/DRY/SOLID.
- Favor a clean, functional style: prefer pure helpers, minimize shared mutability, and lean on iterators and expressions over imperative loops.
- Avoid inline comments; use module or function-level documentation comments when necessary to clarify intent.
- Keep function names snake_case, types CamelCase, and module names lowercase; align new identifiers with existing naming patterns.

## Testing Guidelines
- Use `#[tokio::test]` for async exercises and keep fixtures deterministic; see `service::tests` for patterns.
- Group helper builders inside the test modules to avoid leaking test-only APIs into production code.
- When expanding SSE or storage behavior, add assertions for ordering, pagination, and token pacing.
- Selection and identity must stay pure functions of the request. No counters, no wall clock, no `DefaultHasher`: `tests/determinism.rs` asserts 64 concurrent identical requests produce exactly one distinct body.
- Snapshots in `tests/snapshots/` are unredacted on purpose. Review a diff before accepting it with `cargo insta accept`; a change there means the bytes a GUI sees changed.
- Timing assertions use the median inter-frame gap with wide tolerance (`tests/sse.rs`). Do not assert exact sleeps.
- Browser tests use Playwright's managed server and Chromium. Assert incremental text, final fixture content, and console errors rather than fixed rendering delays.

## Commit & Pull Request Guidelines
- Write imperative, 72-character subject lines (e.g., `Add streaming error handling`).
- Reference related issues in the body and note environment variables or dataset changes.
- PRs should summarize behavior shifts, list manual verification commands, and include screenshots when front-end consumers are affected.

## Security & Configuration Tips
- Never commit real OpenAI credentials. `.env` is gitignored; document new variables in `.env.example` and `README.md`.
- The default `parquet` dataset source needs no credentials and makes no network calls; only `DATASET_SOURCE=live` talks to a real backend.
- Validate new parquet fixtures locally with `cargo test` and keep example datasets free of sensitive content.
- Document any new environment variables-especially `TOKENIZER_MODEL` presets-in `README.md` alongside their defaults.

## Async-openai Compatibility Notes
- `async-openai` is an explicit dependency (`0.41.1`, features `["chat-completion"]`). Its types live under `async_openai::types::chat`.
- When checking what the reference client expects, read the crate source that matches `Cargo.lock`: `~/.cargo/registry/src/index.crates.io-*/async-openai-0.41.1/src/types/chat/`. Do not vendor an upstream clone into the repo; a clone that drifts from the locked version invents constraints the real client does not have.
- The parquet schema already carries optional columns for refusals, tool/function calls, audio metadata, finish reasons, and usage breakdowns so recorded datasets round-trip async-openai fields.
- `src/bin/recorder.rs` proxies real OpenAI/Azure responses (credentials via `.env`) and persists them as parquet fixtures for deterministic playback.
- `tests/async_openai.rs` boots the Axum server and drives it with the real client (non-streamed, streamed with usage, tool calls, and model discovery). It runs by default - no env flag - so keep it passing when changing request/response models or SSE behaviour.
- The models API is behind async-openai's `model` feature, which is why `Cargo.toml` enables `["chat-completion", "model"]`.
