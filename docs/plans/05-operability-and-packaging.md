# 05 - Operability, Packaging and Developer Experience

Scope: how the mock is configured, observed, shipped and maintained. Wire-shape work
(routes, SSE, error envelope, model catalogue) belongs to plans 01-04; this plan only
owns the switches that select those behaviours and the plumbing around them.

Verified baseline for this plan:

| Fact | Evidence |
| --- | --- |
| Config is env-only, hand-parsed, no file/CLI layer | `src/config.rs:49-93` (`Settings::load`) |
| `config.rs` has no `#[cfg(test)]` block, against repo convention | grep for `cfg(test)` in `src/config.rs` returns nothing; `AGENTS.md` "Project Structure" requires inline tests per module |
| Tokenizer preset is validated *after* config load, error does not list valid values | `src/config.rs:55` stores raw string; `src/tokenizer.rs:19-27` `UnsupportedPreset` |
| `LIVE_RECORD` silently falls back to `false` on typos | `src/config.rs:64-68` (`matches!("1"|"true"|"yes")`) |
| Credentials are read ad hoc inside the backend, not via `Settings` | `src/live.rs:99-131` (`build_client`) |
| No graceful shutdown although `tokio` has `signal` enabled | `src/main.rs:47` bare `axum::serve`; `Cargo.toml:19`; contrast `tests/async_openai.rs:53` which does use `with_graceful_shutdown` |
| Startup writes the dataset if missing | `src/main.rs:24` `ensure_sample_dataset(path)` |
| `index.html` is read from the process CWD on every `GET /` | `src/http/routes.rs:57-62` |
| Only 5 log call sites, no HTTP access log, no request id, no `tower-http` | grep `tracing::` -> `main.rs:49`, `routes.rs:135,154`, `service.rs:159`, `dataset.rs:19`; `Cargo.toml:26` has `tower` only |
| Default build already links a full TLS/HTTP client stack | `Cargo.lock` contains `reqwest` (l.1696), `rustls` (l.1780), `hyper-rustls` (l.962), `ring` (l.1738), `secrecy` (l.1885); 293 packages total |
| async-openai 0.41.1 can ship types without a client | registry `async-openai-0.41.1/Cargo.toml` `[features]`: `chat-completion-types` pulls only `derive_builder`+`bytes`, whereas `chat-completion = ["chat-completion-types", "_api"]` and `_api` pulls `reqwest`, `secrecy`, `tokio-util`, `url`, ... ; `rustls = ["dep:reqwest", "reqwest/rustls"]` |
| `AGENTS.md` claim "tiktoken-rs already pulls in async-openai" is stale | registry `tiktoken-rs-0.12.0/Cargo.toml`: `async-openai = ["dep:async-openai"]`, dependency is `optional = true`, `default-features = false`, `features = ["chat-completion-types"]` |
| `openapi.yaml` is the only tracked spec, refreshed and provenance-recorded (RESOLVED) | `openapi.provenance.json`; `docs/spec/upstream-openapi.md` |
| Vendored clones deleted (RESOLVED); reference is the cargo registry copy matching `Cargo.lock` | `AGENTS.md`, "Project Structure" and "Async-openai Compatibility Notes" |
| No CI, no Dockerfile, no toolchain pin | `Test-Path .github` -> False, `Test-Path Dockerfile` -> False, `git ls-files rust-toolchain*` -> empty |
| Cargo metadata is not publishable as-is | `Cargo.toml:1-4` has no `description`, `license`, `repository`, `rust-version` |

---

## 1. Configuration overhaul

### Assessment of the current implementation

`Settings::load` (`src/config.rs:49-93`) reads six variables with
`env::var(..).unwrap_or_else(literal)`. Problems, in order of impact:

1. **Defaults are string literals scattered through the function** (`"127.0.0.1:8080"` l.51,
   `"30"` l.53, `"cl100k_base"` l.55, `"parquet"` l.56, `"data/conversations.parquet"` l.57-58).
   They are duplicated in `README.md`, `.env.example` and `src/bin/recorder.rs:26` (`"cl100k_base"` again).
   Three copies of the same default is already one drift bug waiting.
2. **No `--help`.** A tester cannot discover the knobs without reading source or README.
   The server binary has no `clap` entry point even though `clap 4.6.4` is already a
   dependency (`Cargo.toml:44`) and used by `src/bin/recorder.rs:14-28`.
3. **Validation is partial and mis-ordered.** Dataset source is validated (l.59-76) before
   the bind address (l.78-86), so the error a user sees depends on parse order rather than
   on severity. Tokenizer preset is not validated here at all - it fails later in
   `tokenizer.rs:26` after the dataset has already been read from disk.
4. **Silent coercion.** `LIVE_RECORD=ture` becomes `false` (l.64-68); no warning.
5. **Errors are not actionable.** `SettingsError` variants (l.38-47) echo the bad value but
   never state the accepted set or the fix.
6. **Credentials bypass the type.** `LiveSettings` (l.26-31) carries only `record`/`record_path`;
   the six OpenAI and four Azure variables are read directly in `src/live.rs:99-131`, so
   `Settings` is not a complete description of runtime state and cannot be printed or tested.
7. **Untested.** No inline test module, so none of the above is pinned by an assertion.

### Proposed shape

Layering, lowest precedence first:

```
serde defaults  ->  scenario file (behaviour bundle only)  ->  env  ->  CLI flags  ->  control-plane overrides at runtime
```

Deliberately **two** mechanisms, not one generic config stack:

- **Plumbing** (bind address, dataset path, tokenizer, log level, feature switches) comes from
  one `clap::Parser` struct with `#[arg(long, env = "...", default_value = "...")]`. clap gives
  defaults + env + CLI + `--help` from a single declaration; requires adding the `env` feature
  to `clap` (`Cargo.toml:44` currently `features = ["derive"]`). No new dependency, no
  figment/config crate, and `--help` becomes the authoritative reference.
- **Behaviour** (latency curve, failure injection, model catalogue, dataset selection) comes from
  a named scenario, see section 2. Scenarios are the only file-based layer, because they are the
  only part a tester wants to version and share.

Structure:

```rust
// src/config.rs
pub struct Cli { /* clap derive, flat, one field per setting */ }
pub struct Settings {            // typed, validated, no Strings-as-enums
    pub server: ServerSettings,  // bind, shutdown_grace, cors, auth
    pub simulation: SimulationSettings, // scenario name + resolved behaviour
    pub dataset: DatasetSettings,
    pub tokenizer: TokenizerSettings,   // parsed preset enum, not String
    pub observability: ObservabilitySettings,
    pub live: Option<LiveSettings>,     // Some(..) only when source == live
}
impl Settings { pub fn resolve(cli: Cli) -> Result<Self, SettingsError>; }
```

Rules:
- `TokenizerPreset` becomes an enum with `FromStr`/`ValueEnum`, so `tokenizer.rs:19-27` degrades
  to an infallible match and the invalid-value error is produced during `resolve`.
- `resolve` collects *all* validation failures and reports them together, each with a fix.
- Secrets are stored in a `Redacted(String)` newtype with a manual `Debug` printing `"***"`.
  No new crate needed (`secrecy` only exists in the tree today via `async-openai/_api`).
- `--print-config` dumps the resolved `Settings` as pretty JSON and exits 0. This is the
  acceptance hook for every config item below.
- Per `AGENTS.md` ("Security & Configuration Tips"), every variable listed here must land in the
  `README.md` table *and* `.env.example` in the same commit. Add a test that reads
  `README.md` and asserts every `--long-flag`/env name emitted by clap appears in it; that turns
  the convention into CI enforcement instead of a review checklist.

### Error message examples (target wording)

| Input | Today | Target |
| --- | --- | --- |
| `TOKENIZER_MODEL=gpt2` | `unsupported tokenizer preset \`gpt2\`` (after dataset load) | `TOKENIZER_MODEL=gpt2 is not supported. Valid: cl100k_base, o200k_base, p50k_base, p50k_edit, r50k_base.` at startup |
| `BIND_ADDRESS=8080` | `invalid bind address: 8080` | `BIND_ADDRESS=8080 must be host:port, e.g. 127.0.0.1:8080 or 0.0.0.0:8080.` |
| missing dataset file | parquet/arrow IO error | `DATASET_PATH=data/x.parquet does not exist. Run: cargo run --bin regenerate_dataset, or pass --dataset-path.` |
| `DATASET_SOURCE=live`, no keys | `missing environment variable \`OPENAI_API_KEY\`` | `live mode needs either OPENAI_API_KEY, or AZURE_OPENAI_ENDPOINT + AZURE_OPENAI_API_KEY + AZURE_OPENAI_DEPLOYMENT_NAME (Azure wins when the endpoint is set). For offline use keep DATASET_SOURCE=parquet.` |
| `LIVE_RECORD=ture` | silently false | `LIVE_RECORD=ture is not a boolean. Use true/false (also accepted: 1/0, yes/no).` |

### Complete setting inventory implied by this plan

Existing (keep name, add flag + validation):

| Env | Flag | Default | Notes |
| --- | --- | --- | --- |
| `BIND_ADDRESS` | `--bind` | `127.0.0.1:8080` | loopback on host; Docker image overrides to `0.0.0.0:8080`, see 4 |
| `TOKENS_PER_SECOND` | `--tokens-per-second` | `30` | NonZeroU32; scenario may override |
| `DATASET_SOURCE` | `--dataset-source` | `parquet` | `parquet`\|`live`; `live` requires the `live` cargo feature |
| `DATASET_PATH` | `--dataset-path` | `data/conversations.parquet` | |
| `TOKENIZER_MODEL` | `--tokenizer` | `cl100k_base` | ValueEnum |
| `LIVE_RECORD` | `--live-record` | `false` | strict bool parse |
| `LIVE_RECORD_PATH` | `--live-record-path` | `data/live/live.parquet` | moved under `data/live/`, see 5 |

New, owned by this plan:

| Env | Flag | Default | Purpose |
| --- | --- | --- | --- |
| `NO_LLM_SCENARIO` | `--scenario` | `default` | selects the behaviour bundle (section 2) |
| `NO_LLM_SCENARIO_FILE` | `--scenario-file` | none | extra/override scenario definitions (TOML) |
| `NO_LLM_LOG` | `--log` | `info` | alias for `RUST_LOG`-style filter; keeps `RUST_LOG` working |
| `NO_LLM_LOG_FORMAT` | `--log-format` | `compact` | `compact`\|`json` (json for CI log scraping) |
| `NO_LLM_CORS_ORIGINS` | `--cors-origin` (repeatable) | `*` | browser GUIs need this; implementation in plan 01/02, switch here |
| `NO_LLM_AUTH_MODE` | `--auth-mode` | `off` | `off`\|`any-bearer`\|`token`; pairs with `NO_LLM_AUTH_TOKEN` |
| `NO_LLM_AUTH_TOKEN` | `--auth-token` | none | expected bearer when `auth-mode=token`; `Redacted` |
| `NO_LLM_SEED` | `--seed` | `0` | deterministic script selection instead of `AtomicUsize` round-robin |
| `NO_LLM_SHUTDOWN_GRACE_MS` | `--shutdown-grace-ms` | `2000` | drain in-flight SSE on SIGTERM |
| `NO_LLM_REQUEST_TIMEOUT_MS` | `--request-timeout-ms` | `0` (off) | must stay off by default: slow SSE is the product |
| `NO_LLM_METRICS` | `--metrics` | `false` | exposes `/metrics` |
| `NO_LLM_INDEX_PATH` | `--index-path` | embedded | overrides the built-in test GUI instead of CWD lookup |
| `NO_LLM_CONTROL_PLANE` | `--control-plane` | `true` | master switch for `/_mock/*` routes |
| `MODELS_PATH` | `--models-path` | none | model catalogue file for `GET /v1/models`, owned by `docs/plans/03-chat-gui-compatibility.md`; listed here because `Settings` must resolve it |
| `MODELS` | `--models` | from dataset | comma-separated catalogue override, same plan |

Behaviour knobs that are *scenario fields first*, exposed as flags only for one-off use:
`--ttft-ms`, `--jitter-ms`, `--fail-rate`, `--fail-status`, `--fail-after-tokens`,
`--models` (comma list for the catalogue), `--max-n`.

---

## 2. Scenario / profile concept

One switch, one word: `--scenario flaky` or `NO_LLM_SCENARIO=flaky`. A scenario is a named,
serialisable bundle:

```toml
# scenarios/flaky.toml  (or a [scenarios.flaky] table in one file)
name            = "flaky"
description     = "5 percent 500s, one mid-stream abort in ten, slow first token"
dataset         = "data/conversations.parquet"
seed            = 42
tokens_per_second = 12
ttft_ms         = 900          # delay before the first SSE frame
jitter_ms       = 150          # +/- per-token jitter, seeded
[failures]
rate            = 0.05
status          = 500
kind            = "server_error"    # maps to the OpenAI error envelope type
abort_mid_stream_rate = 0.10
retry_after_s   = 2                 # emitted with 429s
[models]
ids = ["gpt-4o", "gpt-4o-mini", "o3-mini"]
[capabilities]
max_n           = 2
stream_tool_call_args = true
emit_reasoning  = false
```

Built-in set, compiled in with `include_str!` so a container needs no mounted files:

| Scenario | What it exercises in a GUI |
| --- | --- |
| `default` | happy path, 30 tok/s, no failures - the current behaviour, unchanged |
| `instant` | `tokens_per_second` very high, `ttft_ms=0` - fast test suites, no sleeps |
| `slow` | 3 tok/s, `ttft_ms=2500` - spinners, "stop generating", timeout handling |
| `flaky` | mixed 429/500 plus mid-stream aborts - retry and error-toast paths |
| `unauthorized` | `auth-mode=token` with every request rejected 401 - key-entry UX |
| `tools` | tool_calls with incrementally streamed arguments |
| `refusals` | refusal + content_filter finish reasons |

Resolution order: built-in scenario -> `--scenario-file` override of the same name ->
env -> individual CLI flags -> control-plane mutation. Deterministic by construction: every
random decision draws from a `StdRng` seeded with `scenario.seed`, so `--scenario flaky --seed 42`
reproduces the same failure positions on every run, which is what makes a flaky-path test
assertable rather than merely observable.

Composition with the simulation control plane (`docs/plans/02-simulation-engine.md`, section 4,
which owns the `/_mock/*` surface): the resolved scenario lives in an
`ArcSwap<Scenario>` in `AppState`.

| Route | Behaviour |
| --- | --- |
| `GET /_mock/scenario` | active scenario, fully resolved, plus source of each field |
| `PUT /_mock/scenario` | switch by name: `{"name":"flaky"}` |
| `PATCH /_mock/scenario` | partial override of any field, for a single test case |
| `POST /_mock/reset` | back to the startup-resolved scenario and reseeded RNG |

A GUI test then reads: boot once with `--scenario default`, `PATCH` to inject a 500 for one
assertion, `POST /_mock/reset`, continue. No restart, no env juggling, and CI keeps one
long-lived server. Gate all of `/_mock/*` behind `NO_LLM_CONTROL_PLANE` so the same binary can
be handed to someone who must not be able to reshape it.

---

## 3. Observability

This is a test fixture. Target: a developer watching stdout can tell which script answered,
how fast, and why a request failed. Nothing more.

### Levels

| Level | Events |
| --- | --- |
| ERROR | token decode failure (`routes.rs:135`), chunk serialisation failure (`routes.rs:154`), parquet append failure, live upstream error, panic in a handler |
| WARN | non-loopback bind with `auth-mode=off`; unknown model requested; no script matched (fallback selection used); malformed request rejected; client disconnected mid-stream; deprecated request field seen (`function_call`, `max_tokens`) |
| INFO | one redacted startup summary (scenario, dataset path, script count, tokenizer, bind, features); "listening on"; scenario switched via control plane; shutdown with in-flight count |
| DEBUG | per-request line: method, path, status, latency, request id, model, `stream`, `store`, chosen script id, token count |
| TRACE | each SSE frame payload, each injected-failure die roll |

Access logging belongs at DEBUG, not INFO: a GUI test suite drives hundreds of requests and an
INFO line each turns the console into noise. Replace the current `with_target(false)` setting
(`src/main.rs:44`) with targets on, so `NO_LLM_LOG=no_llm_api::service=debug` is usable.

### Request ids and spans

Add `tower-http` (`trace`, `cors`, `request-id`, `set-header`) - it is the missing dependency
behind three separate gaps in this repo. Middleware order: `SetRequestId` (honour inbound
`x-request-id`, else UUID v4) -> `PropagateRequestId` (echo it back) -> `TraceLayer` with a
custom `MakeSpan` carrying `request_id`, `method`, `path`, `model`, `stream`, `scenario`.
Then feed the same id into the response body's `request_id` field (already part of the wire
contract per `docs/spec/chat-completions-scope.md`, "ChatCompletion" section), so a screenshot of a broken
GUI response is enough to find its server-side span.

Never log headers wholesale. `tower-http`'s default span does not include headers, which is the
behaviour we want; if a custom `MakeSpan` is written, `authorization`, `api-key` and
`openai-organization` must be explicitly excluded, and a unit test should assert a fake bearer
token never appears in captured log output.

### Health

Two routes, because they answer different questions once live mode exists:

| Route | Semantics | Body |
| --- | --- | --- |
| `GET /health` | liveness: process is serving. Always 200 once bound. | `{"status":"ok","version":"0.2.0","uptime_s":12}` |
| `GET /ready` | readiness: dataset loaded, tokenizer built, script count > 0, and in live mode the credentials resolved. 503 with a reason otherwise. | `{"status":"ready","dataset":{"source":"parquet","scripts":12},"scenario":"default","tokenizer":"cl100k_base"}` |

In parquet mode readiness is nearly redundant, because `src/main.rs:22-46` loads everything
*before* `TcpListener::bind`, so a bound socket already implies readiness. Keep both anyway:
`/ready` is the natural place to surface script count and scenario for a compose `depends_on`
gate, and it becomes genuinely non-trivial if dataset hot-reload (plan 02/03) lands.
Readiness must not call the upstream in live mode - that would bill money on every probe.
Both routes must be exempt from auth and from failure injection, otherwise `--scenario flaky`
kills your own container.

### Metrics

Optional, off by default, behind `--metrics`. Worth having: `http_requests_total{route,status}`,
`chat_completions_total{stream,scenario}`, `sse_streams_active`, `sse_streams_aborted_total`,
`injected_failures_total{kind}`, `tokens_emitted_total`. That is six series, hand-writable as a
Prometheus text-format string in ~60 lines with an `AtomicU64` set; adding
`metrics`/`metrics-exporter-prometheus` for six counters is not obviously worth the dependency
weight and is the lower-priority option.

Explicitly overkill for this repo, do not build: OpenTelemetry/OTLP export, `tracing-opentelemetry`,
Jaeger wiring, latency histograms with buckets, log shipping, `/debug/pprof`, distributed trace
propagation beyond echoing `x-request-id`, and any dashboard. If someone needs those, they are
testing the mock rather than testing their GUI.

---

## 4. Packaging and distribution

### Dockerfile

Two-stage, glibc distroless, non-root, no shell in the final image. `index.html` must be
embedded at compile time (`include_str!`, see `NO_LLM_INDEX_PATH`) rather than read from CWD as
`src/http/routes.rs:57-62` does today, otherwise `GET /` 500s in any image whose WORKDIR does
not happen to contain the file.

```dockerfile
# syntax=docker/dockerfile:1.7
FROM rust:1.90-slim-bookworm AS builder
WORKDIR /build
ENV CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0
# dependency layer: cached until Cargo.toml/lock change
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src src/bin \
 && echo 'fn main(){}' > src/main.rs \
 && echo '' > src/lib.rs \
 && echo 'fn main(){}' > src/bin/recorder.rs \
 && echo 'fn main(){}' > src/bin/regenerate_dataset.rs \
 && cargo build --release --locked --bin no-llm-api \
 && rm -rf src
COPY src ./src
COPY index.html ./index.html
COPY scenarios ./scenarios
# touch so cargo rebuilds the real sources; default features only => no TLS/HTTP client
RUN cargo build --release --locked --bin no-llm-api \
 && strip target/release/no-llm-api

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /build/target/release/no-llm-api /usr/local/bin/no-llm-api
# the container must listen on all interfaces to be reachable; see security note below
ENV BIND_ADDRESS=0.0.0.0:8080 \
    NO_LLM_SCENARIO=default \
    NO_LLM_LOG=info
EXPOSE 8080
USER nonroot:nonroot
HEALTHCHECK --interval=10s --timeout=2s --start-period=2s --retries=3 \
  CMD ["/usr/local/bin/no-llm-api", "health", "--url", "http://127.0.0.1:8080/ready"]
ENTRYPOINT ["/usr/local/bin/no-llm-api"]
```

Notes that make this work:
- Distroless has no `curl`/`sh`, so `HEALTHCHECK` must be the binary itself. Add a
  `no-llm-api health --url` subcommand (a few lines, reuses no HTTP client if implemented with
  a raw `TcpStream` + one-line request; if `reqwest` is unavailable in the default build that is
  the simplest route).
- `ensure_sample_dataset` (`src/main.rs:24`) writes to the dataset path. Distroless `nonroot`
  cannot write `/app` unless a volume is mounted, so either bake `data/conversations.parquet`
  into the image at build time (`RUN cargo run --release --bin regenerate_dataset`) or point
  `--dataset-path /tmp/conversations.parquet`. Baking it is better: read-only rootfs stays
  possible (`read_only: true` in compose).
- musl/scratch alternative: with default features there is no OpenSSL and no `ring`
  (both arrive only through `async-openai/_api`), so `x86_64-unknown-linux-musl` links cleanly
  and a `FROM scratch` image is realistic. Keep distroless as the default because parquet/arrow
  perform better against glibc's allocator; ship musl only for the standalone binary artifacts.
- Size and startup are claims to measure, not to assert. Budget: final image under 60 MB
  (distroless/cc base is ~25 MB, stripped binary expected in the 15-25 MB range because
  arrow+parquet+tiktoken's embedded BPE tables are large), and time-to-first-accepted-connection
  under 500 ms. `cl100k_base()` builds a 100k-entry BPE map at startup, so measure it; if it
  dominates, make tokenizer loading lazy per preset. Verify with
  `docker image ls`, `docker run ... --print-config`, and `hyperfine`.

### docker-compose for a GUI under test

```yaml
services:
  no-llm-api:
    build: .
    image: ghcr.io/OWNER/no-llm-api:0.2.0
    environment:
      BIND_ADDRESS: 0.0.0.0:8080
      NO_LLM_SCENARIO: default
      NO_LLM_CORS_ORIGINS: "http://localhost:3000"
      NO_LLM_AUTH_MODE: any-bearer      # GUIs insist on a key field; accept anything
      NO_LLM_LOG: info
    ports: ["8080:8080"]                # drop this line if only the GUI needs access
    read_only: true
    tmpfs: ["/tmp"]
    cap_drop: ["ALL"]
    security_opt: ["no-new-privileges:true"]

  gui:
    image: ghcr.io/open-webui/open-webui:main
    depends_on:
      no-llm-api:
        condition: service_healthy
    environment:
      OPENAI_API_BASE_URL: http://no-llm-api:8080/v1
      OPENAI_API_KEY: sk-mock-not-a-secret
      WEBUI_AUTH: "false"
    ports: ["3000:8080"]
```

Real-GUI facts this encodes: Open WebUI points at a custom backend with
`OPENAI_API_BASE_URL` (plural `OPENAI_API_BASE_URLS` for several) plus `OPENAI_API_KEY`, and the
URL must include the `/v1` suffix; LibreChat instead declares a custom endpoint with a
`baseURL` in `librechat.yaml`. Both populate their model picker from `GET /v1/models`, so the
compose file is only useful once that route exists (plan 01/02) - flag the dependency rather
than duplicating the work here. Ship a second compose file wiring the bundled
`index.html` GUI only, for the zero-dependency demo.

### Security posture of the default bind address

`127.0.0.1:8080` (`src/config.rs:51`) is the right host default and the wrong container default:
inside a container it is unreachable from anywhere else, which reads as "the image is broken".
Hence `ENV BIND_ADDRESS=0.0.0.0:8080` in the image only. Because the service today ignores
`Authorization` entirely, a published port means anyone routable can read your fixtures and, once
the control plane lands, rewrite the behaviour of the system under test. Mitigations, all cheap:
emit a WARN at startup when the bind address is non-loopback and `auth-mode=off`; document that
`ports:` should be omitted when only a sibling container needs access; keep
`NO_LLM_CONTROL_PLANE=false` as the recommended setting for any shared host. This is a mock and
does not need real authn, but it should not be silent about being open.

### Prebuilt binaries and crates.io

- GitHub Releases on tag `v*`: matrix over `x86_64-unknown-linux-gnu`,
  `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
  `aarch64-apple-darwin`. Attach stripped archives plus `SHA256SUMS`. `cargo-dist` generates this
  whole workflow, including installer scripts, and is the lower-effort path; a hand-written
  matrix is fine too. Publish the container image to GHCR from the same workflow with tags
  `:X.Y.Z`, `:X.Y`, `:latest`.
- crates.io: yes, worth it, but only after the wire shape settles, because `cargo install
  no-llm-api` is by far the cheapest install story for a test dependency and the `no_llm_api`
  lib target (`Cargo.toml:6-9`) is already the documented reuse surface (`README.md`, "Notes").
  Blockers to clear first: `Cargo.toml:1-4` has no `description`/`license`/`repository`/`readme`/
  `keywords`/`categories`/`rust-version`; the 1.31 MB tracked `openapi.yaml` must be excluded via
  `exclude = [...]` or removed (section 6); the crate name must be checked as available. Keeping
  the lib+bin split in one crate is correct here - splitting into `no-llm-api-core` plus a thin
  bin buys nothing at this size.

---

## 5. Live mode hardening

Current state: `LiveBackend::new` -> `build_client` (`src/live.rs:98-131`) reads
`AZURE_OPENAI_ENDPOINT` first and falls back to OpenAI, so Azure silently wins whenever the
endpoint variable is set - correct but undocumented precedence. Keys go straight into
`AzureConfig`/`OpenAIConfig`, which store them in `secrecy::SecretString`, so async-openai's own
`Debug` will not leak them; our risk is in what *we* write.

| Item | What | Why | Files | Effort |
| --- | --- | --- | --- | --- |
| L1 | Feature-gate the client: `[features] default = []`, `live = ["async-openai/chat-completion", "async-openai/rustls"]`, and make the base dependency `async-openai = { version = "0.41.1", default-features = false, features = ["chat-completion-types"] }`. `#[cfg(feature = "live")] pub mod live;` in `src/lib.rs`. Dev-dependencies keep the full `chat-completion` feature so `tests/async_openai.rs` still drives the server with the real client (dev-dep features do not leak into the normal build). | The default build of an *offline* mock currently links `reqwest`, `rustls`, `hyper-rustls`, `ring` and `secrecy` (Cargo.lock l.962/1696/1738/1780/1885). Dropping them shrinks the binary and the image, cuts CI build time, and removes an entire TLS attack surface from a fixture server. `chat-completion-types` provides the types with no `_api` deps, so nothing else has to change. | `Cargo.toml`, `src/lib.rs`, `src/main.rs:36-45`, `src/bin/recorder.rs` | M |
| L2 | Clear failure mode when the feature is off: `DATASET_SOURCE=live` on a default build must exit non-zero with `live mode requires a build with the "live" feature: cargo install no-llm-api --features live (or use the -live container tag)`. Publish two image tags accordingly. | Silent absence of a mode is the worst failure mode; a one-line message saves an hour. | `src/config.rs`, `src/main.rs` | S |
| L3 | Move all ten credential variables into `LiveSettings` as `Redacted` fields resolved in `config.rs`, delete the `env::var` calls from `live.rs`, and make the precedence explicit and logged (`INFO live backend: azure (endpoint=https://host/, deployment=d)` - host only, never the key). | Makes `Settings` the single description of runtime state, makes precedence testable, and keeps `--print-config` honest. | `src/config.rs`, `src/live.rs:98-131` | M |
| L4 | Redaction pass before any parquet write: build a deny-list from the resolved credential values (api key, org id, project id, deployment name, endpoint host) plus optional `--redact-pattern <regex>` entries, substitute `"[REDACTED]"` in every string field of every row, drop or hash the request `user` field, and never record a non-2xx response. Apply it in one place both `LiveBackend::record_interaction` (`src/live.rs:78-95`) and `src/bin/recorder.rs:66-75` funnel through. | Recorded fixtures are meant to be committed. Prompts sent during recording routinely contain keys, tokens, internal URLs and customer text; once committed, a leak is permanent. `AGENTS.md` already requires "example datasets free of sensitive content" - this makes it mechanical rather than aspirational. | `src/live.rs`, `src/dataset.rs`, `src/bin/recorder.rs` | M |
| L5 | Default `LIVE_RECORD_PATH` to `data/live/` and extend `.gitignore` from the two exact filenames (`.gitignore:17-18`) to `/data/live/`; require an explicit `--promote` step (or manual copy) to move a recording into a tracked fixture, so the redaction pass is always crossed consciously. | Prevents accidental `git add data/` of raw captures. | `.gitignore`, `src/config.rs`, `README.md` | S |
| L6 | Log-hygiene test: capture `tracing` output while running a live-mode startup with a fake key and assert the key string never appears; same assertion over a recorded parquet round-trip. | Turns "we are careful" into CI. | `src/live.rs` tests | S |
| L7 | Bound the blast radius: explicit request timeout on the live client, retries disabled or capped (async-openai uses `backoff` internally), a concurrency cap, and a WARN-level running count of upstream calls made in this process. | Live mode spends real money; an accidental load test against a GUI dev loop is the obvious way to lose a hundred dollars. | `src/live.rs` | S |

---

## 6. Repo hygiene backlog

### Documentation set

Status: the `GEMINI.md`, `task.md`, `chat_completions_scope.md` and `AGENTS.md` rows below all
landed in the cleanup commit. The README rows are still open.

| File | Today | Decision |
| --- | --- | --- |
| `README.md` | user-facing, accurate, has the env table | **Canonical user doc.** Add CLI/`--help` output, scenarios, Docker, health routes. Keep the env table generated-checked by the test proposed in section 1. |
| `AGENTS.md` | contributor + agent conventions | **DONE.** Canonical contributor doc. The stale "tiktoken-rs already pulls in async-openai" and "there is no standalone `tests/` directory" claims were corrected, and the spec-refresh convention added. Feature matrix and doc-ownership table still open. |
| `GEMINI.md` | duplicated README/AGENTS and was wrong in two places: it said `store` "handles the data storage and retrieval from Parquet files" (it is the in-memory `CompletionStore`) and that `openapi.yaml` is this project's API definition (it is the upstream OpenAI spec copy) | **DONE - deleted.** Two agent-instruction files with divergent architecture descriptions is a net negative. |
| `task.md` | a work order whose deliverables had all shipped (rich dataset, recorder, live mode, e2e tests) | **DONE - deleted.** It read as pending work; git history retains it. |
| `chat_completions_scope.md` | the best artefact in the repo: a distilled wire contract | **DONE - moved to `docs/spec/chat-completions-scope.md`.** Still wanted: a header line recording which upstream spec revision it was distilled from. |
| `docs/plans/*` | plans 01-05 plus the `00-roadmap.md` synthesis | **DONE - `docs/README.md` is the index.** |

### The OpenAPI copies

Status: `openapi.yaml` is now the current upstream spec (2,827,615 bytes, OpenAPI 3.1.0, upstream
commit `5c044be3bf3a`), fetched and provenance-recorded by `scripts/fetch-openapi.ps1` / `.sh`;
`openapi.documented.yml` and the third copy inside the vendored clone are both gone. What remains
open is the size of the tracked copy:

1. Generate `docs/spec/chat-completions.openapi.yaml`: the chat-completion paths plus their
   transitively referenced schemas only, expected well under 150 KB. Track that. It is the part
   we actually assert against, it diffs readably in review, and it keeps offline determinism.
2. **DONE.** `scripts/fetch-openapi.ps1` / `.sh` download the spec, record URL, upstream commit,
   date and `sha256` in `openapi.provenance.json`, and offer a `-Check` mode that exits non-zero
   when the local copy is behind upstream. Process documented in `docs/spec/upstream-openapi.md`.
   Still open: untracking the full copy once (1) exists.
3. **DONE.** `AGENTS.md` records that `openapi.yaml` is upstream reference material, is a grep
   target only, and must never be hand-edited.

History rewrite to purge the old 1.3 MB blob is not worth it for a repo this young; replacing it in
HEAD is enough to keep clones and the crates.io tarball small.

### Vendored upstream clones

**DONE - both deleted.** `async-openai/` (48.6 MB, pinned at 0.30.1 while `Cargo.toml` depends on
0.41.1) and `openai-func-enums/` (1.1 MB, referenced by no code path) were removed. Reading a clone
that drifts from the lockfile to learn what the client expects is actively misleading, which is
exactly the kind of drift a reference copy is supposed to prevent. `AGENTS.md` now points at
`~/.cargo/registry/src/*/async-openai-0.41.1/`, which is guaranteed to match `Cargo.lock`.

Superseded detail from the original recommendation, kept for the reasoning: a pinned shallow clone
(`git clone --depth 1 --branch v0.41.1`) would also have worked, but the registry path costs nothing
to keep in sync. `AGENTS.md` records that the registry path is the source of truth so the next agent
does not re-vendor.

---

## 7. Versioning and release

The public contract of a mock is **the bytes on the wire plus the switches that select them**,
not the Rust API. Policy, to be written into `docs/versioning.md` and linked from README:

| Change | Bump |
| --- | --- |
| New route, new optional response field, new scenario, new env/flag with a back-compatible default | MINOR |
| Bug fix that brings a response closer to the OpenAI spec without changing valid-client behaviour; docs; internal refactor | PATCH |
| Removing/renaming a response field, changing a default (port, tokens/sec, scenario), changing SSE frame ordering or the `[DONE]` terminator, changing status codes or the error envelope, removing/renaming an env var or flag, changing the parquet fixture schema in a way old files cannot be read, dropping a tokenizer preset, changing `system_fingerprint` format | MAJOR |
| Changing the *content* of the bundled sample dataset | MINOR - it is observable output and will break someone's snapshot test |
| Rust API changes to the `no_llm_api` lib target | MINOR pre-1.0, MAJOR after; state explicitly that the lib surface is semver-relevant only from 1.0 |

Pre-1.0 caveat: while at `0.x` (`Cargo.toml:3` is `0.1.0`), MINOR carries breaking changes per
Cargo convention. Say so in the README rather than pretending otherwise, and reach 1.0 once the
wire shape stops moving.

Mechanics:
- `CHANGELOG.md` in keep-a-changelog form with a `Wire behaviour` subsection per release, because
  that is the only section a consumer of a mock must read. Add a CI check that any PR touching
  `src/model.rs`, `src/http/`, `src/service.rs` or `src/dataset.rs` also touches the changelog.
- Expose the version where tests can pin it: `/health` body and a `x-no-llm-api-version` response
  header. `system_fingerprint` is *not* the place for it:
  `docs/plans/02-simulation-engine.md:100` already defines it as `fp_mock_{plan_digest:08x}`, a
  digest of the simulation plan, which is the more useful signal. Defer to plan 02 there and keep
  version pinning in the header and `/health`, so a GUI test can assert which mock build it is
  talking to without colliding with response-shape determinism.
- Release checklist: bump `Cargo.toml`, update `CHANGELOG.md`, tag `vX.Y.Z`, CI builds binaries +
  GHCR image, optional `cargo publish`. `cargo-dist` or `release-plz` can automate the first four.

---

## Prioritized checklist

Ranked by value per hour for cheap, deterministic GUI testing.

| # | What | Why it matters for mock-GUI testing | Files | Effort | Acceptance |
| --- | --- | --- | --- | --- | --- |
| 1 | `clap` server CLI + typed `Settings::resolve` + `--print-config`; add `clap` feature `env`; enum tokenizer preset; strict bools; multi-error report | Testers discover and set behaviour without reading source; misconfiguration fails at startup with a fix, not mid-suite | `src/config.rs`, `src/main.rs`, `src/tokenizer.rs`, `Cargo.toml` | M | `cargo run -- --help` lists every setting; `cargo run -- --print-config` emits JSON matching defaults; unit tests in `config.rs` cover each error message; `TOKENIZER_MODEL=gpt2 cargo run` exits non-zero listing the five presets |
| 2 | `GET /health` + `GET /ready`, exempt from auth and failure injection, version in body | Every container orchestrator and CI wait-loop needs one; `docker compose depends_on: service_healthy` is otherwise impossible | `src/http/routes.rs`, `src/main.rs` | S | `curl -sf localhost:8080/health` returns 200 with `version`; `/ready` returns script count; test asserts `/health` still 200 under `--scenario flaky --fail-rate 1.0` |
| 3 | Scenario bundle: `Scenario` struct, 7 built-ins via `include_str!`, `--scenario`/`NO_LLM_SCENARIO`, seeded RNG | One switch to reach slow/flaky/auth-failure states; seeded means a flaky test is reproducible | `src/config.rs`, `src/scenario.rs` (new), `scenarios/*.toml`, `src/service.rs` | M | `--scenario slow` measurably changes SSE pacing in a test; two runs with the same seed produce identical failure positions; `--scenario nope` errors listing valid names |
| 4 | Graceful shutdown on SIGINT/SIGTERM with `--shutdown-grace-ms`, draining active SSE | `docker stop` currently waits 10 s for SIGKILL and truncates streams mid-test, which looks like a GUI bug | `src/main.rs:47` | S | test sends shutdown while a stream is open and asserts the stream terminates with `[DONE]`; `docker stop` returns in under 3 s |
| 5 | Feature-gate live mode (`default = []`, `live = [...]`, `chat-completion-types` base) + clear "built without live" error | Removes `reqwest`/`rustls`/`ring` from an offline fixture: smaller image, faster CI, no TLS surface | `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/bin/recorder.rs` | M | `cargo tree --no-default-features \| Select-String reqwest` finds nothing; `cargo test --no-default-features` passes; `DATASET_SOURCE=live` on the default build exits non-zero with the feature hint |
| 6 | `tower-http`: request id in/out, `TraceLayer` with structured fields, DEBUG access log, targets re-enabled; request id echoed in the response body | Turns "the GUI showed an error" into a grep-able span; header/secret exclusion prevents key leakage into logs | `Cargo.toml`, `src/http/routes.rs`, `src/main.rs:38-46` | M | response carries `x-request-id`, inbound value is echoed; `NO_LLM_LOG=debug` shows one line per request with the id; test asserts a fake bearer never appears in captured logs |
| 7 | Dockerfile as given + embed `index.html` + bake the sample dataset + `health` subcommand; GHCR publish | A one-command mock backend is the whole value proposition for a GUI team that does not build Rust | `Dockerfile`, `.dockerignore`, `src/http/routes.rs:57-62`, `src/main.rs` | M | `docker build .` succeeds; `docker run -p 8080:8080` answers `/ready` and `GET /` with `read_only: true`; `docker image ls` under 60 MB; container runs as non-root (`docker inspect` user `nonroot`) |
| 8 | `docker-compose.yml` (Open WebUI) + `docker-compose.demo.yml` (bundled `index.html`) + README walkthrough | Proves the mock against a real third-party GUI, which is the actual acceptance test of API shape | `docker-compose*.yml`, `README.md` | S | `docker compose up` then the GUI lists models from `/v1/models` and streams a reply (depends on plan 01/02 for `/v1/models`) |
| 9 | CI workflow: fmt, `clippy -D warnings`, `cargo test`, `cargo test --no-default-features`, `ASYNC_OPENAI_COMPAT=1 cargo test`, `docker build`, `rust-toolchain.toml` pinning 1.90 | `AGENTS.md` already treats clippy warnings as merge blockers; nothing enforces it today | `.github/workflows/ci.yml`, `rust-toolchain.toml` | S | workflow green on a PR; a deliberate `clippy` warning fails the run |
| 10 | Live-mode redaction pass + `data/live/` default + log-hygiene tests + credential move into `Settings` | Recorded fixtures are committed; a leaked key or customer prompt is permanent | `src/live.rs`, `src/dataset.rs`, `src/bin/recorder.rs`, `src/config.rs`, `.gitignore` | M | unit test records an interaction containing a fake key and asserts `[REDACTED]` in the parquet round-trip and absence of the key in logs |
| 11 | Doc reconciliation: ~~delete `GEMINI.md`~~, ~~delete `task.md`~~, ~~promote `chat_completions_scope.md` to `docs/spec/`~~, ~~fix the two stale `AGENTS.md` claims~~, README env/CLI table + drift test (only the README table remains) | Contradictory instruction files actively mislead both humans and agents | `README.md`, `docs/` | S | repo has one contributor doc and one user doc; test fails when a flag is missing from the README table |
| 12 | OpenAPI slimming: track a pruned chat-completions extract, untrack `openapi.yaml`, add `scripts/fetch-openapi.*` with a pinned sha256; `scripts/vendor-refs.*`, drop `openai-func-enums/` | 1.3 MB of unreviewable YAML in every clone and in any crates.io tarball; the vendored clone is already version-skewed (0.30.1 vs 0.41.1) | `.gitignore`, `docs/spec/`, `scripts/`, `Cargo.toml` (`exclude`) | S | `git ls-files \| ForEach-Object { (Get-Item $_).Length } \| Measure-Object -Max` shows no tracked file over ~200 KB except the parquet fixture; fetch script reproduces the pinned hash |
| 13 | Control-plane composition: `ArcSwap<Scenario>` + `/_mock/scenario` GET/PUT/PATCH + `/_mock/reset`, gated by `NO_LLM_CONTROL_PLANE` | Lets one long-lived server serve a whole test suite, including error paths, with no restarts | `src/http/routes.rs`, `src/scenario.rs`, `src/config.rs` | M | test switches to `flaky`, asserts a 500, resets, asserts 200; routes absent (404) when the switch is off |
| 14 | `--metrics` with six hand-rolled Prometheus counters | Cheap visibility into stream aborts and injected failures during a long GUI suite | `src/http/routes.rs`, `src/service.rs` | S | `/metrics` returns valid text format with `sse_streams_active`; absent when the flag is off |
| 15 | Release plumbing: `CHANGELOG.md`, `docs/versioning.md`, `x-no-llm-api-version` header, `fp_nollm_X_Y_Z` fingerprint, release workflow (binaries + GHCR), crates.io metadata | Consumers of a mock need to pin wire behaviour; without a changelog every upgrade is a gamble | `CHANGELOG.md`, `docs/versioning.md`, `.github/workflows/release.yml`, `Cargo.toml`, `src/service.rs` | M | tagging `v0.2.0` produces five binary artifacts plus an image; `cargo publish --dry-run` succeeds; response `system_fingerprint` equals `fp_nollm_0_2_0` |

Non-goals, stated so they do not creep in: OpenTelemetry/Jaeger, latency histograms, a metrics
dashboard, Kubernetes manifests or a Helm chart, multi-replica or clustered state, real
authentication, TLS termination in-process, and git history rewriting.
