# Repository guidelines

## Product contract

- `no-llm-api` is a deterministic, offline OpenAI Chat Completions test double. Preserve
  reproducible fixture selection, OpenAI-shaped JSON and SSE, controlled timing/fault simulation,
  and an offline default build.
- Treat [`README.md`](README.md) as the canonical product and operations guide and
  [`docs/spec/chat-completions-scope.md`](docs/spec/chat-completions-scope.md) as the implemented
  wire contract. Describe shipped behavior in present tense. Historical changes belong in
  `CHANGELOG.md`, not in planning documents.
- Version `1.0.0` is the stable compatibility baseline. Follow `docs/versioning.md`: intended
  incompatible wire, configuration, fixture-selection, or scenario changes require a major version.
  Update `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` together.
- The crate is intentionally not publishable until a license is selected. Do not remove
  `publish = false`, invent a license, create a remote, push, or tag unless the user explicitly
  requests that action.

## Architecture and ownership

- `src/main.rs` is only the CLI/bootstrap shim. Production modules live in the library and are
  re-exported by `src/lib.rs` so tests and auxiliary binaries use the same code. Never redeclare
  modules in `main.rs`; doing so compiles the crate and unit tests twice.
- `src/sim/` owns deterministic selection, FNV-1a digests, request-derived identity, scenarios,
  directives, token pieces, pacing, faults, cancellation, and terminal frames. Put streaming
  behavior in `sim::stream`, not in HTTP route handlers.
- `src/http/` owns transport concerns: routing, validation, auth, control plane, metrics, CORS,
  response headers, and the single error envelope in `error.rs`. Do not construct ad hoc error
  JSON in routes or services.
- `service.rs` turns a validated request and selected fixture into completion plans.
  `store.rs` owns the process-local store and pagination. Keep selection and identity independent
  from storage and request order.
- Prefer pure helpers and immutable plans. Shared mutable state is limited to explicit runtime
  facilities such as the completion store, control-plane scenario, bounded request log, metrics,
  and cancellation counter.

## Determinism rules

- Selection and identity are pure functions of request inputs. Never use counters, fixture rotation,
  hash-map iteration order, wall time, random UUIDs, or `DefaultHasher` for response selection or
  payload identity.
- Use the repository's framed FNV-1a digest implementation. Adding a selection input requires
  determinism tests proving identical concurrent requests still produce one distinct body and one
  distinct transcript.
- Preserve the matching ladder: conversation prefix, contiguous user-turn suffix, exact last-user
  match, then digest fallback. Changing the ladder is consumer-visible behavior.
- Timing may change delivery intervals but never reconstructed response bytes. Timing tests use the
  median inter-frame gap with wide tolerances; never assert exact sleeps.
- Multi-byte token boundaries must be decoded with the growing-buffer strategy. Do not decode token
  bytes independently or silently drop incomplete UTF-8.

## Streaming and HTTP invariants

- A normal stream opens with a role-only delta, emits non-empty content/reasoning/refusal/tool
  fragments, emits one terminal choice frame per choice, optionally emits one final usage frame,
  and ends with exactly one `[DONE]`.
- Preserve stable chunk identity and ordering. Reasoning precedes visible content. Tool arguments
  concatenate into valid JSON and never use empty argument fragments. Absent delta members are
  omitted rather than serialized as `null`.
- Dropping the response body cancels pacing immediately. Do not reintroduce detached producers that
  continue after the client disconnects.
- SSE responses remain uncompressed and carry `X-Accel-Buffering: no`. Do not add middleware that
  buffers or compresses streams.
- Known request fields are strongly typed and validated. Unknown fields stay forward-compatible and
  are ignored. Errors always use the four-member OpenAI envelope, including explicit `param` and
  `code` nulls when absent.
- Root and `/v1` route mounts remain equivalent. Every response carries `x-request-id` and
  `x-no-llm-api-version`; completion responses also carry `x-simulate-match`.

## Feature and security boundaries

- Live proxying remains behind the `live` feature. The default normal dependency graph must contain
  no `async-openai`, `reqwest`, or `rustls`. Verify with:

  ```bash
  cargo tree --locked --edges normal --no-default-features
  ```

- Only `DATASET_SOURCE=live` with a live-feature build may contact OpenAI or Azure. Preserve request
  timeout, concurrency, and process call caps.
- Never commit credentials or real recorded conversations. `.env` and live datasets are ignored.
  Document configuration in both `.env.example` and `README.md`; `tests/docs.rs` checks every
  CLI flag and environment variable.
- Credentials must not implement `Debug` or `Serialize`, enter tracing spans, appear in the
  control-plane request log, or survive the recording redaction sweep.
- The control plane defaults to loopback-only. A non-loopback deployment must opt in explicitly and
  should use a control token. Do not broaden its exposure as a side effect of route work.

## Fixtures, models, and generated artifacts

- Author conversations in `fixtures/*.yaml`; compile them to parquet with
  `cargo run --bin fixtures -- build --force`. The linter rejects unknown fields, invalid tool
  arguments, missing payloads, and invalid turn endings. Never bypass it by hand-writing parquet.
- Built-in fixtures and scenarios are embedded. Adding a fixture requires updating the generated
  dataset and reviewing affected snapshots. Adding a scenario requires registering it in
  `sim::scenario::BUILTINS` and documenting it in the README.
- A fixture addition can change digest fallback selection for existing unknown prompts. Treat that
  as a compatibility decision, not harmless test data.
- Model catalogues expose only OpenAI model fields publicly; capabilities and latency stay in the
  control-plane view. Preserve per-request directive precedence over scenario timing, and scenario
  precedence over model latency.
- `docs/spec/chat-completions.openapi.yaml` is generated by `spec_extract`; `openapi.yaml` is a
  gitignored upstream input. Never hand-edit either generated specification or provenance. Refresh
  upstream, regenerate the extract, then run `cargo test --test spec`.
- Snapshots are intentionally unredacted because they are wire contracts. Review every diff before
  `cargo insta accept`; never accept snapshots merely to make a test pass.

## Implementation flow

1. Start with `git status --short`. Existing changes belong to the user; do not overwrite,
   reformat, stage, or revert unrelated work.
2. Read the relevant module, its inline unit tests, the closest integration tests, and the maintained
   contract documentation before editing. Use the locked dependency source when client type details
   matter.
3. State the observable behavior and invariants being changed. Choose the owning module before
   writing code; avoid cross-layer fixes in `routes.rs`.
4. Implement the smallest complete vertical change. Update typed models, fixture schema, runtime
   behavior, tests, docs, examples, and configuration together when the contract crosses them.
5. Run `cargo fmt --all --check`, then the narrowest relevant test. Use `cargo check` before a
   large refactor, but do not treat it as final validation.
6. Run the full gates below. Review `git diff`, `git diff --check`, generated data, snapshots, and
   `git status` before committing.
7. Commit only task-owned files with an imperative subject of at most 72 characters. Never tag or
   push as an implied part of committing.

## Required validation

For normal Rust changes:

```bash
cargo fmt --all --check
cargo test --locked --all-targets
cargo test --locked --all-targets --all-features
cargo test --locked --doc
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo deny --log-level error check advisories bans licenses sources
```

- On memory-constrained Windows hosts, set `CARGO_BUILD_JOBS=1` for all-target/all-feature and
  release commands. An allocator failure can surface as misleading missing-crate or linker errors.
  Do not run multiple Cargo builds against the same target directory concurrently.
- This package has multiple binaries. Use `cargo run --bin no-llm-api -- ...`; bare `cargo run`
  is ambiguous.
- Unit tests stay inline. Cross-module and client contracts live in `tests/`; shared SSE parsing and
  fixture builders live in `tests/support/`. Use `#[tokio::test]` for async behavior.
- `tests/async_openai.rs` uses the real locked `async-openai` 0.41.1 client and runs without an
  environment gate. It covers non-streamed and streamed content, usage, tools, refusals, finish
  reasons, and model discovery. Read the matching Cargo registry source instead of an upstream clone.

For browser or packaging changes:

```bash
npm ci --prefix e2e
npx --prefix e2e playwright install chromium
npm --prefix e2e test
docker compose config --quiet
docker compose -f docker-compose.demo.yml config --quiet
```

- Playwright owns port `18080`; do not run browser suites concurrently. Assert incremental text,
  exact final fixture content, API-error display, and browser-console hygiene instead of fixed
  rendering delays.
- `scripts/smoke-open-webui.sh` is the authoritative third-party GUI check. It downloads a large
  rolling image, so run it only for Open WebUI/Compose compatibility changes or explicit release
  verification. Preserve its cleanup trap and use a unique Compose project name.
- On Windows, invoke POSIX scripts with Git Bash explicitly when `bash` resolves to an incomplete
  WSL installation. Validate shell syntax before a long container smoke.
- A release change also runs `cargo build --release --locked --bin no-llm-api`, confirms
  `no-llm-api --version`, checks Cargo metadata, and ensures the changelog contains the package
  version.

## Code quality and review

- Follow Rust 2024 and `rustfmt`. Prefer precise types, small pure functions, iterator-based
  transformations, and explicit ownership over `serde_json::Value`, shared mutability, or clever
  abstractions.
- Keep KISS, DRY, and SOLID proportional to this small crate. Do not add a framework for one call
  site or expose test-only helpers in production APIs.
- Use module and function documentation for non-obvious contracts. Avoid comments that narrate the
  code; explain invariants, safety boundaries, or why an apparently simpler approach is wrong.
- A successful change includes negative and boundary tests, not only the happy path. For storage,
  assert ordering and pagination. For SSE, assert frame shape, ordering, termination, cancellation,
  Unicode, and pacing. For auth and live mode, assert secrets never appear.
- Treat warning-free Clippy, documentation consistency, the offline dependency boundary, and a clean
  worktree as completion criteria, not optional cleanup.
