# 00 - Roadmap (single entry point)

Synthesis of plans 01-05. Read this first; drop into a numbered plan only for the detail behind a row.

## What this is, and what "battle ready" means

`no-llm-api` is an OpenAI Chat Completions server with no LLM behind it. Replies come from parquet
fixtures and are streamed as SSE at a configurable token rate, so a chat GUI can be developed and
regression-tested for free, offline, and repeatably.

Battle ready = these observable capabilities:

1. A real GUI (Open WebUI / LibreChat / Jan / Vercel AI SDK / openai-js in a browser) points at
   `http://127.0.0.1:8080/v1`, lists models, streams a reply, cancels it, and sees a spec-shaped error.
2. Every byte on the wire deserialises into `async-openai` 0.41.1 types - streamed and non-streamed.
3. The same request yields byte-identical output, on any machine, under concurrency.
4. Slow / flaky / unauthorized / tool-calling / refusal behaviour is reachable by one switch, seeded.
5. `cargo test` proves 1-4; `docker run` delivers it to someone who does not build Rust.

## Critical path

Milestones are days of work, ordered so the cheapest GUI wins land first. Each exit criterion is runnable.

| M | Goal | Exit criterion |
| --- | --- | --- |
| M1 | A GUI can connect, discover models, and see real errors. Enforcement is in place. | `cargo test --test http_api` green; `curl -s /v1/models \| jq .object` -> `"list"`; `curl -s -o /dev/null -w '%{http_code} %{content_type}' /v1/nope` -> `404 application/json`; `OPTIONS /v1/chat/completions` echoes `access-control-allow-headers`; CI green on ubuntu + windows |
| M2 | The stream is correct and never hangs. | `cargo test --test sse` green (emoji/CJK fixture concatenates byte-exact, last frame `data: [DONE]`, exactly one `finish_reason`); `cargo test --test async_openai` green with a **streamed** tool-call case with no env vars set |
| M3 | Output is deterministic and snapshot-locked. | `cargo test --test snapshots` green; test spawning 64 concurrent identical requests asserts one distinct response body; two identical POSTs produce byte-identical SSE including `id`/`created` |
| M4 | Behaviour is selectable without a rebuild or restart. | `cargo run -- --help` lists every setting; `cargo run -- --print-config` emits resolved JSON; `cargo test --test scenarios` asserts `--scenario slow` changes pacing, a `PATCH /_mock/scenario` injects a 500, `POST /_mock/reset` restores 200 |
| M5 | It ships and stays shipped. | `cargo tree --no-default-features \| Select-String reqwest` finds nothing; `docker build .` then `docker run -p 8080:8080` answers `/ready` and `GET /` under `read_only: true`; `docker compose up` -> Open WebUI lists models and streams |

Rationale for the order: M1 is the difference between "the GUI shows an empty dropdown" and "the GUI
works at all" (plan 03 section 1: three of five clients cannot reach chat without `GET /v1/models`).
M2 removes two hard parse breaks and a silent hang. M3 is a hard prerequisite for every assertion
after it, including a clean LibreChat demo (plan 03: `titleConvo` fires a second completion that the
current round-robin fallback consumes). M4 and M5 are leverage, not correctness.

## Deduplicated work items

Effort: S = under half a day, M = 0.5-2 days, L = 2+ days. "Source" lists every plan that proposed
the row; more than one source means the proposals were merged here.

| ID | Title | Source | Effort | M | Deps |
| --- | --- | --- | --- | --- | --- |
| R1 | `main.rs` consumes the `no_llm_api` lib instead of re-declaring modules (fixes double-compiled tests: 8 authored, not 15) | 04#4 | S | M1 | - |
| R2 | CI: fmt, `clippy -D warnings`, test matrix ubuntu+windows, `--doc`, cargo-deny, fresh-clone guard, `rust-toolchain.toml` (merged: 04 adds fresh-clone/deny, 05 adds toolchain pin + `docker build` + `--no-default-features`) | 04#5, 05#9 | S | M1 | R1 |
| R3 | Error envelope everywhere: `{message,type,param,code}`, `.fallback()`, `JsonRejection`/`QueryRejection` handlers, per-cause status mapping (merged: identical proposals) | 01A3, 03W4 | M | M1 | - |
| R4 | `GET /models` + `/models/{id}` at root and `/v1`, catalogue from `MODELS_PATH` -> `MODELS` -> dataset ids, 404 `model_not_found` (merged: identical proposals) | 01B1, 03W1 | M | M1 | R3 |
| R5 | `CorsLayer` (tower-http): mirror origin and request headers, expose `x-request-id`/`retry-after`/`x-ratelimit-*`, max-age 600 | 03W3 | S | M1 | - |
| R6 | `GET /health` + embed `index.html` via `include_str!` (merged three ways; `/ready` split out to R41) | 01D1, 03W10, 05#2 | S | M1 | - |
| R7 | `tests/http_api.rs` via `tower::oneshot`: 404/405/415 envelopes, `order` 400, pagination `has_more`/`first_id`/`last_id`, `metadata[k]=v` filter, delete-then-get, root vs `/v1` parity | 04#3 | M | M1 | R3 |
| R8 | `x-request-id` in and out, reused as the body's `request_id` (merged: 01 wanted the header, 03 wanted it exposed, 05 wanted it in spans) | 01D2, 03W5, 05#6 | S | M1 | R5 |
| R9 | SSE assertion harness `tests/support/sse.rs` + `tests/sse.rs` (ordering, delta==content, single trailing `[DONE]`, usage-frame position, stable ids, median-gap pacing) | 04#1, 04#8 | M | M2 | R1 |
| R10 | Boundary-safe streaming: precompute per-token byte pieces, flush on UTF-8 boundaries, `U+FFFD` on genuinely invalid, never abort (merged: 02 and 03 diagnosed the same tiktoken `decode` failure) | 02A1, 03W2 | M | M2 | R9 |
| R11 | Consumer-driven stream (`async_stream`, no detached task): immediate cancel, cancel counter, guaranteed terminal frame - error frame then `[DONE]` on internal failure (merged: 03 wanted cancel, 01 wanted termination, both rewrite the same loop) | 03W6, 01C2 | M | M2 | R10 |
| R12 | Tool calls stream as `ChatCompletionMessageToolCallChunk` with required `index`, `id`/`type` on the first fragment only, `arguments` split over >=2 frames, never `""`, parallel interleave (merged three ways; HARD BREAK today) | 01A1, 02B4, 03W12 | M | M2 | R11 |
| R13 | Response `message.content` must serialise as string; parts move to `content_parts` on `/messages` (merged; HARD BREAK today) | 01A2, 02C5 | M | M2 | - |
| R14 | Chunk-sequence fidelity: role-only first chunk, content-only middles, `delta:{}` + `finish_reason` last, refusal as deltas, `service_tier` on chunks, drop `audio` from deltas | 01C1, 02C6 | M | M2 | R11 |
| R15 | async-openai contract test default-on (delete the `ASYNC_OPENAI_COMPAT` early return); add streamed tool-call and refusal cases | 04#2 | S | M2 | R12 |
| R16 | `skip_serializing_if` on non-spec response fields; accept `"stream": null` (stopgap subset of R30 - land same day if R30 slips) | 03W7 | S | M2 | - |
| R17 | `X-Accel-Buffering: no` on SSE + a test asserting SSE is never gzipped | 03W9 | S | M2 | - |
| R18 | `sim::normalize` + `SelectionKey` + in-crate FNV-1a digest (never `DefaultHasher`), unit-tested per rule | 02A2 | M | M3 | - |
| R19 | `ScriptIndex` + pure `select()`; delete `rotation`/`cursors` atomics; delete the `index % len` wrap | 02A3, 02A6, 04#11 | M | M3 | R18 |
| R20 | Multi-turn matching ladder (longest prefix -> suffix-anchored -> last-user -> fallback) + never-fail fallback ladder + `X-Simulate-Match` header | 02A5 | M | M3 | R19 |
| R21 | `IdentityMode`: plan-derived `id`/`created`/`request_id`/`system_fingerprint`, injectable clock (merged: 04's `with_seed` is 02's deterministic mode) | 02A4, 04#11 | S | M3 | R19 |
| R22 | `insta` snapshots: 6 non-streamed bodies + 3 SSE transcripts + 3 error envelopes; golden transcripts replace 02's separate harness | 04#7, 02D2 | M | M3 | R21, R9 |
| R23 | Fixture drift guard (`tests/fixtures.rs`) + `regenerate_dataset --force` (README currently claims a regeneration that cannot happen) | 04#6 | S | M3 | - |
| R24 | `clap` CLI (`env` feature) + typed validated `Settings::resolve` + `--print-config` + README-drift test; tokenizer preset becomes an enum | 05#1 | M | M4 | - |
| R25 | Scenario bundle: struct, 7 built-ins via `include_str!`, `--scenario`, all randomness from a seeded `StdRng` | 05#3 | M | M4 | R24 |
| R26 | Timing engine: `ttft_ms`, seeded per-token jitter, burst, `chunk_tokens`, per-request tps, sleep-at-start-of-gap (merged: 05's scenario fields are 02's timing model) | 02B2, 05#3 | M | M4 | R11, R25 |
| R27 | Fault injection: `stall`, `drop`, `sse_error`, `http_error` (429/500/503 + `Retry-After`), `slow_then_recover`, plus `X-Simulate-*` / `x_simulate` per-request directives (merged three ways; see decision D2) | 02B1, 02B3, 03W13, 05#3 | M | M4 | R26, R3 |
| R28 | Auth middleware on `/chat/completions*` and `/models*`: `off` \| `any-bearer` \| `keys`, forbidden-key 403 path, `WWW-Authenticate` on 401 (merged; see decision D4 for naming) | 03W5, 05#1 | S | M4 | R3, R24 |
| R29 | Control plane: `ArcSwap<Scenario>` + `GET/PUT/PATCH /_mock/scenario` + `POST /_mock/reset` + `GET /_mock/models`, redacted request log (merged `/_sim` and `/_mock`; see decision D1) | 02B5, 05#13, 03 s5 | M | M4 | R25 |
| R30 | Lean POST response vs enriched GET/list object; add `tools` + `input_user` to stored objects; drop request echoes | 01B2 | M | M4 | R16 |
| R31 | Typed request enums (`stop`, `response_format`, `tools`, `tool_choice`, `audio`, `modalities`, `reasoning_effort`, `service_tier`), `logit_bias: i8`, `seed: i64`, role-discriminated messages with `tool_call_id` | 01B3 | L | M4 | R3 |
| R32 | Request validation: non-empty `messages`, scalar ranges, `top_logprobs` requires `logprobs`, unknown model -> 404 (config-gated) | 01B4 | S | M4 | R31 |
| R33 | Per-model simulation profile: `context_window`, `capabilities{tools,vision,audio,reasoning}`, `latency{ttft_ms,tps,jitter}` (merged: 05's `[models]` table is 03's `data/models.json`) | 03W8, 05#3 | M | M4 | R4, R26 |
| R34 | `reasoning_content` on message and delta (emitted before content) + `reasoning_tokens` in usage + `thinking_style: tag` variant (merged) | 02C3, 03W11 | S | M4 | R14 |
| R35 | `n > 1`: request field, per-choice `index`, deterministic derived alternatives (merged) | 01C3, 02C4 | M | M4 | R19, R14 |
| R36 | Human-authorable fixture source (YAML) + `fixtures build` binary + linter; new nullable parquet columns; deletes ~330 lines of hardcoded rows at `dataset.rs:669-996` | 02C1 | L | M4 | R23 |
| R37 | Behaviour-coverage fixtures: markdown, length, refusal, content_filter, json_schema, reasoning, empty, audio, multi-byte, 2k-token | 02C2 | M | M4 | R36 |
| R38 | Graceful shutdown on SIGINT/SIGTERM draining active SSE (`docker stop` currently truncates streams after a 10s SIGKILL wait) | 05#4 | S | M4 | R11 |
| R39 | `tower-http` `TraceLayer` + DEBUG access log + targets re-enabled + header/secret exclusion test | 05#6 | M | M4 | R8 |
| R40 | Feature-gate live mode: `default = []`, `live = ["async-openai/chat-completion","async-openai/rustls"]`, base dep `chat-completion-types`; clear "built without live" exit | 05#5 | M | M5 | R24 |
| R41 | `GET /ready` (script count, scenario, tokenizer; never pings upstream), exempt from auth and fault injection | 05#2 | S | M5 | R6, R25 |
| R42 | Live hardening: credentials into `Settings` as `Redacted`, one redaction pass before any parquet write, `data/live/` default + widened `.gitignore`, log-hygiene test, timeout/retry/concurrency caps | 05#10 (L2-L7) | M | M5 | R40, R24 |
| R43 | Dockerfile (distroless nonroot, dependency-cache layer, baked sample dataset, `health` subcommand for HEALTHCHECK) + `.dockerignore` + GHCR publish | 05#7 | M | M5 | R6, R40, R41 |
| R44 | `docker-compose.yml` (Open WebUI) + `docker-compose.demo.yml` (bundled `index.html`) + README walkthrough | 05#8 | S | M5 | R43, R4 |
| R45 | Doc reconciliation: ~~delete `GEMINI.md`~~, ~~archive `task.md`~~, ~~promote `chat_completions_scope.md` to `docs/spec/`~~, ~~fix two stale `AGENTS.md` claims~~, ~~add the testing-layout rule~~, README env/CLI table (merged; everything but the CLI table landed in the cleanup commit) | 05#11, 04#10 | S | M5 | R24 |
| R46 | OpenAPI slimming: track a pruned chat-completions extract, untrack the full `openapi.yaml`, ~~`scripts/fetch-openapi.*` with pinned sha256~~, ~~drop `openai-func-enums/`~~ (see decision D10; the fetch scripts and provenance file landed, the pruning did not) | 05#12 | S | M5 | - |
| R47 | Release plumbing: `CHANGELOG.md` with a Wire-behaviour section, `docs/versioning.md`, `x-no-llm-api-version` header, release workflow (5 targets + GHCR), crates.io metadata | 05#15 | M | M5 | R43 |
| R48 | Playwright `e2e/` driving `index.html` (never part of `cargo test`) | 04#12 | M | M5 | R6 |
| R49 | `--metrics` with six hand-rolled Prometheus counters | 05#14 | S | M5 | R39 |
| R50 | Accept-and-ignore remaining spec request fields (`logprobs`, `top_logprobs`, `prediction`, `web_search_options`, `verbosity`, `prompt_cache_key`, `safety_identifier`, `functions`, `include_obfuscation`) | 01D3 | S | M5 | R31 |

Unit tests for `config::Settings::load` and `tokenizer::load` (04#9) are folded into R24 - the
rewrite and its tests are one change. 02's `/_sim/requests` log (02D1) is folded into R29.

## Open decisions

Real disagreements between plans. Each needs a call before the owning milestone starts.

| # | Conflict | Options | Tradeoff | Recommendation |
| --- | --- | --- | --- | --- |
| D1 | Control-plane namespace, listener and default. 02 s4.2: `/_sim/*`, off by default, **separate** loopback listener, bearer token if non-loopback. 05 s2: `/_mock/*`, **on** by default, same listener. 03 s4 assumes `/_mock/models`. | (a) 02's design (b) 05's design (c) hybrid | A separate listener is genuinely safer but breaks every containerised GUI setup (one published port) and doubles the test harness. On-by-default on the same port means a published container port lets anyone reshape the system under test. | Hybrid: one namespace `/_mock/*` on the API listener; enabled by default **only when the bind address is loopback**; when bind is non-loopback, require `NO_LLM_CONTROL_PLANE=1` plus a token and log a WARN. Keep 02's redaction rules for the request log verbatim. |
| D2 | Three injection mechanisms. 02: per-request `X-Simulate-*`/`x_simulate`. 05: scenario-file `[failures]`. 03W13: `INJECT_RATE_LIMIT_EVERY_N`. | (a) pick one (b) layer them | Per-request is the only parallel-safe form; scenario files are the only shareable form. A third env counter adds a code path nobody will keep tested. | Layer two: scenario supplies defaults, `X-Simulate-*` overrides per request (precedence: request field > header > scenario > flag). Drop `INJECT_RATE_LIMIT_EVERY_N`; `X-Simulate-Fault: http_error;status=429` covers it. |
| D3 | The word "scenario" means two things, in two formats. 02 s6: `fixtures/*.scenario.yaml` = conversation fixtures. 05 s2: `scenarios/*.toml` = latency/failure profile. | (a) two words, two formats (b) two words, one format (c) one concept | Colliding vocabulary in the same config surface will confuse every contributor. Two serde formats means two dependencies. | (b): rename 02's artefact to a **fixture set** (`fixtures/*.yaml`), keep **scenario** for the behaviour profile, and use YAML for both (`serde_yaml_ng` 0.10, pinned - `serde_yaml` is archived). Block scalars are mandatory for markdown fixtures, so YAML wins the tiebreak; built-ins still `include_str!`. |
| D4 | Auth variable names and mode taxonomy. 03: `AUTH_MODE=permissive\|strict`, `AUTH_KEYS`, `AUTH_FORBIDDEN_KEYS`. 05: `NO_LLM_AUTH_MODE=off\|any-bearer\|token`, `NO_LLM_AUTH_TOKEN`. | (a) 03 (b) 05 (c) union | Unprefixed `AUTH_*` risks colliding with a sibling GUI container's env in compose. 05's `any-bearer` mode is the one real GUIs need (they insist on a non-empty key field) and 03 lacks it; 03's forbidden-key 403 path is the one Jan's key rotation needs and 05 lacks it. | (c) with 05's prefix: `NO_LLM_AUTH_MODE=off\|any-bearer\|keys`, `NO_LLM_AUTH_KEYS`, `NO_LLM_AUTH_FORBIDDEN_KEYS`. Default `off` (no regression). Same rule for `NO_LLM_CORS_ORIGINS` over 03's `CORS_ALLOW_ORIGINS`. |
| D5 | `system_fingerprint` format, three ways: `fp_<10 hex>` from a dataset digest (01D2), `fp_mock_{plan_digest:08x}` (02 s1.3), `fp_nollm_X_Y_Z` (05#15). | (a)/(b)/(c) | Realistic shape matters to GUIs that display it; plan-derived content is what makes "the backend plan changed" assertable; encoding the build version there collides with both. | `fp_` + 10 hex of the plan digest: 01's shape, 02's derivation. Version pinning goes in `x-no-llm-api-version` and `/health`, which 05 already concedes. |
| D6 | SSE keep-alive `:ping`. 01C2: make it opt-in, default off, because real chat-completions traffic has none. 03 s2: verified safe against openai-node, `reqwest-eventsource`, browser `EventSource` and `index.html`; leave it. | (a) off (b) on (c) conditional | 03's parser evidence is solid, so "on" breaks nothing - but fidelity to real OpenAI is the product, and a GUI that mishandles comments is a bug the mock should expose. 02's `stall` fault needs keep-alives to be exercisable. | (c): default off for fidelity; auto-enable during a `stall` fault; keep a config flag. Note 03's finding that it never fires today (min pacing 1 token/s < 15s interval), so this is not a behaviour regression. |
| D7 | Non-spec response fields. 01B2: delete the request echoes, lean POST vs enriched GET/list. 03W7: keep the fields, add `skip_serializing_if`. | (a) 01 (b) 03 (c) both | 03's is one line per field and lands today; 01's is spec-accurate and matches the spec's own `listChatCompletions` example. They are not exclusive. | Both, in order: R16 (03) as the same-day stopgap, R30 (01) as the real fix. Do not stop at R16. |
| D8 | **SETTLED.** Vendored `./async-openai` clone as the reference. 01 and 03 cite it; 05 s6 showed it was version 0.30.1 while we depend on 0.41.1. | (a) keep (b) pin to v0.41.1 (c) use the cargo registry copy | A skewed reference clone is worse than none - it invents constraints the real client does not have. | (c). Both untracked clones (`./async-openai`, `./openai-func-enums`) were deleted in the cleanup commit; `AGENTS.md` now points at `~/.cargo/registry/src/*/async-openai-0.41.1/`, which is guaranteed to match `Cargo.lock`. Citations in plans 01 and 03 that use `./async-openai/...` paths refer to the deleted 0.30.1 clone - re-resolve them against the registry copy. |
| D9 | Where the stream loop lives. 02: new `src/sim/stream.rs`. 03W6: rewrite in place in `routes.rs`. | (a)/(b) | Doing R10/R11/R12 in `routes.rs` and then moving them for R26 means rewriting the same loop twice. | Extract to `src/sim/stream.rs` once, during R11, consumer-driven (`async_stream`). Sequencing decision, not a design one - but it must be made before M2 starts. |
| D10 | **PARTLY SETTLED.** Spec citations point at `openapi.documented.yml`, which 03 verified is **not tracked** (`.gitignore:14`); only the stale 3.0.0 `openapi.yaml` was. Plans 01 and 04 cite `openapi.documented.yml` line numbers. | (a) track the big file (b) track a pruned extract (c) leave it | Line-number citations into an untracked 2.2 MB file cannot be checked by anyone who clones the repo, which quietly rots every acceptance criterion that references a schema. | Interim: `openapi.yaml` was refreshed to the current upstream 3.1.0 spec (2.7 MB, commit `5c044be3bf3a`, 2026-07-23) with `scripts/fetch-openapi.ps1`/`.sh` and provenance in `openapi.provenance.json`; see `docs/spec/upstream-openapi.md`. R46's pruned extract is still wanted to get the tracked size down - when it lands, re-anchor the plan citations to schema names in that extract. |

## First three commits

Small, ordered, each independently verifiable. No behaviour change in the first.

**Commit 1 - enforcement scaffolding (R1, R2).**
Make `src/main.rs` a `use no_llm_api::{...}` shim; add `.github/workflows/ci.yml`, `deny.toml`,
`rust-toolchain.toml`.
Verify: `cargo test` reports 8 tests across lib + integration, not 15 (proves modules are no longer
compiled into both targets); `cargo clippy --all-targets --all-features --locked -- -D warnings`
clean; push and confirm the workflow is green on ubuntu-latest and windows-latest, and that the
`fresh-clone` job fails if `/async-openai` is ever committed.

**Commit 2 - the silent hang (R9, R10, R11).**
Land `tests/support/sse.rs` and `tests/sse.rs` first, including a fixture whose reply contains
`"cafe\u0301 <emoji> <CJK>"`. That test must fail on the current tree - it is the proof the bug is
real (tiktoken `decode` errors on a partial multi-byte token and the handler `return`s without a
final chunk or `[DONE]`). Then replace the decode-of-growing-buffer with precomputed byte pieces
plus a boundary-safe flush, move the loop into `src/sim/stream.rs` as a consumer-driven stream, and
guarantee a terminal frame on every path.
Verify: `cargo test --test sse` green; concatenated deltas equal the fixture byte-for-byte; last
frame is exactly `data: [DONE]`; a forced decode failure still emits an error frame then `[DONE]`;
a stream dropped after 2 frames at `TOKENS_PER_SECOND=1` increments the cancel counter within 1.5s.

**Commit 3 - a GUI can connect (R3, R4, R6, R5).**
Error envelope with all four keys + `.fallback()` + `JsonRejection` handler; `GET /models` and
`/models/{model}` at root and `/v1`; `GET /health`; `include_str!` for `index.html`; `CorsLayer`.
Verify: plan 03 section 8 smoke commands 1, 2 and 6 pass; `cargo test --test http_api` green;
new async-openai test asserts `client.models().list()` deserialises and returns >= 1 model;
`GET /` returns 200 when the binary is started from a different working directory.

## Non-goals for the short term

| Non-goal | Reason |
| --- | --- |
| `POST /v1/responses` | Dozens of typed streaming event types and its own state model (01 s5). Weeks of work; most GUIs still support Chat Completions. Revisit only for a Responses-only target GUI. |
| `/v1/audio/*` | Multipart upload and binary bodies; only voice UIs need it (01 s5). |
| `/v1/embeddings`, `/v1/moderations` | Cheap (~60 lines each) but nothing in a chat flow calls them. Add on demand, not on spec. |
| OpenTelemetry, Jaeger, latency histograms, dashboards, `/debug/pprof` | Instruments the mock rather than the GUI under test (05 s3). Six counters behind `--metrics` is the whole budget. |
| Kubernetes manifests, Helm, multi-replica or shared state | A fixture server is single-process by design; state lives in memory on purpose (05 non-goals). |
| Real authentication, in-process TLS | The mock must be trivially reachable. `any-bearer` + optional key list is enough to exercise a GUI's key-entry UX. |
| Git history rewriting to purge `openapi.yaml` | Removing it from HEAD keeps clones and the crates.io tarball small; rewriting a young repo's history costs more than it saves (05 s6). |
| Session affinity by `metadata.conversation_id` (02D3) | Solves GUIs that do not resend history - none of the five surveyed clients behave that way. |
| Runtime JSON-Schema validation of `json_schema` fixtures (02D4) | A build-time lint is enough; a runtime schema dependency is not justified. |
| crates.io publish | Blocked on the wire shape settling plus missing `Cargo.toml` metadata (05 s4). Do it after M5, not during. |

## Risks

| Risk | Why it could stall the effort | Mitigation |
| --- | --- | --- |
| M2 and M4 both rewrite the streaming loop | R10-R14 (correctness) and R26-R27 (timing/faults) touch the same code; doing them in `routes.rs` means a second rewrite and a second round of test churn | Settle decision D9 before M2 starts; extract `src/sim/stream.rs` once, with the timing hook present but inert. |
| R31 (typed request enums) is the only L item on the critical path and touches `model.rs`, `service.rs`, `live.rs`, `dataset.rs` | An unbounded refactor that blocks R32 and can absorb a whole week | Split as plan 01 suggests: pass 1 = `stop`/`response_format`/`service_tier`/`reasoning_effort`/`modalities`; pass 2 = `tools`/`tool_choice`/role-discriminated messages. Land R32's validation on pass 1. |
| Determinism work (M3) invalidates fixtures and snapshots written in M2 | Snapshots taken before `IdentityMode` need re-approval; the fallback answer for off-script prompts changes | Land R21 before R22; until then use `insta` redactions for `id`/`created`/`request_id` only, and snapshot only prompts that match a script (04 s4). |
| Timing assertions flake on CI runners | A flaky pacing test gets `#[ignore]`d, and the product's core knob goes unverified again | Assert median inter-frame gap with 50% tolerance, keep all timing tests in one file, and add one strict lower-bound test at `TOKENS_PER_SECOND=2` (04 s3). Prefer `tokio::time::pause` where the harness allows. |
| Live mode drags TLS and money into an offline mock | `reqwest`/`rustls`/`ring`/`secrecy` are in the default build today; an accidental live run during a GUI dev loop bills real money | R40 first in M5 (`default = []`), then R42's caps and redaction. Keep `NO_LLM_API_LIVE=1` as the single opt-in test gate and blank it in CI (04 s6). |
| Spec citations rot (decision D10) | Acceptance criteria referencing `openapi.documented.yml:NNNNN` are uncheckable on a fresh clone | R46 early: track the pruned extract and cite schema names, not only line numbers. |
| Scope creep from five plans into one sprint | 50 rows is more than days of work; the temptation is to start at R50 | The milestone column is the contract. Nothing from M4/M5 starts before its milestone's exit criterion is met, and the non-goals table above is binding. |
| Control plane turns the mock into an attack surface | On-by-default admin routes plus ignored `Authorization` on a published container port (05 s4) | Decision D1's loopback-gated default, WARN on non-loopback bind with auth off, and 02's header redaction in the request log. |
