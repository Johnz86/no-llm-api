# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs` bootstraps settings, data seeding, and Axum routing. Core modules live beside it (`config`, `dataset`, `http`, `live`, `model`, `service`, `store`, `tokenizer`) and are re-exported by `src/lib.rs` so tests and the `recorder`/`regenerate_dataset` binaries can reuse them.
- Module-level unit tests live in inline `#[cfg(test)]` blocks. Cross-module and client-contract tests live in `tests/`; `tests/async_openai.rs` boots the router in-process and drives it with the real client.
- Generated parquet fixtures reside under `data/` (created on demand). Keep additional scripted datasets there to avoid polluting `src/`.
- Plans and design notes live in `docs/`; start at `docs/plans/00-roadmap.md`. The distilled Chat Completions contract is `docs/spec/chat-completions-scope.md`.
- `openapi.yaml` is a verbatim copy of the upstream OpenAI spec, refreshed with `scripts/fetch-openapi.ps1` (or `.sh`), which records the upstream commit and sha256 in `openapi.provenance.json`. Never hand-edit it; upstream's `info.version` is a useless staleness signal, so check the provenance file. See `docs/spec/upstream-openapi.md`.

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
- `tests/async_openai.rs` boots the Axum server and drives it with the real client (non-streamed, streamed with usage, tool calls). It is currently gated behind `ASYNC_OPENAI_COMPAT=1`; keep it passing when changing request/response models or SSE behaviour.
