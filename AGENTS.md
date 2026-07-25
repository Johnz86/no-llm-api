# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs` bootstraps settings, data seeding, and Axum routing. Core modules live beside it (`config`, `dataset`, `http`, `model`, `service`, `store`).
- Each module keeps its focused unit tests in an inline `#[cfg(test)]` block; there is no standalone `tests/` directory.
- Generated parquet fixtures reside under `data/` (created on demand). Keep additional scripted datasets there to avoid polluting `src/`.

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
- Never commit real OpenAI credentials; the service relies solely on local parquet data.
- Validate new parquet fixtures locally with `cargo test` and keep example datasets free of sensitive content.
- Document any new environment variables-especially `TOKENIZER_MODEL` presets-in `README.md` alongside their defaults.

## Async-openai Compatibility Notes
- `tiktoken-rs` already pulls in `async-openai`, so the client is available without extra dependencies.
- Planned schema expansion will add optional columns for refusals, tool/function calls, audio metadata, finish reasons, and usage breakdowns so recorded datasets can round-trip all async-openai fields.
- We intend to ship a recorder that can proxy real OpenAI/Azure responses (keys via `.env`) and persist them as parquet fixtures for deterministic playback.
- Future end-to-end tests will boot the Axum server, drive it with async-openai (streaming and non-streaming), and assert full API compatibility; keep this in mind when changing request/response models or SSE behaviour.
