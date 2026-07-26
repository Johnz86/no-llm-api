# 04 - Testing, Verification and CI

Scope: short-term (days) work to make `no-llm-api` trustworthy as a deterministic mock backend for chat GUIs.
All claims below are grounded in the current tree; file:line refs are from this session.

## 0. Test layout convention (reconcile AGENTS.md vs tests/)

`AGENTS.md` states "there is no standalone `tests/` directory", but `tests/async_openai.rs` exists (only file
under `tests/`, confirmed by glob). Both are load-bearing for different reasons.

Decision - keep both, with a hard rule:

| Layer | Location | Rule |
| --- | --- | --- |
| Pure logic on private items | inline `#[cfg(test)] mod tests` in the module | Only when the test needs private access (dataset row parsing, store internals, tokenizer). |
| Anything reached through `pub` API, the router, or the wire | `tests/*.rs` | HTTP, SSE, client-contract, snapshots, fixtures. |

Reason: HTTP/SSE tests need `build_router` plus a server or `tower::oneshot`, which is exactly the public
surface already exported by `src/lib.rs:1-8`. Keeping them out of the modules keeps the lib test target small
and stops wire-level fixtures from competing with unit tests for review attention.

Blocking side issue found while counting tests: `src/main.rs:1-8` re-declares `mod config; mod dataset; ... mod
store;` instead of using the library (`src/lib.rs:1-8` already exports all eight modules). Every module - and
every inline test - is therefore compiled twice, once into `lib` and once into the `no-llm-api` bin target. That
is why 7 authored inline tests report as 14 (+1 integration = 15). Fix: make `main.rs` a thin `use
no_llm_api::{...}` shim. This halves test-binary count and removes duplicate clippy findings.

Action item: update `AGENTS.md` "Project Structure" and "Testing Guidelines" sections to the table above.

## 1. Coverage audit (honest)

Authored tests: 8 functions total.

| Test | File:line | What it actually asserts |
| --- | --- | --- |
| `loads_sample_dataset` | `src/dataset.rs:1006` | Sample parquet loads; `conv-lyra` turn 0 has `finish_reason=tool_calls` + non-empty `tool_calls`; turn 1 has `function_call`; `conv-vega` turn 1 refusal text + `content_filter`; `conv-audio` turn 0 is `MessageContent::Parts` with `audio.id="aud_123"` and `prompt_tokens_details` present; `conv-builder` turn 0 `function_call`. |
| `list_respects_order_and_pagination` | `src/store.rs:267` | Store-level asc/desc order, `after` cursor, single metadata equality filter. |
| `messages_follow_ordering` | `src/store.rs:319` | Message asc/desc and `after`. |
| `update_metadata_overwrites` | `src/store.rs:354` | `update_metadata` writes then reads back. |
| `stores_completion_when_requested` | `src/service.rs:489` | `store: None` does not persist; `store: Some(true)` persists 1 record and `get` round-trips. |
| `matches_user_prompt_to_dataset` | `src/service.rs:517` | Exact prompt match returns the scripted sprint text. |
| `applies_max_completion_token_cap` | `src/service.rs:538` | `max_completion_tokens=5` caps tokens, `usage.completion_tokens == tokens.len()`, `finish_reason=length`. |
| `async_openai_compatibility` | `tests/async_openai.rs:23` | With `ASYNC_OPENAI_COMPAT` set: non-streamed body deserialises into `CreateChatCompletionResponse` and contains "Sprint closed"; a streamed run eventually yields a chunk with `usage.is_some()`; a tool prompt yields `finish_reason=tool_calls`. |

### Behaviour surface vs coverage

| Behaviour | Where implemented | Covered? | Gap |
| --- | --- | --- | --- |
| SSE frame ordering | `src/http/routes.rs:124-208` | No | No test reads frames in order at all. |
| SSE pacing (`TOKENS_PER_SECOND`) | `src/http/routes.rs:125,159-162` | No | Rate is the product's core feature and is completely unverified. |
| Delta concatenation == final content | `src/http/routes.rs:127-158` | No | The `&decoded[rendered.len()..]` slice at `:139` is a byte-index slice on a UTF-8 string; multi-byte token boundaries are only safe because `decoded` is a prefix-extension. Untested and panic-capable. |
| `[DONE]` terminator | `src/http/routes.rs:207` | Indirectly (client loop ends) | Never asserted as the last frame. |
| Usage chunk on `include_usage` | `src/http/routes.rs:201-206` | Weakly (`usage.is_some()` on any chunk) | Position (after final chunk, before `[DONE]`) and `choices == []` unasserted. |
| Final chunk carries tool_calls/refusal/audio | `src/http/routes.rs:165-200` | No | Streamed tool-call path untested; only the non-streamed one is. |
| HTTP pagination `has_more`/`first_id`/`last_id` | `src/http/routes.rs:69-103, 339-376` | No | `limit+1` fetch-then-truncate logic exists only in the route; store tests cannot reach it. |
| `metadata[key]=value` query parsing | `src/http/routes.rs:297-337` | No | Hand-written `Deserialize` with `#[serde(flatten)]` + `deserialize_any`; highest-complexity untested code in the repo, and it degrades silently to "no filter". |
| `order` validation -> 400 | `src/http/routes.rs:68-71,344-347` | No | |
| 404 envelope | `src/http/routes.rs:389-397` | No | |
| Error envelope shape | `src/http/routes.rs:379-387` | No | Emits only `message` + `type`. Spec `Error` requires `type,message,param,code` (`docs/spec/chat-completions.openapi.yaml:3268-3287`); `async-openai`'s `ApiError` (`AO/error.rs:79-84`) tolerates missing `param`/`code`, other clients may not. |
| Malformed JSON body | axum default | No | Returns axum's plain-text 422, not the OpenAI envelope. |
| `GET /` index | `src/http/routes.rs:56-61` | No | Reads relative `index.html`, so behaviour depends on process cwd. |
| Dataset round-trip (write -> read) | `src/dataset.rs:395-460`, `rows_from_interaction:616-664` | No | Only the bundled sample is read; nothing writes then reads back. |
| Rotation fallback on prompt miss | `src/service.rs:43-49` | No | `AtomicUsize` round-robin: identical concurrent requests get different answers. Directly hostile to GUI test determinism. |
| Store lifecycle: delete then get/messages | `src/store.rs:91-98` | No | `delete` never tested; `ChatCompletionDeleted` shape never asserted. |
| `config::Settings::load` | `src/config.rs:49-112` | No | Zero tests for env parsing, including `TOKENS_PER_SECOND=0` -> error and unknown `DATASET_SOURCE`. |
| `tokenizer::load` presets | `src/tokenizer.rs` | No | The five documented `TOKENIZER_MODEL` values are unverified. |
| Live backend | `src/live.rs` | No | Requires network; must stay out of CI (see section 6). |

### Highest-risk untested behaviour, ranked

1. `src/http/routes.rs` has no test module at all (grep for `mod tests` matches only `dataset.rs:1000`,
   `service.rs:463`, `store.rs:189`). The entire wire contract - status codes, pagination envelopes, SSE - is
   unverified. This is the surface a GUI actually consumes.
2. SSE pacing and ordering (`routes.rs:124-208`). The selling point ("tokens per second") has no assertion.
3. `tests/async_openai.rs:25-32` returns `Ok(())` when `ASYNC_OPENAI_COMPAT` is unset - a permanently green
   test that asserts nothing. Anyone reading `cargo test` output today believes client compatibility is
   covered; it is not, unless the env var happens to be set.
4. Non-deterministic fallback (`service.rs:43-49`) plus non-deterministic ids/timestamps
   (`service.rs:188-190`) make snapshot testing and reproducible GUI runs impossible today.
5. `MetadataFilters` custom deserializer (`routes.rs:297-337`) - subtle, and silently degrades to "no filter"
   on shape mismatch.
6. `ensure_sample_dataset` returns early when the file exists (`src/dataset.rs:545-547`), so
   `cargo run --bin regenerate_dataset` (`src/bin/regenerate_dataset.rs:5`) is a no-op against the committed
   `data/conversations.parquet`. README claims it "recreates" the file. Committed fixtures can silently drift
   from `write_dataset`'s schema (`dataset.rs:402-413`).

## 2. Test pyramid

| Tier | Tool | Files | Runs in default `cargo test`? |
| --- | --- | --- | --- |
| Unit (private logic) | plain `#[test]` / `#[tokio::test]` inline | `src/{dataset,store,service,config,tokenizer}.rs` | Yes |
| HTTP contract | `tower::ServiceExt::oneshot` on `build_router` | `tests/http_api.rs` | Yes |
| SSE behaviour | same router + SSE harness (section 3) | `tests/sse.rs`, `tests/support/sse.rs` | Yes |
| Client contract | real `async-openai` client over loopback `TcpListener` | `tests/async_openai.rs` | Yes - make default-on |
| Snapshot / golden | `insta` over HTTP bodies and SSE transcripts | `tests/snapshots.rs` | Yes |
| Browser E2E | Playwright | `e2e/` (Node project, not a cargo target) | No - separate command and CI job |

HTTP tier detail: `tower 0.5.3` is already both a dependency and a dev-dependency (`Cargo.toml:31,51`), so
`oneshot` needs no new crate. Use `oneshot` for everything non-streaming (fast, no sockets, no port binding -
matters on Windows runners). For SSE, `oneshot` still works: `response.into_body()` is a
`http_body_util::BodyExt` stream; collect frames as they arrive so pacing is observable. If frame-level timing
through `oneshot` proves awkward, fall back to a real `TcpListener` bound to `127.0.0.1:0` as
`tests/async_openai.rs:48-58` already does.

### Should `ASYNC_OPENAI_COMPAT` stay opt-in?

Make it default-on. Evidence:

- The test never touches the internet: it binds `127.0.0.1:0` (`tests/async_openai.rs:48`) and points
  `OpenAIConfig::with_api_base` at that loopback address (`tests/async_openai.rs:110-114`).
- `async-openai` is a normal (non-optional) dependency (`Cargo.toml:44`) and defaults to `rustls`
  (`async-openai-0.41.1/Cargo.toml`, `default = ["rustls"]`), so no OpenSSL/system TLS is needed on
  Linux or Windows runners.
- The current skip-on-missing-env pattern produces false green output, which is worse than no test.

Keep exactly one opt-in gate, for live network paths only: `NO_LLM_API_LIVE=1` guarding tests that touch
`src/live.rs`. Everything else runs unconditionally. Delete the `ENV_FLAG` block at
`tests/async_openai.rs:21` and `:25-32`.

Caveat to encode in the test: `async-openai`'s client retries with backoff on server errors
(`AO/client.rs:782-830`). Do not use it to assert 4xx/5xx behaviour - use `oneshot`
or raw `reqwest` for error-path tests so failures are fast and deterministic.

### Browser E2E tool choice

Recommend Playwright (`@playwright/test`) in an `e2e/` directory.

| Option | Verdict |
| --- | --- |
| Playwright | Chosen. First-class Windows support (`npx playwright install --with-deps`), auto-waiting locators that suit token-by-token streaming text, `webServer` config can launch `cargo run` and wait for readiness, trace viewer for CI debugging, and it can assert on the SSE response body via `page.route`/`request` events. |
| `thirtyfour`/`fantoccini` (Rust WebDriver) | Rejected for now: keeps the repo Node-free, but requires matching chromedriver/geckodriver binaries per runner, has no auto-wait, and streaming-text assertions become manual polling loops. |
| `wasm-bindgen-test` / headless_chrome | Rejected: not a fit for testing a plain `index.html` served by axum. |

The GUI to drive is `index.html`, which POSTs `/v1/chat/completions` with `stream: true` and reads the body
with a `TextDecoder` loop (`index.html:126-170`), rendering into `#chat-messages` with `#message-input` and
`#send-button` (`index.html:94-98`). Those ids are stable selectors; assert that (a) partial text grows over
time, (b) final text equals the scripted dataset answer, (c) no console errors (the code logs
`Error parsing stream data` at `index.html:164` - a good failure signal to assert absent).

E2E must never be part of `cargo test`. One command: `npm --prefix e2e test`.

## 3. SSE assertion harness

New file `tests/support/sse.rs` (shared via `mod support;` in each integration test).

```rust
pub struct Frame {
    pub index: usize,
    pub at: Duration,          // since first byte of the response body
    pub raw: String,           // data payload, comments/keep-alive stripped
    pub chunk: Option<ChatCompletionChunk>, // None for the [DONE] sentinel
}

pub struct Transcript {
    pub frames: Vec<Frame>,
    pub started: Instant,
}

impl Transcript {
    /// Drive the router directly; no socket, no port.
    pub async fn collect(router: Router, request: Request<Body>) -> anyhow::Result<Self>;

    /// Same, but against an already-running loopback server.
    pub async fn collect_from(url: &str, body: serde_json::Value) -> anyhow::Result<Self>;

    // --- assertions ---
    pub fn assert_terminated(&self);                  // last frame raw == "[DONE]"
    pub fn assert_single_done(&self);                 // exactly one [DONE]
    pub fn content(&self) -> String;                  // concat of choices[0].delta.content
    pub fn assert_content_eq(&self, expected: &str);  // deltas == final message text
    pub fn assert_role_only_first(&self);             // delta.role present only on frame 0
    pub fn finish_reasons(&self) -> Vec<FinishReason>;
    pub fn assert_finish_on_last_choice_frame(&self); // finish_reason only on final choice frame
    pub fn usage_frame(&self) -> Option<&Frame>;      // must sit immediately before [DONE]
    pub fn assert_ids_stable(&self);                  // one id/created across all frames
    pub fn assert_rate(&self, tokens_per_second: u32, tolerance: f64);
    pub fn transcript_text(&self) -> String;          // normalised, for insta snapshots
}
```

Implementation notes:

- Strip SSE comment lines. The router enables `KeepAlive::new().interval(15s).text("ping")`
  (`src/http/routes.rs:210-215`), which emits `:ping` comments; a naive line splitter would treat them as
  frames.
- `assert_rate` must measure content frames only (the final metadata chunk and usage chunk are emitted with no
  sleep, `routes.rs:193-206`), and must compare *median inter-frame gap*, not total elapsed, so a slow CI
  runner's scheduling jitter does not fail the build. Suggested defaults: `tolerance = 0.5` (i.e. observed rate
  within 50% of nominal) at `TOKENS_PER_SECOND=50`, plus a strict lower bound test at
  `TOKENS_PER_SECOND=2` asserting total elapsed >= (n-1)/2 seconds. Timing assertions belong in one file so
  they can be marked `#[ignore]` if a runner turns out pathological.
- `assert_content_eq` closes the byte-slicing risk at `routes.rs:139`: add a fixture whose answer contains
  multi-byte characters (emoji, CJK, accented Latin) so token-boundary slicing is exercised.

## 4. Determinism and snapshot testing

Crate: `insta` (dev-dependency, `insta = { version = "1", features = ["json", "redactions"] }`), reviewed with
`cargo insta review`; `INSTA_UPDATE=no` in CI so unexpected diffs fail rather than auto-accept.

Two snapshot families:

1. `assert_json_snapshot!` on full non-streamed response bodies (one per scenario: text, tool_calls,
   function_call, refusal/content_filter, audio/parts, length-truncated).
2. `assert_snapshot!` on `Transcript::transcript_text()` - a normalised SSE transcript, one frame per line, so
   ordering and frame boundaries are visible in the diff.

Sources of churn today and the fix:

| Volatile field | Origin | Fix |
| --- | --- | --- |
| `id` (`chatcmpl-<uuid>`) | `src/service.rs:189` | Inject an id source. Add a test-only constructor (e.g. `ChatService::with_seed(u64)`) that produces `chatcmpl-test-0001`, `-0002`, ... from a counter. This is also the deterministic-seed feature the product needs, so it is not test-only scaffolding. |
| `request_id` (`req_<uuid>`) | `src/service.rs:190` | Same id source. |
| `created` | `unix_timestamp()`, `src/service.rs:188` and `:385` | Injectable clock; fixed `1_700_000_000` in tests. Note `async-openai` types `created` as `u32` (`AO/types/chat/chat_.rs:1086`), so keep the value positive and below 2^32. |
| Rotation fallback answer | `src/service.rs:43-49` | Snapshot only prompts that match a script until seeded selection lands; then snapshot the seeded fallback too. |
| Frame timing | harness | Never snapshot timestamps; `transcript_text()` omits them. Timing is asserted numerically, not by snapshot. |

Until the injectable id/clock exists, use insta redactions (`".id" => "[id]"`, `".created" => "[ts]"`,
`".request_id" => "[req]"`) so snapshots can land in the same day's work. Prefer replacing redactions with real
injection afterwards - redactions hide exactly the fields a GUI uses for message keys.

Also snapshot-worthy: the error envelope for 404, invalid `order`, and malformed JSON. These lock in the
`type,message,param,code` shape from `docs/spec/chat-completions.openapi.yaml:3268-3287`.

## 5. Fixture strategy

- Test datasets are always temp datasets. Existing pattern (`tempdir()` + `ensure_sample_dataset`, e.g.
  `src/service.rs:489-495`) is right; hoist it into `tests/support/fixtures.rs` as
  `fn dataset(rows: &[DatasetRow]) -> (TempDir, PathBuf)` plus `fn sample() -> (TempDir, PathBuf)` so no test
  reads or writes `data/`.
- Purpose-built fixtures per behaviour, built with `dataset::write_dataset` (`src/dataset.rs:395`): a
  multi-byte-content script, a long script for pacing, a two-turn script for message pagination, an
  empty-content + refusal-only script.
- `regenerate_dataset`'s role: it is the single source of truth for the committed
  `data/conversations.parquet`. It must actually regenerate - today it cannot, because
  `ensure_sample_dataset` short-circuits on an existing path (`src/dataset.rs:545-547`) and the binary calls
  only that (`src/bin/regenerate_dataset.rs:5`). Add `--force` (or have the binary call `write_dataset`
  directly) and fix the README claim.
- Fixture drift guard, as a normal test in `tests/fixtures.rs`:
  1. Read `data/conversations.parquet` and assert its arrow schema field names, types and nullability equal the
     11-field schema in `write_dataset` (`src/dataset.rs:402-413`).
  2. Regenerate the sample into a temp path via `write_dataset(path, &sample_rows())` and assert row count,
     `conversation_id` set, and per-row content equal the committed file. A schema or sample change then fails
     CI with "run `cargo run --bin regenerate_dataset -- --force`".
  This requires `sample_rows()` (`src/dataset.rs:669`) to be `pub` or reachable via a `pub fn sample_rows()`
  wrapper.

## 6. GitHub Actions workflow

`/.github/workflows/ci.yml` (no `.github` directory exists yet):

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: "0"
  RUST_BACKTRACE: "1"
  # Never set OPENAI_* / AZURE_OPENAI_* / DATASET_SOURCE here.
  NO_LLM_API_LIVE: ""
  INSTA_UPDATE: "no"

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --all --check

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --all-targets --all-features --locked -- -D warnings

  test:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --locked --all-targets
      - run: cargo test --locked --all-targets
      - run: cargo test --locked --doc

  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2
        with:
          command: check advisories bans licenses sources

  fresh-clone:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Reject path dependencies
        run: |
          if grep -nE '^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=.*\bpath[[:space:]]*=' Cargo.toml; then
            echo "path dependency found in Cargo.toml"; exit 1
          fi
          if grep -n 'source = "path' Cargo.lock; then
            echo "path dependency found in Cargo.lock"; exit 1
          fi
      - name: Build from a pristine clone
        run: |
          git clone --depth 1 "file://$PWD" /tmp/fresh
          cd /tmp/fresh
          test ! -d async-openai && test ! -d openai-func-enums
          cargo build --locked --all-targets
          cargo test --locked --all-targets
```

Notes:

- The `fresh-clone` job is the guard against re-vendoring reference clones. Both were deleted and
  `.gitignore` still excludes `/async-openai`, so the `test ! -d` assertions make an accidental commit
  of those trees a hard failure.
- `--locked` everywhere: dependencies were just bumped, so a silent lockfile update in CI would hide breakage.
- `cargo-deny` over `cargo-audit`: it covers advisories plus license/source checks, and `sources` catches a
  dependency being repointed at a git or path source. Add a minimal `deny.toml` in the same change; start with
  `[advisories] yanked = "warn"` and no allow-list exceptions.
- `async-openai` defaults to rustls (`async-openai-0.41.1/Cargo.toml`, `default = ["rustls"]`), so no
  `apt-get install libssl-dev` step is needed.
- Must NOT run in CI: anything with `DATASET_SOURCE=live` (`src/config.rs:60-79`) or the recorder binary's
  replay path, both of which call out to OpenAI/Azure using `OPENAI_*`/`AZURE_OPENAI_*`
  (`src/live.rs:99-105`). Gate: those tests are `#[ignore]`-annotated *and* early-return unless
  `NO_LLM_API_LIVE=1`; CI explicitly blanks that variable and never defines credential secrets. Optionally add
  a tiny CI step asserting `env | grep -c OPENAI` is zero to catch a future secret leak into the workflow.
- Windows specifics that must hold: tests bind `127.0.0.1:0` (fine on hosted Windows runners) and must not
  depend on the process cwd - which `serve_index` (`src/http/routes.rs:56-61`) currently does, so its test
  should assert either a 200 or the explicit 500 path rather than hardcoding cwd.
- Optional follow-up job (not day-one): `npm --prefix e2e ci && npx playwright test`, ubuntu-only, with
  `continue-on-error: false` once stable.

## 7. Local pre-push block

```bash
cargo fmt --all
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --locked --all-targets
cargo test --locked --all-targets
cargo test --locked --doc
cargo deny check advisories bans licenses sources
```

PowerShell one-liner equivalent (stops on first failure):

```powershell
cargo fmt --all; if ($?) { cargo clippy --all-targets --all-features --locked -- -D warnings }; if ($?) { cargo test --locked --all-targets }; if ($?) { cargo test --locked --doc }
```

Only when touching fixtures or the parquet schema:

```bash
cargo run --bin regenerate_dataset -- --force
cargo test --locked --test fixtures
```

Only when touching the GUI:

```bash
npm --prefix e2e test
```

Never run locally as part of the normal loop (costs money, hits the network):

```bash
# DATASET_SOURCE=live NO_LLM_API_LIVE=1 cargo test --locked -- --ignored
```

## 8. Prioritized checklist

| # | What | Why it matters for mock-GUI testing | Files | Effort | Acceptance |
| --- | --- | --- | --- | --- | --- |
| 1 | Add `tests/support/sse.rs` harness (section 3) and `tests/sse.rs` covering ordering, delta==final content, single `[DONE]` last, usage frame position, id/created stable across frames | Streaming is what GUIs actually render; today zero assertions exist | new `tests/support/sse.rs`, `tests/sse.rs` | M | `cargo test --test sse` passes; deliberately reordering the final/usage/`[DONE]` sends in `routes.rs` makes it fail |
| 2 | Make async-openai contract test default-on; delete the `ASYNC_OPENAI_COMPAT` early return; add streamed tool-call and refusal scenarios | Removes a false-green test and locks the real client contract with no network | `tests/async_openai.rs` | S | `cargo test` (no env vars) runs the test; log line "skipping async-openai" no longer appears |
| 3 | Add `tests/http_api.rs` using `tower::ServiceExt::oneshot`: 404 envelope, invalid `order` -> 400, malformed JSON, pagination `has_more`/`first_id`/`last_id` on both list endpoints, `metadata[key]=value` filter, delete-then-get, `/v1` and root parity | The whole wire contract is currently untested; `/v1` vs root parity is exactly what breaks GUI base-URL config | new `tests/http_api.rs` | M | Test asserts every implemented route in `routes.rs:31-54` at least once; passes on both OSes |
| 4 | Fix `main.rs` to consume the `no_llm_api` lib instead of re-declaring modules | Halves test/clippy runtime and stops duplicate inline-test execution (14 vs 7) | `src/main.rs` | S | `cargo test` reports 7 lib unit tests, not 14 |
| 5 | Land `.github/workflows/ci.yml` + `deny.toml` | Nothing is enforced today; fmt/clippy drift and Windows-only breakage go unnoticed | new `.github/workflows/ci.yml`, `deny.toml` | S | Green run on ubuntu-latest and windows-latest; `fresh-clone` job fails if a `path =` dep or vendored clone is committed |
| 6 | Fixture drift guard + `regenerate_dataset --force` | Committed `data/conversations.parquet` can silently diverge from `write_dataset`'s schema (`dataset.rs:402-413`, `545-547`) | `src/dataset.rs`, `src/bin/regenerate_dataset.rs`, new `tests/fixtures.rs`, `README.md` | S | Deleting a field from the schema fails `cargo test --test fixtures`; `--force` rewrites the file |
| 7 | Snapshot suite with `insta` (6 non-streamed bodies + 3 SSE transcripts), redactions first | Makes any accidental API-shape change (extra `response_prefix`/`logit_bias` echo, renamed field) a visible diff | new `tests/snapshots.rs`, `Cargo.toml` dev-deps, `tests/snapshots/*.snap` | M | `cargo test` fails on any body-shape change; `cargo insta review` is the documented update path |
| 8 | Pacing test at low rate + multi-byte-content fixture | Verifies `TOKENS_PER_SECOND` (the product's core knob) and guards the byte-slice at `routes.rs:139` | `tests/sse.rs`, `tests/support/fixtures.rs` | S | With `TOKENS_PER_SECOND=2`, total elapsed >= (n-1)/2 s; emoji/CJK answer streams without panic and concatenates exactly |
| 9 | Unit tests for `config::Settings::load` and `tokenizer::load` | Bad env values should fail loudly at boot, not mid-request; all five documented presets must load | `src/config.rs`, `src/tokenizer.rs` | S | `TOKENS_PER_SECOND=0` and `DATASET_SOURCE=bogus` return the matching `SettingsError`; each documented preset loads |
| 10 | Update `AGENTS.md` testing convention + README testing section | Contributors currently get contradictory instructions | `AGENTS.md`, `README.md` | S | Both documents describe the section-0 table and the single `NO_LLM_API_LIVE` gate |
| 11 | Determinism hooks (`with_seed` id/clock injection) and seeded fallback selection, then replace insta redactions | Snapshots and GUI regression runs need byte-stable output; `service.rs:43-49` is nondeterministic under concurrency | `src/service.rs`, `tests/snapshots.rs` | M | Two identical requests to a seeded service produce byte-identical bodies including `id`/`created`; 20 concurrent unmatched prompts return the same sequence on every run |
| 12 | Playwright `e2e/` project driving `index.html` | Catches integration breakage no Rust test sees (CORS, SSE framing in the browser, incremental render) | new `e2e/package.json`, `e2e/playwright.config.ts`, `e2e/chat.spec.ts` | M | `npm --prefix e2e test` boots the server via `webServer`, asserts streamed text grows then equals the scripted answer, and asserts zero console errors |

Item 12 is complete. The suite also intercepts a spec-shaped HTTP failure and verifies that the
embedded client displays its message rather than feeding the JSON error envelope to the SSE parser.
The separate Open WebUI compatibility workflow drives the real third-party GUI weekly and on demand,
because downloading its multi-gigabyte rolling image for every pull request is impractical.
