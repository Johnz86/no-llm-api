# 02 - Conversation Simulation Engine

Design for the deterministic fake-LLM core. Target: a GUI test can run the same scenario 1000x,
in parallel, on any machine, and get byte-identical SSE frames unless it explicitly asked for
variation.

Evidence base for this document: `src/service.rs`, `src/dataset.rs`, `src/store.rs`,
`src/tokenizer.rs`, `src/http/routes.rs`, `src/model.rs`;
`async-openai/async-openai/src/types/chat.rs` (reference client, v0.41.1);
`openapi.documented.yml`; `tiktoken-rs-0.12.0/src/patched_tiktoken.rs`.

---

## 0. Invariants the engine must guarantee

| ID | Invariant | Why it matters for GUI testing |
| --- | --- | --- |
| I1 | `f(normalized_request) -> response_plan` is a pure function. No process-global mutable state participates in selection. | Parallel test runs and retries produce the same assertion targets. |
| I2 | Every non-volatile byte of the response is derived from the plan; only `created` and `id` may vary, and only when explicitly allowed. | Golden-file / snapshot assertions on the SSE transcript. |
| I3 | A request never fails because it is off-script. Unknown input always yields a valid completion. | GUI exploratory testing and fuzzing do not produce 500s that mask real bugs. |
| I4 | The chunk sequence is always well-formed: >=1 chunk, exactly one `finish_reason`, optional usage chunk, terminating `data: [DONE]` - unless a fault was explicitly requested. | Clients (async-openai, Vercel AI SDK, LangChain) hang or error on malformed streams. |
| I5 | Simulated wall-clock behaviour is a declared parameter, not an emergent property of the tokenizer. | Tests can assert TTFT and total duration bounds. |

---

## 1. Deterministic selection

### 1.1 Where the current code violates determinism

| File:line | Code | Violation |
| --- | --- | --- |
| `src/service.rs:29` | `rotation: AtomicUsize` | Selection depends on how many requests the process has served, i.e. on test ordering. |
| `src/service.rs:28` | `cursors: Vec<AtomicUsize>` | Per-script turn pointer is global, not per-conversation. Two concurrent conversations interleave and steal each other's turns. |
| `src/service.rs:44-46` | `rotation.fetch_add(...) % len`, `cursors[i].fetch_add(...)` | `Ordering::Relaxed` fetch_add under concurrency yields a nondeterministic (script, turn) pair per request. |
| `src/service.rs:136-138` | `match ... { None => dataset.next_assistant() }` | Fallback is the nondeterministic path, so *every* off-script prompt (the common case for a GUI under test) is nondeterministic. |
| `src/service.rs:188` | `let created = unix_timestamp()` | Response bytes differ on every call; snapshot tests impossible without scrubbing. |
| `src/service.rs:189-190` | `chatcmpl-{Uuid::new_v4()}`, `req_{Uuid::new_v4()}` | Random ids; same problem, and ids also appear in every stream chunk (`service.rs:418`, inside `chunk_from_delta`). |
| `src/dataset.rs:200-203` | `assistant_at(i)` uses `index % assistants.len()` | Silent wrap-around: a 2-turn script answers turn 7 with turn 1. Masks fixture bugs. |
| `src/service.rs:38-41` | `self.scripts.iter().find_map(...)` | First script whose *any* user turn matches wins. Two fixtures with the same user line are resolved by script id sort order (`dataset.rs:181`), not by conversation context. History and model are ignored entirely. |
| `src/dataset.rs:247` | `normalize = trim().to_ascii_lowercase()` | ASCII-only case folding, no whitespace collapse, no NFC. `"Hello  world"` != `"Hello world"`; any non-ASCII casing mismatch misses. |
| `src/dataset.rs:109` | `grouped: HashMap<String, Vec<RawTurn>>` | `HashMap` iteration order is randomized per process. Mitigated by the sort at `dataset.rs:181` - keep that sort, it is the only reason script order is stable today. |

### 1.2 Normalization rules (`sim::normalize`)

Applied to build the selection key. Must be a pure function with unit tests per rule.

1. Message filter: keep `system`/`developer`, `user`, `assistant`, `tool` messages in order. Drop
   nothing - history shape is part of identity.
2. Content flattening: render each message to a canonical string.
   - `MessageContent::Text(s)` -> `s`.
   - `MessageContent::Parts` -> concatenate part texts with `\n`, and for non-text parts emit a
     stable placeholder token (`[image:<sha of url or first 64 bytes>]`, `[audio:<format>:<len>]`)
     so binary payloads do not enter the hash but their presence does.
3. Text canonicalization, in order: Unicode NFC; replace `\r\n` and `\r` with `\n`; collapse runs
   of Unicode whitespace to a single U+0020; trim; lowercase with `str::to_lowercase` (full Unicode,
   not `to_ascii_lowercase`); strip a trailing run of `.!?` and Unicode quotes.
4. Role prefixing: emit `"{role}\u{1F}{canonical_text}\u{1E}"` per message. Using unit separators
   prevents `user:"a b"` colliding with `user:"a"` + `user:"b"`.
5. Excluded from the key: `temperature`, `top_p`, penalties, `metadata`, `user`, `store`,
   `stream`, `stream_options`, `logit_bias`. These must not change *which* response is chosen -
   a GUI toggling "stream" must get the same text.
6. Included in the key: normalized history, `model` (verbatim, case-sensitive - `gpt-4o` vs
   `gpt-4o-mini` are different personas), `seed`, `n`, `response_format.type` and, when
   `json_schema`, `json_schema.name`, and the sorted list of `tools[].function.name`.

### 1.3 Selection algorithm

```rust
// src/sim/selector.rs - no I/O, no interior mutability
pub struct SelectionKey {
    pub history_digest: u64,   // fnv1a64 over normalized history
    pub prefix_digest: u64,    // fnv1a64 over history minus the trailing user turn
    pub model: String,
    pub seed: u64,             // request.seed, else scenario default, else 0
    pub shape: ShapeKey,       // n, response_format, tool names
}

pub fn select(scripts: &ScriptIndex, key: &SelectionKey) -> Plan {
    // 1. exact multi-turn match: longest scripted prefix equal to the request prefix
    if let Some(hit) = scripts.by_prefix(key.prefix_digest) { return hit.plan(); }
    // 2. exact single-turn match on the last user message (today's behaviour, but indexed)
    if let Some(hit) = scripts.by_last_user(key.history_digest) { return hit.plan(); }
    // 3. deterministic fallback
    let h = mix(key.history_digest, key.model_digest(), key.seed);
    scripts.fallback_pool()[ (h % pool_len) as usize ].plan()
}
```

Hash choice: implement FNV-1a 64 (about 15 lines) or SplitMix64 mixing inside the crate. Do **not**
use `std::collections::hash_map::DefaultHasher` - `SipHasher13` output is explicitly not stable
across Rust releases, which would break golden files on toolchain upgrade. Do not add a hashing
dependency for this; the strings being hashed are small.

Seed participation:
- `seed` is a *selector input only*, mixed into the fallback bucket and into the deterministic PRNG
  used for jitter (section 5). It never affects an exact script match - a scripted prompt answers
  the same way for every seed, which is what fixture authors expect.
- Absent `seed`, use the scenario's `default_seed`, else `0`. Never use time or a random source.
- Echo `seed` back and derive `system_fingerprint` from the plan, e.g.
  `fp_mock_{plan_digest:08x}`. The spec models `system_fingerprint` as the change-detection
  companion to `seed` (`openapi.documented.yml:34453-34455`, and the field is marked
  `deprecated: true` at `openapi.documented.yml:34093-34095`), so a plan-derived value is both
  spec-shaped and useful: a GUI test can assert "the backend plan did not change".
- Type nit: repo has `seed: Option<u64>` (`src/model.rs:110`) while spec and reference client use
  `i64` (`chat.rs:808`). Widen to `i64` and hash the two's-complement bits; negative seeds are legal
  and a client sending `-1` currently gets a 422.

### 1.4 Byte-identical repeats

Two identical POSTs must produce identical bytes. Three volatile fields block that today.
Introduce `IdentityMode` (config `SIM_IDENTITY`, default `deterministic` in test builds):

| Field | `deterministic` | `realistic` (default for demos) |
| --- | --- | --- |
| `id` | `chatcmpl-{plan_digest:016x}` (uuid v5 over the plan digest is an acceptable alternative; requires `uuid` feature `v5`) | `chatcmpl-{uuid v4}` |
| `created` | fixed epoch from config (`SIM_CLOCK_EPOCH`, default `1735689600`) | `unix_timestamp()` |
| `request_id` | `req_{plan_digest:016x}` | random |
| `system_fingerprint` | `fp_mock_{plan_digest:08x}` | same |

Acceptance: `POST` the same body twice in `deterministic` mode, capture raw SSE bytes, assert
equality; and assert equality across two independently spawned server instances in the same test.

---

## 2. Multi-turn coherence

GUIs are stateless: they resend the whole history each turn. The engine must therefore infer
position from the history, never from server-side counters (which is exactly what
`src/service.rs:28` gets wrong).

### 2.1 Turn detection

Build, at load time, a `ScriptIndex` with two maps:

- `by_prefix: HashMap<u64, (script_id, turn_idx)>` - key is the digest of the normalized script
  prefix *up to and including* the user turn that precedes each assistant turn. For
  `conv-orion` (`dataset.rs` sample rows 0-4) that yields two entries: `[system, user#1]` -> turn 2
  and `[system, user#1, assistant#2, user#3]` -> turn 4.
- `by_last_user: HashMap<u64, SmallVec<(script_id, turn_idx)>>` - single-turn fallback matching,
  the indexed form of today's `response_for_user` (`dataset.rs:205-217`). On collision, prefer the
  entry whose script id sorts first, so the behaviour is defined rather than iteration-order noise.

Matching strategy, in order:
1. **Longest scripted prefix.** Compute digests of all request prefixes, longest first, and probe
   `by_prefix`. This handles the case where the GUI has drifted (edited an earlier message) but
   the recent turns are still on-script.
2. **Suffix-anchored prefix.** Same probe using only the last `k` messages for `k` in `4, 3, 2`.
   Lets a script be entered mid-way, which is what happens when a GUI test replays a saved thread.
3. **Last-user-message match** via `by_last_user`.
4. **Fallback** (2.2).

Assistant messages in the request history are *not* required to match the script's assistant text;
only user/tool turns are compared. GUIs mangle assistant text (markdown re-rendering, trailing
whitespace) and a test that edits an assistant bubble should still stay on script.

Tool loop: when the last message is `role: tool`, the prefix probe must include it, and the matched
script turn is the assistant turn that follows the scripted `tool` row. `conv-lyra`
(`dataset.rs` rows 2-4) already encodes exactly this shape: assistant with `tool_calls` /
`finish_reason: tool_calls`, then a `tool` row, then the final assistant answer. Preserve that
convention.

### 2.2 Off-script fallback ladder

Never error (I3). Each rung is deterministic and configurable per scenario
(`fallback: nearest | template | canned | reject`):

| Rung | Behaviour | When to use |
| --- | --- | --- |
| `nearest` | Token-set Jaccard similarity between the normalized last user message and all indexed user turns; accept the best if score >= `fallback_threshold` (default 0.6), ties broken by script id then turn index. Cheap: precompute sorted token-id sets at load. | Human demos: "summarise the sprint" hits the sprint script. |
| `template` | Render a templated reply from the scenario: `"You said: {last_user_message}. (mock reply, script {script_id} not matched)"`, with `{{turn_count}}`, `{{model}}`, `{{token_count}}` also available. | Automated GUI tests that assert echo of input, e.g. XSS/markdown escaping tests. |
| `canned` | Deterministic pick from the `fallback_pool` using the hash from 1.3. | Load/soak tests where content does not matter but variety does. |
| `reject` | Return a spec-shaped 400 `invalid_request_error`. Opt-in only, for negative tests. | Verifying the GUI's error surface. |

Default ladder: `nearest` -> `template`. Log the chosen rung at `debug` with the digest so a
failing test can be diagnosed, and expose it as a response header
`X-Simulate-Match: prefix|suffix|last-user|nearest|template|canned` (headers are invisible to
OpenAI clients, so this is safe).

---

## 3. Response behaviour matrix

Each row is a fixture-expressible behaviour plus the wire shape it must produce. "Client" notes
what the reference Rust client (`async-openai` 0.41.1) can actually deserialize - the strictest
consumer we have.

| Behaviour | Non-streamed shape | Streamed shape | Notes / client constraints |
| --- | --- | --- | --- |
| Plain text | `choices[0].message.content: String`, `finish_reason: stop` | N content deltas + final empty delta with `finish_reason` | `ChatCompletionResponseMessage.content` is `Option<String>` (`chat.rs:422`), **not** parts - multi-part assistant content in the dataset (`dataset.rs` `conv-audio` `content_parts`) must be rendered to a string before serialization or the client fails. |
| Markdown-heavy | same | deltas must be allowed to split mid-fence | Exercises GUI incremental markdown rendering: unbalanced ``` and unclosed table rows mid-stream. Author fixtures with fences, nested lists, inline code, a table, and a long link. |
| Length-capped | truncate to `max_completion_tokens`, `finish_reason: length` | last content delta then `finish_reason: length` | Already implemented (`service.rs:149-169`) but the truncated `content` must still be valid UTF-8 - see 7.2. GUI should show the "response truncated" affordance. |
| Refusal | `message.refusal: String`, `content: null`, `finish_reason: stop` | `delta.refusal` string deltas, then `finish_reason: stop` | Client has `refusal` on both message (`chat.rs:425`) and stream delta (`chat.rs:1013`). Current code sends the whole refusal in the final chunk only (`routes.rs:185`); refusals should stream like content. Note: `finish_reason` for a refusal is `stop`, not `content_filter`. |
| Content filter | `finish_reason: content_filter`, partial or empty content | partial deltas then `finish_reason: content_filter` | `conv-vega` turn 4 encodes this today. Tests the GUI's "flagged" state. |
| Single tool call | `message.tool_calls: [1]`, `content: null`, `finish_reason: tool_calls` | tool-call deltas with `index: 0`, `id`+`type`+`function.name` on the first, `function.arguments` fragments after | `ChatCompletionMessageToolCallChunk` requires `index: u32` and makes `id`/`type`/`function` optional (`chat.rs:991-997`), and `FunctionCallStream.arguments` is `Option<String>` (`chat.rs:987`) - so incremental argument streaming is expected by the client and is currently not done (`routes.rs:187` dumps the whole `tool_calls` array in the final chunk). |
| Parallel tool calls | `tool_calls: [2+]` | interleaved chunks with distinct `index` values | Must be able to interleave (round-robin fragments across indices) to catch GUIs that assume index 0. Gate on `parallel_tool_calls`. |
| Tool-result turn | next script turn after the `tool` message | normal text stream | Full loop coverage; see 2.1. |
| Structured JSON | `content` is a JSON string satisfying the request's `json_schema` | deltas that are *invalid* JSON prefixes until the last one | `ResponseFormat::JsonSchema { json_schema: { name, schema, strict } }` (`chat.rs:488-511`). Fixture stores the JSON payload; the engine validates it against the requested schema only in a `strict` lint mode (do not add a JSON Schema dep for runtime). Tests GUIs that try to `JSON.parse` partial streams. |
| Reasoning / thinking | additive field `reasoning_content: String` on the message | `delta.reasoning_content` deltas emitted **before** any `content` delta | `async-openai` has no `reasoning_content` field anywhere in `chat.rs` (only `reasoning_effort` on the request, `chat.rs:734`, and `reasoning_tokens` in usage details, `chat.rs:122`) - so this is a de-facto extension (DeepSeek/OpenRouter convention) that the reference client silently ignores. That is the correct trade: GUIs that render a thinking pane read it, strict clients are unaffected. Also populate `usage.completion_tokens_details.reasoning_tokens`. Offer a `thinking_style: tag` alternative that wraps reasoning in `<think>...</think>` inside `content` for GUIs using that convention. |
| `n > 1` | `choices[0..n]`, each with its own `index` and `finish_reason` | chunks carry `choices[i]` with the right `index`; per-choice streams may interleave | Request field `n` does not exist in `src/model.rs:78-129`; add it (`Option<u8>`, `chat.rs:776`). Fixture supplies `alternatives`; if fewer than `n` exist, derive extras deterministically (suffix marker `" (variant {i})"`) rather than erroring. |
| Audio modality | `message.audio { id, expires_at, data, transcript }` | client's stream delta has **no** `audio` field (`chat.rs:1002-1013`) | `src/model.rs` `ChatCompletionChunkDelta` has `audio` and `routes.rs:188` sends it - harmless for lenient clients but non-spec. Recommendation: keep audio out of deltas; emit it only on the non-streamed message, and for streams emit the transcript as content plus audio in the final message retrievable via `GET /chat/completions/{id}`. All four `ChatCompletionResponseMessageAudio` fields are non-optional in the client (`chat.rs:406-414`) - fixtures must fill all four. |
| Empty response | `content: null`, `finish_reason: stop`, `usage.completion_tokens: 0` | exactly one chunk: role-only delta with `finish_reason`, then `[DONE]` | Current code handles the `!emitted` case (`routes.rs:168-179`) but only because it back-fills content. Needs an explicit `empty` fixture kind. Catches GUIs that render an empty bubble or spin forever. |

---

## 4. Control plane

Two mechanisms, because they solve different problems: in-band is per-request and parallel-safe;
out-of-band is stateful and convenient for hand-driven GUI sessions.

### 4.1 In-band (preferred for automated tests)

Three equivalent surfaces, resolved in this precedence order: request field > header > magic prompt.

**(a) Request extension field** `x_simulate` (ignored by real OpenAI, ignored by async-openai's
serializer since it builds its own struct - so only reachable by raw-HTTP tests):

```json
{
  "model": "gpt-4o",
  "messages": [{"role": "user", "content": "hi"}],
  "stream": true,
  "x_simulate": {
    "scenario": "markdown-heavy",
    "seed": 42,
    "ttft_ms": 800,
    "tokens_per_second": 5,
    "jitter_ms": [0, 120],
    "fault": { "kind": "http_error", "status": 500, "after_tokens": 12 }
  }
}
```

**(b) `X-Simulate-*` headers** - the only surface usable through a typed client like async-openai
(which lets you set default headers) or through a GUI's "custom headers" box:

| Header | Value grammar | Example |
| --- | --- | --- |
| `X-Simulate-Scenario` | scenario id | `tool-parallel` |
| `X-Simulate-Seed` | i64 | `42` |
| `X-Simulate-Ttft-Ms` | u32 | `1500` |
| `X-Simulate-Tps` | u32, overrides `TOKENS_PER_SECOND` | `3` |
| `X-Simulate-Jitter-Ms` | `min:max` | `0:250` |
| `X-Simulate-Fault` | `kind[;k=v]*` | `http_error;status=503;retry_after=2` |
| `X-Simulate-Fault` | | `stall;after_tokens=10;ms=30000` |
| `X-Simulate-Fault` | | `drop;after_tokens=5` |
| `X-Simulate-Fault` | | `sse_error;after_tokens=8;code=server_error` |
| `X-Simulate-Fault` | | `slow_then_recover;slow_tps=1;for_tokens=6` |
| `X-Simulate-Behaviour` | behaviour id from section 3 | `length`, `refusal`, `json`, `reasoning`, `empty` |

A compound demand such as "stream slowly then 500 mid-stream" is
`X-Simulate-Tps: 2` + `X-Simulate-Fault: http_error;status=500;after_tokens=10`. Because the
response has already begun, the engine implements this as `sse_error` + connection close (see 5).

**(c) Magic prompt directives** - for GUIs where you can only type text. A leading line matching
`^/sim\s+(.+)$` (or an inline `[[sim: ...]]` block) is parsed as the same key=value grammar,
stripped from the prompt before selection, and echoed in `X-Simulate-Match`:

```
/sim behaviour=length tps=4 ttft_ms=1200
Write me a long essay about pagination.
```

Directive parsing must be strictly opt-in (`SIM_INBAND_PROMPT=1`, default off) so that ordinary
fixtures containing a line starting with `/sim` are not hijacked, and unparseable directives are
ignored rather than fatal (I3).

### 4.2 Out-of-band admin API

Namespace: `/_sim/...`. Deliberately not under `/v1` and prefixed with `_` so it can never collide
with an OpenAI path.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/_sim/health` | liveness, loaded scenario count, fixture digest |
| `GET` | `/_sim/scenarios` | list scenario ids, behaviours, turn counts |
| `POST` | `/_sim/scenarios/{id}/activate` | pin the active scenario for subsequent requests |
| `POST` | `/_sim/state/reset` | clear `CompletionStore` and any session state |
| `PUT` | `/_sim/config` | set `tokens_per_second`, `ttft_ms`, `jitter`, `identity_mode` |
| `POST` | `/_sim/faults` | queue a fault for the next N requests (`{"kind":"http_error","status":429,"count":2}`) |
| `DELETE` | `/_sim/faults` | clear queued faults |
| `GET` | `/_sim/requests?limit=50` | recorded request log for assertions (headers, body, chosen plan) |
| `POST` | `/_sim/fixtures/reload` | recompile scenario sources and swap the index atomically |

**Security.** Every one of these mutates or exposes server behaviour, and `/_sim/requests` echoes
whatever the client sent - including `Authorization` headers and prompt text. Therefore:

- Disabled by default. Enable with `SIM_ADMIN=1`.
- When enabled, bind to loopback only by default (`SIM_ADMIN_BIND`, default `127.0.0.1:8081`) on a
  **separate listener** from the API port, so exposing the mock on `0.0.0.0` for a containerized
  GUI does not expose the admin plane.
- If `SIM_ADMIN_BIND` is non-loopback, require `SIM_ADMIN_TOKEN` and enforce it as a bearer token;
  refuse to start otherwise, and log a warning that the admin plane is reachable off-host.
- `/_sim/requests` redacts `Authorization`, `api-key`, and any header matching `(?i)token|secret`
  down to `present`/`absent`. Prompt bodies are only retained when `SIM_ADMIN_RECORD_BODIES=1`.
- Never enable the admin plane in an image published as a public sandbox: pinning a scenario or
  queueing a 500 is a trivially available denial-of-service and an information leak against any
  other user of that instance.

Because activation is server state, automated tests should prefer 4.1 (per-request, parallel-safe).
Admin activation exists for humans clicking through a GUI.

---

## 5. Failure and timing injection

Timing model per stream: `TTFT` -> `N` token gaps -> optional post-stream delay before the usage
chunk. Gap `i` = `1000/tps + jitter(i)` ms, where `jitter(i)` is drawn from a SplitMix64 PRNG
seeded with `mix(plan_digest, seed, i)` - so jitter is reproducible, which the current
fixed-`Duration` loop (`routes.rs:125`) cannot express at all.

| Fault | Wire behaviour | Real GUI behaviour under test |
| --- | --- | --- |
| `ttft_ms` | delay before the first chunk (currently zero; the first token is emitted immediately) | Spinner / "thinking" indicator appears and is dismissed exactly once. Catches GUIs that only show a spinner on non-streamed calls. |
| `jitter_ms` | per-gap random-but-seeded delay | Detects layout thrash and scroll-anchoring bugs from irregular token arrival. |
| `stall;after_tokens=N;ms=M` | emit N tokens, hold the connection open M ms with only SSE keep-alives, then continue | Client read-timeout handling and "still generating" affordances. `routes.rs:210-215` already installs a 15s keep-alive comment - a stall longer than that also verifies the GUI ignores keep-alive frames. |
| `drop;after_tokens=N` | emit N tokens then drop the sender without a final chunk and without `[DONE]` | Partial-message rendering and recovery. async-openai surfaces this as a stream error; a GUI should keep the partial text and mark it incomplete, not discard the bubble. |
| `sse_error;after_tokens=N` | emit N tokens, then one SSE frame whose data is an OpenAI error envelope (`{"error":{"message":...,"type":"server_error","code":null}}`), then close without `[DONE]` | Mid-stream error surfacing. This is how real OpenAI reports failures after headers are sent; the spec shows the shape at `openapi.documented.yml:31533`. |
| `http_error;status=429;retry_after=2` | pre-stream: status 429 + `Retry-After: 2` + `x-ratelimit-remaining-requests: 0` + error envelope | Retry/backoff logic and rate-limit banners. Requires the error envelope work from plan 01. |
| `http_error;status=500` / `503` | pre-stream failure; 503 also sets `Retry-After` | Generic failure paths and circuit breakers. |
| `http_error;...;after_tokens=N` | cannot change status mid-stream; degrade to `sse_error` + close and log the coercion | Documents the HTTP reality so tests do not assert something impossible. |
| `slow_then_recover;slow_tps=1;for_tokens=6` | first 6 gaps at 1 tps, remainder at the normal rate | Progressive-render performance and "slow response" warnings; also the classic bug where a GUI batches tokens and only flushes on a size threshold. |
| `first_chunk_empty` | initial delta with `role` only, no content | Clients that assume every chunk has content. |
| `duplicate_finish` (negative) | two chunks carrying `finish_reason` | Idempotency of the GUI's completion handler. Off by default; useful for hardening. |

All faults are per-request when requested in-band, and counted (`count: N`) when queued via
`/_sim/faults`. Acceptance for each: an integration test that drives the endpoint with `reqwest`
(raw SSE bytes) and asserts frame sequence and elapsed-time bounds using `tokio::time::pause`
where possible.

---

## 6. Fixture authoring ergonomics

Parquet stays the at-rest/wire format (columnar, appendable by the recorder, already the loader's
input at `dataset.rs:105-185`). Add a human-authored source layer that compiles to it.

Pipeline: `fixtures/*.scenario.yaml` --(`cargo run --bin fixtures -- build`)--> `data/*.parquet`
--(existing `ConversationScripts::load`)--> `ScriptIndex`.

Design points:
- Source of truth is the YAML; parquet is generated and should be `.gitignore`d except for one
  committed sample so `cargo run` works on a clean checkout (today `ensure_sample_dataset` at
  `dataset.rs:545` calls `sample_rows`, `dataset.rs:669-996` - ~330 lines of hardcoded `DatasetRow`
  literals. Replacing those with an embedded YAML string via `include_str!` removes the single
  ugliest block in the codebase).
- The compiler is also a linter: unknown keys, unbalanced tool-call/tool-result pairs, refusals
  with a `content_filter` finish reason, audio rows missing any of the four required fields, and
  `json` behaviours whose payload does not parse.
- New columns needed in the parquet schema (`dataset.rs:401-413`): `scenario_id`, `behaviour`,
  `reasoning_content`, `alternatives` (JSON array for `n>1`), `timing` (JSON: ttft/tps/jitter),
  `fault` (JSON). All nullable `Utf8`, so old files still load - the loader already uses
  `column_with_name` and tolerates missing columns (`dataset.rs:351-358`).
- YAML crate: `serde_yaml` is deprecated and archived (dtolnay archived the repo in 2024). Use
  `serde_yaml_ng = "0.10.0"` pinned, or avoid the dependency entirely by authoring in JSON with
  `serde_json` (already a dependency) at the cost of unreadable multi-line strings. Recommendation:
  take `serde_yaml_ng` - block scalars are the whole point for markdown fixtures.

### Example scenario file

```yaml
# fixtures/sprint.scenario.yaml
schema_version: 1
scenario_id: sprint-review
default_seed: 7
models: ["gpt-4o", "gpt-4o-mini"]
fallback: nearest          # nearest | template | canned | reject
fallback_threshold: 0.6

defaults:
  timing:
    ttft_ms: 250
    tokens_per_second: 30
    jitter_ms: [0, 40]

conversations:
  - id: conv-orion
    turns:
      - role: system
        content: You are an efficient assistant who speaks in short sentences.
      - role: user
        content: Summarize the sprint update.
      - role: assistant
        behaviour: markdown
        finish_reason: stop
        content: |
          ## Sprint 42

          - Closed **14** tickets
          - Shipped the analytics pipeline
          - Stabilised the `/v1/chat/completions` surface

          | Area | Status |
          | --- | --- |
          | API | green |
          | Auth | amber |
        timing:
          ttft_ms: 900
          tokens_per_second: 8
      - role: user
        content: Highlight risks?
      - role: assistant
        behaviour: reasoning
        reasoning_content: |
          Checking the risk register: migration slipped, auth failover unverified.
        content: Cloud migration is a week late; auth failover is still in QA.
        usage:
          prompt_tokens: 28
          completion_tokens: 14
          completion_tokens_details:
            reasoning_tokens: 12

  - id: conv-tools
    turns:
      - role: user
        content: Check the checkout service logs and summarise.
      - role: assistant
        behaviour: tool_calls
        finish_reason: tool_calls
        tool_calls:
          - id: call_01
            type: function
            function:
              name: gather_logs
              arguments: '{"service":"checkout","level":"debug"}'
          - id: call_02
            type: function
            function:
              name: read_metrics
              arguments: '{"window":"1h"}'
        stream:
          interleave_tool_arguments: true
          argument_chunk_chars: 8
      - role: tool
        tool_call_id: call_01
        content: '{"status":"ok","lines":128}'
      - role: tool
        tool_call_id: call_02
        content: '{"p99_ms":412}'
      - role: assistant
        content: Logs are clean; p99 is 412 ms, which is above the 300 ms target.

  - id: conv-faults
    turns:
      - role: user
        content: Trigger a mid-stream failure.
      - role: assistant
        content: This response will not finish because the fault fires first.
        fault:
          kind: sse_error
          after_tokens: 4
          code: server_error

fallback_pool:
  - behaviour: text
    content: I do not have a scripted answer for that, but here is a mock reply.
  - behaviour: empty
  - behaviour: length
    content_repeat: "Lorem ipsum dolor sit amet. "
    repeat_count: 400
```

---

## 7. Tokenization and pacing correctness

### 7.1 Why decode-of-growing-buffer is the wrong primitive

`routes.rs:131-139` re-decodes the entire token prefix each iteration and slices
`&decoded[rendered.len()..]`. Two concrete defects:

1. **Streams silently abort on multi-byte content.** `CoreBPE::decode` validates UTF-8 and returns
   `Err` when the byte sequence is incomplete (`tiktoken-rs-0.12.0/src/patched_tiktoken.rs:205-208`:
   `String::from_utf8(...)` -> `Err(anyhow!("Unable to decode into a valid UTF-8 string"))`).
   cl100k_base splits emoji and many CJK characters across several tokens, so a fixture containing
   an emoji hits `Err` on the intermediate prefix, and `routes.rs:133-136` then logs and `return`s -
   the client gets a truncated stream with **no final chunk and no `[DONE]`**, and the request
   appears to hang. The existing fixtures are ASCII, which is why no test catches this; the sample
   refusal row in `dataset.rs` already uses a U+2019 apostrophe, so the repo is one fixture edit
   away from the bug.
2. **O(n^2) work and a latent slice panic.** Byte-slicing at `rendered.len()` is only sound while
   every decoded prefix is a byte-prefix of the next. That holds for well-formed BPE output but is
   not guaranteed by the API contract, and any violation is an index-out-of-bounds panic inside a
   spawned task - i.e. a dropped connection with no diagnostics.

### 7.2 Correct primitive: precomputed byte pieces with a boundary-safe flush

At fixture load time, store per-token byte pieces alongside the token ids
(`CoreBPE::decode_bytes` per token, or `split_by_token_iter` which already does lossy per-token
strings at `patched_tiktoken.rs:282-284`). At stream time:

```
pending: Vec<u8>
for piece in pieces:
    pending.extend(piece)
    match str::from_utf8(&pending):
        Ok(s)  => emit s; pending.clear()
        Err(e) if e.error_len().is_none() =>       // incomplete tail, not invalid
            emit valid prefix (pending[..e.valid_up_to()]); retain the remainder
        Err(_) => emit U+FFFD, clear             // genuinely invalid: never abort
```

Properties: O(total bytes), never panics, never aborts the stream, and a grapheme split across
tokens is emitted as soon as it completes - the same visible behaviour as real providers, where a
multi-token emoji appears in one delta.

Truncation for `finish_reason: length` (`service.rs:149-169`) must use the same boundary logic: cut
at the token boundary, then trim any incomplete trailing UTF-8 sequence rather than relying on
`decode` succeeding.

### 7.3 Whitespace-leading tokens

cl100k_base tokens carry their leading space (`" world"`). Emitting the raw piece therefore
reproduces real provider behaviour, including deltas that are pure whitespace or a lone `"\n"`.
Do not trim deltas - GUIs that `trim()` each chunk and re-join produce visibly wrong text, and this
mock should expose that bug. Add a fixture whose text has double spaces, a leading newline, and a
trailing space, and assert the concatenation of deltas equals the full text byte-for-byte.

### 7.4 Rate, burst, and pacing

- Current pacing sleeps after every token including the last (`routes.rs:160-161`), adding one full
  gap before the final chunk. Move the sleep to the *start* of each gap after the first token and
  drive TTFT explicitly, so total time is `ttft + (n-1)/tps` rather than `n/tps`.
- Add `SIM_BURST_TOKENS` / `x_simulate.burst`: emit the first B tokens with no delay, then pace.
  Real providers do this (server-side batching), and it is the cheapest way to reproduce GUIs that
  mis-handle several deltas arriving in one network read.
- Add `SIM_CHUNK_TOKENS` (default 1): group T tokens per SSE frame. Real providers frequently send
  multi-token chunks; a GUI that assumes one token per frame for typing animation breaks.
- `tokens_per_second: 0` should mean "no pacing" (instant), which the current `NonZeroU32`
  (`config.rs:11`) forbids; represent unlimited as a separate variant rather than abusing 0.
- Under `tokio::time::pause`, pacing must be driven only by `tokio::time::sleep` (already true) so
  timing tests are instant and deterministic.

---

## 8. Staged implementation checklist

Ranked by value per day of work for cheap GUI testing. Stages are independently shippable.

### Stage A - determinism and stream safety (do first)

| # | What | Why | Files | Effort | Acceptance |
| --- | --- | --- | --- | --- | --- |
| A1 | Boundary-safe streaming: precompute per-token byte pieces, flush on UTF-8 boundaries, never abort | A single non-ASCII fixture currently hangs the client with no `[DONE]` (7.1) | `src/dataset.rs`, `src/http/routes.rs`, new `src/sim/stream.rs` | M | Test: fixture with emoji + CJK; assert concatenated deltas == full text and that the transcript ends with `[DONE]` |
| A2 | `sim::normalize` + `SelectionKey` + FNV-1a digest, unit-tested per rule | Prerequisite for every other determinism item (1.2) | new `src/sim/normalize.rs`, `src/sim/hash.rs` | M | Table test: 12 input pairs, each asserting equal/unequal digests (NFC, CRLF, whitespace runs, Unicode case, punctuation, role separation) |
| A3 | Replace `rotation`/`cursors` atomics with `ScriptIndex` + pure `select()` | Removes the order dependence at `service.rs:28-46` | `src/service.rs`, `src/dataset.rs`, new `src/sim/selector.rs` | M | Test: spawn 64 concurrent identical requests, assert all responses identical; assert a fixed request maps to the same script across 100 runs |
| A4 | `IdentityMode`: deterministic `id`/`created`/`request_id`/`system_fingerprint` | Enables golden-file assertions (1.4) | `src/config.rs`, `src/service.rs` | S | Test: two identical POSTs in deterministic mode produce byte-identical SSE bodies |
| A5 | Multi-turn prefix matching + fallback ladder (`nearest` -> `template`) | GUI resends full history; today only the last user line is used (2.1) | `src/sim/selector.rs`, `src/service.rs` | M | Test: replay `conv-orion` turn-by-turn, assert turn 2 then turn 4; assert an off-script prompt returns 200 with the templated echo |
| A6 | Remove `index % len` wrap in `assistant_at` | Silent wrap hides fixture errors (`dataset.rs:200`) | `src/dataset.rs` | S | Test: requesting a turn beyond the script falls through to the fallback ladder rather than replaying turn 0 |

### Stage B - control plane and injection

| # | What | Why | Files | Effort | Acceptance |
| --- | --- | --- | --- | --- | --- |
| B1 | `SimulationDirective` parsed from `x_simulate` body field and `X-Simulate-*` headers, with precedence | Per-request, parallel-safe control (4.1) | new `src/sim/directive.rs`, `src/model.rs`, `src/http/routes.rs` | M | Test: header and body forms produce the same directive; unknown keys ignored, never 500 |
| B2 | Timing engine: `ttft_ms`, seeded per-token jitter, `burst`, `chunk_tokens`, per-request tps override | Every spinner/partial-render test needs this (5, 7.4) | `src/sim/stream.rs`, `src/config.rs` | M | Test with `tokio::time::pause`: assert first-chunk arrival >= ttft and total elapsed within one gap of `ttft + (n-1)/tps` |
| B3 | Faults: `stall`, `drop`, `sse_error`, `http_error` (pre-stream), `slow_then_recover` | Covers retry, abort, partial-render paths (5) | `src/sim/stream.rs`, `src/http/routes.rs` | M | One integration test per fault asserting exact frame sequence; `http_error` asserts status, `Retry-After`, and the OpenAI error envelope |
| B4 | Incremental tool-call argument streaming with correct `index`, plus parallel interleave | Reference client models this shape (`chat.rs:991-997`); GUIs render tool cards from it | `src/sim/stream.rs`, `src/model.rs` | M | async-openai test: accumulate `tool_calls` chunks and assert the reassembled arguments parse as the fixture JSON, for 1 and 2 calls |
| B5 | `/_sim` admin router on a separate loopback listener, off by default, with redaction and token gate | Human-driven GUI sessions; must not be a hole (4.2) | new `src/http/admin.rs`, `src/main.rs`, `src/config.rs` | M | Test: default build returns 404 for `/_sim/health`; with `SIM_ADMIN=1` returns 200 on the admin port and still 404 on the API port; non-loopback bind without a token fails startup |

### Stage C - fixtures and behaviour coverage

| # | What | Why | Files | Effort | Acceptance |
| --- | --- | --- | --- | --- | --- |
| C1 | YAML scenario format + `fixtures build` binary + linter; parquet schema gains `scenario_id`, `behaviour`, `reasoning_content`, `alternatives`, `timing`, `fault` | Parquet is unauthorable by hand; also deletes ~330 lines of hardcoded rows in `dataset.rs:669-996` (6) | new `fixtures/*.yaml`, new `src/bin/fixtures.rs`, `src/dataset.rs`, `Cargo.toml` (`serde_yaml_ng` 0.10.0) | L | `cargo run --bin fixtures -- build` regenerates `data/conversations.parquet`; round-trip test YAML -> parquet -> `ScriptIndex` preserves every field; linter rejects a deliberately broken fixture |
| C2 | Behaviour coverage fixtures: markdown, length, refusal, content_filter, json_schema, reasoning, empty, audio, long (2k tokens) | The matrix in section 3 is only real if fixtures exist | `fixtures/*.yaml` | M | One test per behaviour asserting the documented wire shape, streamed and non-streamed |
| C3 | `reasoning_content` on message and delta (+ `thinking_style: tag` alternative), `reasoning_tokens` in usage | Modern GUIs render a thinking pane; the reference client ignores the field safely (`chat.rs` has no such field) | `src/model.rs`, `src/sim/stream.rs` | S | Test: reasoning deltas all precede content deltas; async-openai compat test still deserializes without error |
| C4 | `n > 1` support: request field, per-choice `index`, deterministic derived alternatives | `n` is absent from `src/model.rs:78-129`; multi-choice GUIs cannot be tested at all | `src/model.rs`, `src/service.rs`, `src/sim/stream.rs` | M | Test: `n=3` non-streamed returns 3 choices with indices 0,1,2; streamed chunks carry all three indices and exactly three `finish_reason`s |
| C5 | Render multi-part assistant content to `String` before serialization | `ChatCompletionResponseMessage.content` is `Option<String>` in the reference client (`chat.rs:422`); `conv-audio` currently emits an array | `src/service.rs`, `src/model.rs` | S | async-openai compat test that requests the audio fixture and deserializes the non-streamed response |
| C6 | Drop `audio` from stream deltas; keep it on the message | Not present in `ChatCompletionStreamResponseDelta` (`chat.rs:1002-1013`) | `src/model.rs`, `src/http/routes.rs` | S | Streamed audio fixture emits transcript as content, and `GET /chat/completions/{id}` returns the audio object |

### Stage D - nice to have once A-C land

| # | What | Why | Files | Effort | Acceptance |
| --- | --- | --- | --- | --- | --- |
| D1 | `/_sim/requests` request log | Lets GUI tests assert what the client actually sent (headers, tool schemas) | `src/http/admin.rs`, `src/store.rs` | M | Test: after two POSTs, the log has two entries with redacted auth headers |
| D2 | Golden-transcript test harness: record raw SSE bytes to `tests/golden/*.sse` and diff | Cheapest possible regression net for the whole engine | `tests/` | M | `cargo test` fails when any chunk shape changes; `UPDATE_GOLDEN=1` refreshes |
| D3 | Session affinity by `metadata.conversation_id` for GUIs that do not resend history | Some GUIs send only the newest message | `src/service.rs`, `src/store.rs` | M | Test: two single-message requests sharing a `conversation_id` advance the script |
| D4 | `strict` JSON-schema lint of `json` fixtures at build time | Catches fixtures that violate the schema a GUI asked for | `src/bin/fixtures.rs` | M | Linter rejects a fixture whose payload omits a required property |
