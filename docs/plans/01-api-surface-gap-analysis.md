# 01 - API Surface Gap Analysis

Scope: field-by-field / endpoint-by-endpoint diff of this repo against (a) `openapi.documented.yml`
(bundled OpenAI spec) and (b) `async-openai` 0.41.1 types. Planning only.

Reference shorthand used below:

| Alias | Path |
| --- | --- |
| `SPEC` | `C:\GIT\rust\no-llm-api\openapi.documented.yml` (line numbers verified this session) |
| `AO` | `%USERPROFILE%\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\async-openai-0.41.1\src` |
| `AO_CHAT` | `AO\types\chat\chat_.rs` |

Anchors: `SPEC:33702` CreateChatCompletionRequest, `SPEC:34021` CreateChatCompletionResponse,
`SPEC:34156` CreateChatCompletionStreamResponse, `SPEC:32480` ChatCompletionResponseMessage,
`SPEC:32635` ChatCompletionStreamResponseDelta, `SPEC:31932` ChatCompletionMessageToolCallChunk,
`SPEC:32607` ChatCompletionStreamOptions, `SPEC:37977` Error, `SPEC:38015` ErrorResponse,
`SPEC:42204` ListModelsResponse, `SPEC:43486` Model, `SPEC:57403` ServiceTier.
`AO_CHAT:745` CreateChatCompletionRequest, `:1080` CreateChatCompletionResponse,
`:1178` CreateChatCompletionStreamResponse, `:439` ChatCompletionResponseMessage,
`:1140` ChatCompletionStreamResponseDelta, `:1123` ChatCompletionMessageToolCallChunk,
`:1238` ChatCompletionMessageListItem, `:79` Role.

---

## 1. Request surface (`src/model.rs:78-129`)

### 1.1 Fields absent from `ChatCompletionRequest`

All are silently dropped (serde ignores unknown keys), so a GUI gets a 200 that quietly ignored
its intent - the worst failure mode for a test harness.

| Field | Spec / AO type | GUI relevance |
| --- | --- | --- |
| `n` | `Option<u8>` (`AO_CHAT:745` block) | low-med: "generate 3 variants" UIs; needs multi-choice responses + per-index stream deltas |
| `logprobs`, `top_logprobs` | `Option<bool>`, `Option<u8>` | low: token-probability inspectors (e.g. debug panels) |
| `prediction` | `PredictionContent` (`AO_CHAT:696`) | low: only exercises `completion_tokens_details.accepted/rejected_prediction_tokens` |
| `web_search_options` | `WebSearchOptions` (`AO_CHAT:642`) | med: pairs with `message.annotations` citations rendering |
| `verbosity` | `Verbosity` (`AO_CHAT:663`) | low |
| `prompt_cache_key`, `safety_identifier` | `Option<String>` | low, but they replace `user`; trivial to accept |
| `functions` (deprecated) | `Vec<ChatCompletionFunctions>` | low: legacy GUIs still send it |
| `stream_options.include_obfuscation` | `SPEC:32624`, `AO_CHAT:1000` | must be *accepted* (currently ignored, which is acceptable behaviour, but see 3.6) |
| `tool_call_id` on messages | required on `ChatCompletionRequestToolMessage` | HIGH: tool-result round-trips lose the call linkage entirely (`src/model.rs:55-70` has no such field) |

### 1.2 Wrongly typed / `Value`-shaped where a typed enum is required

`Value`/`String` fields accept garbage and echo it back, so a GUI's malformed request looks
successful. Each row below is a validation gap, not only a typing nit.

| Repo field | Current | Required shape | Notes |
| --- | --- | --- | --- |
| `stop` (`model.rs:92`) | `Value` | `StopConfiguration` = string \| 1..4 strings (`AO_CHAT:29`) | 5+ entries must 400 |
| `response_format` (`:102`) | `Value` | tagged `text` \| `json_object` \| `json_schema{json_schema{name,schema,strict}}` (`AO\types\shared\response_format.rs:5`) | structured-output GUIs |
| `tools` (`:110`) | `Vec<Value>` | `Vec<ChatCompletionTools>`, tag `type: function\|custom` (`AO_CHAT:487`) | `function.name` pattern + `parameters` JSON-Schema unvalidated |
| `tool_choice` (`:112`) | `Value` | `none\|auto\|required` \| `{type:function,function:{name}}` \| allowed_tools \| custom (`AO_CHAT:552`) | forced-tool tests silently ignored |
| `audio` (`:124`) | `Value` | `{voice, format}` enums (`AO_CHAT` ChatCompletionAudio) | |
| `modalities` (`:118`) | `Vec<String>` | `[text\|audio]` enum | |
| `reasoning_effort` (`:98`) | `String` | `none\|minimal\|low\|medium\|high\|xhigh` (`AO\types\shared\reasoning_effort.rs:5`) | |
| `service_tier` (`:100`) | `String` | `auto\|default\|flex\|scale\|priority` (`SPEC:57403`) | echoed to response -> breaks typed clients (see 2.2) |
| `logit_bias` (`:106`) | `Map<String,Value>` | `HashMap<String,i8>` (-100..100) | |
| `seed` (`:108`) | `u64` | spec/AO `i64` | negative seed currently 400s where OpenAI accepts |
| `stream` (`:82`) | `bool` (non-nullable) | nullable in spec | literal `"stream": null` currently 400s |
| `messages[].content` parts (`:24-27`) | `Vec<Value>` | tagged parts `text\|image_url\|input_audio\|file` per role (`AO_CHAT:366` family) | vision GUIs send `image_url`; renderer at `model.rs:37-52` just concatenates any `text`/`content` key |
| `messages[]` role model (`:55-70`) | one flat struct, all fields optional, any role | role-discriminated union with per-role required `content`/`tool_call_id`/`name` | assistant-only fields accepted on `user` etc. |
| `response_prefix` (`:104`) | `Option<String>` | not in OpenAI spec | repo-invented; keep but document as an extension, do not echo (2.1) |

### 1.3 Validation that should exist but does not

`messages: []` is accepted and served a round-robin fixture (`service.rs:139-142`);
spec requires `minItems: 1`. Unknown `model` returns 200 with the echoed name.
`temperature` > 2, `top_p` > 1, `frequency_penalty`/`presence_penalty` outside -2..2,
`top_logprobs` without `logprobs`, `n` outside 1..128 - all unchecked. A GUI cannot test its
error banner without these.

---

## 2. Response surface (`src/model.rs:243-287`, built in `src/service.rs:231-252`)

### 2.1 Non-spec fields emitted on `POST /chat/completions`

`SPEC:34021` allows exactly `id, choices, created, model, service_tier, system_fingerprint,
object, usage`. `AO_CHAT:1080` matches that. The repo additionally emits 16 fields, all
`#[serde(default)]` with no `skip_serializing_if`, so they appear as explicit `null`s:

`metadata, request_id, temperature, top_p, frequency_penalty, presence_penalty, stop, seed,
tool_choice, response_format, parallel_tool_calls, modalities, response_prefix, logit_bias,
stream_options, audio` (`model.rs:255-287`).

Two distinct problems:

1. Request-state leakage: `logit_bias`, `stream_options`, `response_prefix`, `audio`, `modalities`
   are echoed straight back from the request (`service.rs:243-252`). Nothing in OpenAI does this.
   `async-openai` tolerates unknown keys (no `deny_unknown_fields`), but a strict client
   (Go `DisallowUnknownFields`, JSON-Schema validation in a GUI test suite) rejects the body.
2. Wrong endpoint: `SPEC:3068-3100` (the `listChatCompletions` example) shows that the *stored*
   completion object returned by GET/list legitimately carries `request_id, tool_choice, seed,
   top_p, temperature, presence_penalty, frequency_penalty, service_tier, tools, metadata,
   response_format, input_user`. So these fields belong on `GET /chat/completions{,/{id}}` only -
   and the repo is missing `tools` and `input_user` there.

Correct split: lean object from POST; enriched object from GET/list.

### 2.2 Missing / wrong response fields

| Item | Evidence | Impact |
| --- | --- | --- |
| `message.content` can serialize as a JSON **array** | fixtures with `content_parts` become `MessageContent::Parts` (`dataset.rs:297-301`) and flow unchanged into the response message (`service.rs:196-211`) | HARD BREAK: `AO_CHAT:439` declares `content: Option<String>`; any parts-bearing fixture makes every typed client fail to parse |
| `message.annotations` missing | `SPEC:32497` | GUIs that render web-search citations show nothing |
| tool-call nulls | `model.rs:369-375` has `id/type/function` all `Option` | `AO_CHAT:384` needs `id: String`, tag `type`, `function{name,arguments}` both `String`; a partial fixture emits `"type": null` and breaks the internally-tagged enum. Current sample fixture happens to be complete (`dataset.rs:782`), which is why the compat test passes |
| `service_tier` always `"default"` | `service.rs:226-229` | spec: present only when the request set it; and the value is an unvalidated echo of the request string |
| `system_fingerprint` = `"fp_mock"` | `service.rs:239` | real format is `fp_<10 hex>`; GUIs that display or diff fingerprints see an unrealistic value |
| `logprobs` always `null` | `service.rs:216` | `logprobs: true` cannot be tested |
| `role` may be `developer` | `ChatRole` (`model.rs:7-15`) vs `AO_CHAT:79` `Role` (no `Developer`) | listing messages of a conversation that used a developer message breaks the reference client |
| `/messages` items lack `content_parts` | `AO_CHAT:1238`, `SPEC` example at `SPEC:4776-4784` | spec puts the array in `content_parts` and the flattened string in `content`; repo stuffs the array into `content` and adds a non-spec `name` at top level (`model.rs:307-322`) - `name: null` does appear in the OpenAI example, so `name` is fine; the `content`/`content_parts` split is not |

Correct by construction today: `object`, `created`, `choices[].index`, `finish_reason` enum values,
`usage` + `prompt_tokens_details` / `completion_tokens_details`, `logprobs: null` present rather
than omitted (`SPEC:34032` requires the key), and the `ChatCompletionList` / `ChatCompletionDeleted`
envelopes.

---

## 3. Streaming chunk correctness (`src/http/routes.rs:104-218`, `src/service.rs:405-460`)

| # | OpenAI behaviour | Repo behaviour | Verdict |
| --- | --- | --- | --- |
| 3.1 | First chunk is role-only: `delta:{"role":"assistant","content":""}`, then content chunks (`SPEC:34272` example) | role folded into the first *content* chunk (`routes.rs:135-140`, `delta_for_text(.., index == 0)` at `service.rs:428`) | cosmetic for most GUIs; fix while touching this code |
| 3.2 | Tool calls stream as `ChatCompletionMessageToolCallChunk`: `index` **required** (`SPEC:31959`, `AO_CHAT:1123` `pub index: u32`), `id`+`type` only on the first fragment, `function.arguments` split across chunks | one final chunk carrying the whole `ChatCompletionMessageToolCall` with **no `index`** (`routes.rs:187`) | HARD BREAK: `serde` reports missing field `index`; `async-openai` stream parsing fails. Unexercised because `tests/async_openai.rs:87-101` only tests tool calls non-streamed |
| 3.3 | `refusal` arrives as incremental deltas | whole refusal attached to the final chunk (`routes.rs:185`) | GUI refusal panels never animate; low-med |
| 3.4 | Final chunk is `delta:{}` + `finish_reason` (`SPEC:34285`) | final chunk may also carry content / tool_calls / refusal / audio (`routes.rs:170-190`) | med: double-render risk in GUIs that append every delta |
| 3.5 | Chunk has `service_tier` (`SPEC:34232`); delta has no `audio` field (`SPEC:32635-32677`) | chunk lacks `service_tier` (`model.rs:331-341`); delta emits `audio` (`model.rs:366`) | low both ways; `Option` fields make AO tolerant |
| 3.6 | `obfuscation` is **not** part of the documented chunk schema (in this spec it appears only on Realtime/Responses delta events, e.g. `SPEC:46824`); only the request flag `stream_options.include_obfuscation` is documented (`SPEC:32624`) | flag ignored, field not emitted | acceptable as-is. Do not invent an `obfuscation` field; optionally add it behind a config flag for GUIs that were written against live traffic |
| 3.7 | Usage chunk: `choices: []` + populated `usage`, sent immediately before `[DONE]`; other chunks carry `usage: null` | exactly that (`service.rs:450-460`, `routes.rs:196-207`) | correct |
| 3.8 | Framing `data: {json}\n\n`, terminator `data: [DONE]\n\n` | axum `Event::json_data` + `.data("[DONE]")` (`routes.rs:207`) | correct |
| 3.9 | No SSE keep-alive comments on chat completions | `: ping` comment every 15s (`routes.rs:210-216`) | med: naive hand-rolled GUI parsers that split on `data:` can mis-handle comment lines; make keep-alive opt-in |
| 3.10 | Mid-stream failure is delivered as an SSE payload containing an error object, then the stream ends | tokenizer decode failure `return`s from the spawned task without sending `[DONE]` (`routes.rs:119-126`) | med: client hangs until its own timeout - bad for deterministic tests |
| 3.11 | With `n > 1`, chunks carry one choice per `index` | single choice, `index: 0` hardcoded (`service.rs:405-419`) | tracks the `n` gap in 1.1 |

---

## 4. Error envelope

Required shape (`SPEC:37977` + `SPEC:38015`): `{"error":{"message":str,"type":str,"param":str|null,
"code":str|null}}` with all four keys required. `async-openai`'s `ApiError`
(`AO\error.rs:79`) has `message: String` plus `Option` for the rest, so it tolerates the repo's
two-field body - but GUIs that surface `code` see `undefined`.

Current state:

| Situation | Repo today | Should be |
| --- | --- | --- |
| Envelope shape | `{error:{message,type}}` only (`routes.rs:382-387`) | add `param` and `code` (nullable) |
| Any `ServiceError` | 500 + `type: "server_error"` (`routes.rs:409-415`) | map per cause; `EmptyResponse` is arguably 500, live-backend failures should surface upstream status |
| Malformed JSON / wrong content-type | axum `Json` rejection: plain-text 400/415/422 | 400 envelope, `type: invalid_request_error` |
| Unknown route | empty-body 404 (no `fallback` in `build_router`, `routes.rs:31-54`) | 404 envelope, message `Invalid URL (POST /v1/foo)` |
| Wrong method / `OPTIONS` | empty-body 405 | 405 envelope; `OPTIONS` needs CORS preflight (see plan 02) |
| Bad `limit`/`after` query types | axum `Query` rejection, plain text | 400 envelope. (`order` already returns an envelope: `routes.rs:78-81`) |
| Missing/invalid `Authorization` | ignored entirely | 401 envelope, `code: invalid_api_key` (opt-in via config so existing keyless tests keep working) |

Recommended injectable catalogue (statuses confirmed by the OpenAI error-codes guide;
`type` strings for 429 vary in the wild between `requests`/`tokens`, so make them config-driven):

| Trigger | HTTP | `type` | `code` | `param` |
| --- | --- | --- | --- | --- |
| bad field / bad JSON | 400 | `invalid_request_error` | null | offending field |
| prompt too long | 400 | `invalid_request_error` | `context_length_exceeded` | `messages` |
| bad or missing key | 401 | `invalid_request_error` | `invalid_api_key` | null |
| unknown model | 404 | `invalid_request_error` | `model_not_found` | `model` |
| rate limited | 429 | `rate_limit_error` (configurable) | `rate_limit_exceeded` | null |
| out of quota | 429 | `insufficient_quota` | `insufficient_quota` | null |
| internal | 500 | `server_error` | null | null |
| overloaded | 503 | `server_error` | null | null |

Error/latency injection mechanics are plan 03's job; this plan only fixes the envelope and the
status/type/code mapping.

---

## 5. Missing endpoints a chat GUI touches

| Endpoint | Spec | Verdict |
| --- | --- | --- |
| `GET /v1/models` | `SPEC:13065` -> `ListModelsResponse` `{object:"list",data:[Model]}` (`SPEC:42204`, `AO\types\models\model.rs:17`) | **In scope, highest endpoint value.** Model pickers in Open WebUI / LibreChat / Chatbox-style GUIs populate from here; without it they show an empty dropdown or refuse to start |
| `GET /v1/models/{model}` | `SPEC:13194` -> `Model` `{id,object:"model",created,owned_by}` (`SPEC:43486`) | **In scope, trivial once the list exists**; unknown id must be 404 `model_not_found` |
| `GET /health` (non-OpenAI) | - | **In scope**: needed by docker/compose healthchecks and CI wait-loops |
| `POST /v1/responses` | `SPEC:18339` | **Out of short-term scope.** Its streaming surface is dozens of typed events (`response.output_text.delta`, `response.function_call_arguments.delta`, ...) - a multi-week port with its own state model. Only newer GUIs default to it, and most still support Chat Completions. Revisit only if a target GUI is Responses-only |
| `POST /v1/embeddings` | `SPEC:6999` | **Optional S.** Deterministic pseudo-vector (hash -> normalized floats) is ~60 lines and unblocks GUIs with local RAG/"memory" features. Not needed for chat testing; do it only if a target GUI calls it at startup |
| `POST /v1/moderations` | `SPEC:13429` | **Optional S.** Some GUIs pre-flight user input. Static "not flagged" response with all category scores 0 is cheap |
| `/v1/audio/*` | speech/transcription | **Out of scope.** Requires multipart upload and binary audio bodies; only voice UIs need it |

For everything deliberately unimplemented, return a 501 (or 404) with a *proper error envelope*
so a GUI fails fast and legibly instead of hanging on an empty-body response.

---

## 6. Route-level issues (`src/http/routes.rs:31-61`, `src/main.rs:44-50`)

| Issue | Evidence | Impact |
| --- | --- | --- |
| `/` reads `index.html` from the process CWD on every request | `routes.rs:56-61` | breaks whenever the binary runs from another directory (docker `WORKDIR`, `cargo test` from a subdir); failure is a plain-text 500, not JSON. Use `include_str!` and gate on a config flag |
| No `fallback` handler | `routes.rs:31-54` | unknown routes -> empty 404 (see section 4) |
| No `OPTIONS` handling | no `CorsLayer` in `build_router` or `main.rs:44` | any browser-hosted GUI on a different origin fails preflight. Cross-ref plan 02 |
| `HEAD` behaviour unverified | axum's `get()` answers HEAD implicitly | assert it in a test rather than assuming |
| Trailing slash unhandled | axum 0.8 performs no trailing-slash redirect | `/v1/chat/completions/` behaviour is currently unknown; add `NormalizePathLayer` plus a test that pins it |
| No `/v1` root, no `/v1/health` | `routes.rs:47-54` | probes hitting `/v1` get an empty 404 |
| `limit` silently clamped to 100 | `routes.rs:65`, `:333` | matches OpenAI's cap in spirit; document it |
| No `X-Request-ID` response header | - | `request_id` exists only in the JSON body; GUIs/debug panels that read the header see nothing. Low |

---

## 7. Prioritized work items

Ranked by value for cheap, deterministic GUI testing. Every acceptance criterion is checkable by a
test in `tests/` or a single command.

### P0 - correctness breaks for a real client

- [ ] **A1. Stream tool calls as `ChatCompletionMessageToolCallChunk` with `index`**
  - What: new chunk-delta tool-call type carrying `index: u32`, `id`/`type` only in the first
    fragment, `function.arguments` split into >= 2 fragments; emit them progressively instead of
    dumping the full tool call in the final chunk.
  - Why: `async-openai` and every typed SDK fail to deserialize the current chunk, so a GUI's
    streamed tool-call path cannot be tested at all.
  - Files: `src/model.rs` (new `ChatCompletionMessageToolCallChunk`), `src/service.rs`
    (`chunk_from_delta`, delta builders), `src/http/routes.rs` (stream loop).
  - Effort: M
  - Acceptance: `ASYNC_OPENAI_COMPAT=1 cargo test` includes a new streamed tool-call case that
    consumes the stream via `client.chat().create_stream()` without error and asserts
    (a) every `tool_calls` delta has `index == 0`, (b) `id` appears exactly once,
    (c) concatenated `arguments` fragments parse as JSON and equal the fixture.

- [ ] **A2. Response `content` must be a string; move parts to `content_parts`**
  - What: render `MessageContent::Parts` to text for `choices[].message.content`; expose the array
    only as `content_parts` on `/chat/completions/{id}/messages` items.
  - Why: any fixture with `content_parts` currently produces a JSON array where the spec and
    `AO_CHAT:439` require `string|null` - a hard parse failure for the whole response.
  - Files: `src/model.rs` (split response vs request content types), `src/service.rs`,
    `src/store.rs`, `src/dataset.rs` (fixture with parts).
  - Effort: M
  - Acceptance: new unit test asserts `serde_json::to_value(response)["choices"][0]["message"]["content"].is_string()`
    for a parts-bearing fixture; a `/messages` test asserts `content` is a string and
    `content_parts` is the array; compat test passes with the parts fixture in the dataset.

- [ ] **A3. OpenAI error envelope everywhere, with status/type/code mapping**
  - What: add `param`/`code` to the error body; a `fallback` handler for unknown routes; envelope
    responses for `Json`/`Query` rejections, 405, and 415; the mapping table in section 4.
  - Why: GUIs render `error.message`/`error.code` in their banners and retry on specific codes;
    today malformed input yields axum plain text and unknown routes an empty body.
  - Files: `src/http/routes.rs` (or a new `src/http/error.rs`), `src/service.rs` (typed errors).
  - Effort: M
  - Acceptance: tests asserting (i) `POST /v1/chat/completions` with `{"bad json` -> 400 and body
    has `error.{message,type,param,code}` keys, (ii) `GET /v1/nope` -> 404 envelope,
    (iii) `PUT /v1/chat/completions` -> 405 envelope, (iv) `Content-Type: text/plain` -> 400/415
    envelope.

### P1 - endpoints and payload hygiene GUIs depend on

- [ ] **B1. `GET /v1/models` and `GET /v1/models/{model}`**
  - What: serve `ListModelsResponse` / `Model` from a configured model list (default: the model ids
    present in the loaded parquet plus a couple of well-known aliases); 404 `model_not_found` for
    unknown ids.
  - Why: model pickers fail closed without this; it is the first request most GUIs make.
  - Files: `src/http/routes.rs`, `src/model.rs`, `src/config.rs`, `README.md`.
  - Effort: S
  - Acceptance: test deserializes `GET /v1/models` into `async_openai::types::models::ListModelResponse`
    (or asserts `object=="list"` and every item has `id/object/created/owned_by`), asserts
    `GET /v1/models/{known}` -> 200 and `GET /v1/models/does-not-exist` -> 404 with
    `error.code == "model_not_found"`.

- [ ] **B2. Split lean POST response from enriched stored object**
  - What: POST returns only `id, object, created, model, choices, usage, system_fingerprint`
    (+`service_tier` when requested). GET/list keep `request_id, metadata, temperature, top_p,
    penalties, stop, seed, tool_choice, response_format, service_tier` and gain `tools`,
    `input_user`; drop `response_prefix`, `logit_bias`, `stream_options`, `audio`, `modalities`,
    `parallel_tool_calls` from all responses.
  - Why: removes request-state leakage and stops strict clients rejecting the create response;
    aligns list/get with `SPEC:3068-3100`.
  - Files: `src/model.rs`, `src/service.rs`, `src/store.rs`, `src/http/routes.rs`.
  - Effort: M
  - Acceptance: test asserts the POST body's top-level key set equals the spec set exactly, and
    that a stored completion fetched via GET contains `request_id`+`metadata` but no
    `logit_bias`/`stream_options`/`response_prefix`.

- [ ] **B3. Typed request enums + rejection of invalid values**
  - What: replace `Value`/`String` with typed enums for `stop`, `response_format`, `tools`,
    `tool_choice`, `audio`, `modalities`, `reasoning_effort`, `service_tier`; `logit_bias` ->
    `HashMap<String,i8>`; `seed` -> `i64`; `stream` -> `Option<bool>`; role-discriminated messages
    with `tool_call_id`.
  - Why: a mock that accepts anything cannot validate a GUI's request construction; typed enums
    turn silent acceptance into the 400 the GUI expects.
  - Files: `src/model.rs`, `src/service.rs`, `src/live.rs`, `src/dataset.rs`.
  - Effort: L (split: 1 = stop/response_format/service_tier/reasoning_effort/modalities,
    2 = tools/tool_choice/messages)
  - Acceptance: table-driven test where each invalid payload (`stop` with 5 items, `tool_choice:
    "banana"`, `response_format:{"type":"xml"}`, `service_tier:"turbo"`, `logit_bias:{"1":500}`)
    returns 400 with `error.param` naming the field, and each valid payload returns 200. A
    round-trip test asserts a `tool` message's `tool_call_id` survives into
    `/chat/completions/{id}/messages`.

- [ ] **B4. Request validation for scalars and `messages`**
  - What: `messages` non-empty; ranges for `temperature`, `top_p`, `frequency_penalty`,
    `presence_penalty`, `n`, `top_logprobs`; `top_logprobs` requires `logprobs: true`; unknown
    `model` -> 404 `model_not_found` (config-gated so fixture models keep working).
  - Why: lets a GUI exercise its validation/error UI deterministically.
  - Files: `src/service.rs` (validation module), `src/http/routes.rs`.
  - Effort: S
  - Acceptance: test per rule asserting 400/404 + `error.code`/`error.param`; existing 15 tests
    still pass.

### P2 - streaming fidelity

- [ ] **C1. Chunk sequence fidelity**
  - What: emit a role-only first chunk (`delta:{"role":"assistant","content":""}`), keep content
    chunks content-only, make the finish chunk `delta:{}` + `finish_reason`, stream `refusal` as
    deltas, add `service_tier` to chunks, remove `audio` from the delta (or gate it behind a flag).
  - Why: matches what GUIs were written against; the current final chunk can double-render text.
  - Files: `src/http/routes.rs`, `src/service.rs`, `src/model.rs`.
  - Effort: M
  - Acceptance: raw-SSE test (hyper client, not the SDK) asserting chunk 1 has
    `delta.role == "assistant"` and `delta.content == ""`, no later chunk carries `role`,
    the finish chunk's `delta` is `{}`, and a refusal fixture yields >= 2 refusal deltas.

- [ ] **C2. Stream error termination + keep-alive toggle**
  - What: on internal failure mid-stream, emit one `data: {"error":{...}}` frame then `[DONE]`;
    make the 15s `: ping` keep-alive opt-in via config (default off).
  - Why: a hanging stream turns a GUI test into a timeout instead of an assertion; keep-alive
    comments are not present in real chat-completions traffic.
  - Files: `src/http/routes.rs`, `src/config.rs`, `README.md`.
  - Effort: S
  - Acceptance: test with a forced decode failure asserts the stream ends with an error frame
    followed by `[DONE]`; test asserts no line starting with `:` when keep-alive is disabled.

- [ ] **C3. `n > 1` multi-choice support**
  - What: honour `n` for both non-streamed (`choices[0..n]`) and streamed (one chunk per choice
    index) responses; fixtures reused/rotated deterministically per index.
  - Why: unlocks GUIs with response-variant pickers; also forces the index plumbing that C1/A1 need.
  - Files: `src/service.rs`, `src/http/routes.rs`, `src/model.rs`.
  - Effort: M
  - Acceptance: test asserts `n=3` yields `choices.len()==3` with indices `0,1,2`, and that the
    stream contains deltas for each index with exactly one `finish_reason` per index.

### P3 - nice to have, cheap

- [ ] **D1. `GET /health` + embedded index.html**
  - What: `{"status":"ok","dataset":<path|live>,"models":<count>}`; serve the GUI via `include_str!`
    behind a config flag.
  - Why: healthcheck for docker/CI; removes CWD dependence.
  - Files: `src/http/routes.rs`, `src/config.rs`.
  - Effort: S
  - Acceptance: test asserts `GET /health` -> 200 JSON with `status == "ok"`; a test run from a
    temp CWD still serves `/` with 200.

- [ ] **D2. Realistic `system_fingerprint`, conditional `service_tier`, `X-Request-ID`**
  - What: `fp_` + 10 hex chars derived deterministically from (dataset digest, model); emit
    `service_tier` only when the request set it; echo `request_id` in an `X-Request-ID` header.
  - Why: makes recorded fixtures indistinguishable from live traffic for GUIs that display them.
  - Files: `src/service.rs`, `src/http/routes.rs`.
  - Effort: S
  - Acceptance: test asserts the fingerprint matches `^fp_[0-9a-f]{10}$` and is stable across two
    identical requests; asserts `service_tier` is absent when the request omits it and
    `X-Request-ID` matches the body's `request_id`.

- [ ] **D3. Accept-and-ignore the remaining spec request fields**
  - What: add `n` (see C3), `logprobs`, `top_logprobs`, `prediction`, `web_search_options`,
    `verbosity`, `prompt_cache_key`, `safety_identifier`, `functions`,
    `stream_options.include_obfuscation`; optionally synthesize `logprobs` and
    `message.annotations` from fixtures.
  - Why: prevents silent drops and lets requests round-trip through the recorder unchanged.
  - Files: `src/model.rs`, `src/dataset.rs`, `src/live.rs`.
  - Effort: S (M if `logprobs`/`annotations` are synthesized)
  - Acceptance: test posts a request containing every field above and asserts 200 plus, for a
    stored completion, that the persisted request fields survive a parquet round-trip.

### Cross-plan dependencies

CORS/`OPTIONS`, auth enforcement, and error/latency injection are called out here only where they
change the API shape; their implementation belongs to plans 02/03. A3's envelope and B4's
validation are prerequisites for any injection work, so land them first.
