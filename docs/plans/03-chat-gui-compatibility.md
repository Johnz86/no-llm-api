# 03 - Real-World Chat GUI Compatibility

Target: a chat front-end configured with `baseURL = http://127.0.0.1:8080/v1` + some API key must connect,
list models, stream a reply, cancel it, and see a real error envelope - with no code changes on the GUI side.

## Sources checked

| Source | What was read | Where |
| --- | --- | --- |
| OpenAI OpenAPI spec (local) | `Error` (required: `type`,`message`,`param`,`code`), `ErrorResponse`, `ListModelsResponse`, `Model`, `GET /models`, `GET /models/{model}` | `openapi.yaml:23268`, `:23305`, `:26541`, `:27386`, `:6297`, `:6376` (note: `openapi.documented.yml` is not present in the repo; only `openapi.yaml`) |
| openai-node (master) | `SSEDecoder.decode` ignores lines starting with `:`; terminator check `sse.data.startsWith('[DONE]')`; mid-stream `data.error` -> throws `APIError`; `finally { if (!done) controller.abort() }` | `src/core/streaming.ts` |
| openai-node (master) | `APIError` reads `headers.get('x-request-id')` and `error.code` / `error.param` / `error.type` | `src/core/error.ts` |
| async-openai 0.41.1 (local clone) | `stream()` breaks only on `message.data == "[DONE]"` (exact equality); uses `reqwest-eventsource` (comments ignored); `Model`/`ListModelResponse` shape; usage-chunk semantics | `async-openai/async-openai/src/client.rs` (`stream()`), `.../types/model.rs:5-20`, `.../types/chat.rs:891` |
| async-openai 0.41.1 features | `[features]` = rustls / native-tls / realtime / byot only - the models API is **not** feature-gated, so `client.models().list()` is usable in our compat test | `async-openai/async-openai/Cargo.toml:15-27` |
| axum 0.8.9 | `Sse::into_response` sets only `content-type: text/event-stream` + `cache-control: no-cache`; `KeepAlive::text(x)` -> `Event::default().comment(x)` -> wire bytes `:ping\n\n` | `axum-0.8.9/src/response/sse.rs:94-99`, `:547-551`, `:288-294` |
| tiktoken-rs 0.12.0 | `CoreBPE::decode` = `String::from_utf8(decode_bytes(..))` -> **Err on partial multi-byte token** | `tiktoken-rs-0.12.0/src/patched_tiktoken.rs:205-206` |
| Open WebUI docs (fetched 2026-07-25) | connection verification calls `/models` with `Bearer`; `/v1/models` GET "Recommended", `/v1/chat/completions` POST "Yes"; model-list fetch timeout env `AIOHTTP_CLIENT_TIMEOUT_MODEL_LIST` (default 10 s); base URL must include `/v1`, no trailing slash | docs.openwebui.com/getting-started/quick-start/connect-a-provider/starting-with-openai-compatible/ |
| LibreChat docs (fetched 2026-07-25) | `librechat.yaml` custom endpoint: `baseURL`, `apiKey` (must be non-empty, e.g. `"ollama"`), `models.default[]`, `fetch: true` -> pulls `{baseURL}/models`, `dropParams`, `titleConvo`/`titleModel` (extra completion call per conversation) | librechat.ai/docs/quick_start/custom_endpoints + `ai_endpoints/vllm`, `ai_endpoints/ollama` |
| Jan desktop docs (fetched 2026-07-25) | fetches `{base_url}/models` on save; API key field **required** (placeholder `sk-no-key` for keyless servers); key rotation retries only on `401`/`403`/`429`; "Test keys" = one `/models` call per key; missing `/v1` -> 404 on every request | jan.ai/docs/desktop/remote-models/custom-endpoint |
| Vercel AI SDK docs (fetched 2026-07-25) | `@ai-sdk/openai-compatible`: `baseURL`, `apiKey` -> `Authorization: Bearer`, `includeUsage` -> `stream_options.include_usage`; supports `reasoning content`; no `/models` call | ai-sdk.dev/providers/openai-compatible-providers |
| Vercel AI SDK issues | #6687 empty-string tool `arguments` breaks tool execution and truncates the stream; #12477 missing/partial `usage` crashes retry logic | github.com/vercel/ai/issues/6687, /12477 |
| This repo | router shape, SSE producer, error envelope, request/response structs | `src/http/routes.rs:31-54` (router, `/v1` nest at `:53`), `create_chat_completion()`, `not_found()`/`bad_request()`/`service_error()`, `src/model.rs:78-125` (request), `:243-284` (response), `:331-345` (chunk), `src/service.rs:385-390` (`unix_timestamp`) |

## 1. Per-client wire requirements

| Client | Probes on connect | Auth | Needs CORS | SSE parsing | Usage chunk | Error surfacing |
| --- | --- | --- | --- | --- | --- | --- |
| Open WebUI (server-side aiohttp) | `GET {base}/models` on "Verify Connection" and on model-list refresh; 10 s timeout | `Authorization: Bearer <key>` | No (server-to-server) | own aiohttp SSE reader, relays to browser | passes through | shows HTTP status + body text; non-JSON body = opaque error |
| LibreChat (server-side, openai-node) | `GET {base}/models` only when `fetch: true` | `Authorization: Bearer <key>`; empty key is a config error | No | openai-node `SSEDecoder` | ignored unless configured | openai-node `APIError.message`/`code` |
| Jan (Tauri desktop) | `GET {base}/models` on provider save and per key on "Test keys" | `Authorization: Bearer <key>`, non-empty required | No (native HTTP) | native | ignored | branches on status: 401/403/429 trigger key rotation |
| Vercel AI SDK (`@ai-sdk/openai-compatible`, server or browser) | none - goes straight to `POST {base}/chat/completions` | `Authorization: Bearer <key>` | **Yes** when used in the browser | `eventsource-parser`, zod-validated chunks | `includeUsage: true` -> expects a final `usage` object with complete token fields | zod-parsed `{error:{message,type,param,code}}` |
| openai-js / openai-python direct in a browser page (incl. our `index.html`) | none | `Authorization: Bearer` + `dangerouslyAllowBrowser: true` (JS) | **Yes**, preflighted | openai-node `SSEDecoder` | opt-in | `APIError` with `status`, `code`, `param`, `requestID` |

Consequences for this mock:

1. `GET /v1/models` is the single highest-value missing endpoint - three of five clients will not even reach chat without it.
2. Every client sends `Authorization`, so ignoring the header (current behaviour) is fine for the happy path but leaves the 401 path untestable. Jan additionally needs 401 vs 403 vs 429 to be distinguishable.
3. CORS only matters for the browser cases, but that includes our own `index.html` if it is ever served from a different origin (Vite dev server on `:5173`).
4. LibreChat `titleConvo: true` issues a **second, unrelated completion** ("write a title for this conversation"). With the current round-robin fallback (`AtomicUsize` in `service.rs`) that call consumes a scripted answer and the conversation gets a nonsense title. Deterministic, prompt-keyed selection (plan 02) is a prerequisite for a clean LibreChat demo.

## 2. Browser constraints

### CORS

Current state: no CORS layer at all (`Cargo.toml` has no `tower-http`; `routes.rs:31-54` adds no middleware). Any cross-origin browser client fails at preflight.

Requirements:

| Item | Value | Why |
| --- | --- | --- |
| Preflight trigger | `Authorization` + `Content-Type: application/json` are both non-simple headers, so **every** GUI request is preflighted with `OPTIONS` | without an `OPTIONS` responder the browser never sends the POST |
| `Access-Control-Allow-Origin` | mirror the request `Origin` (default), `*` acceptable when `allow_credentials` is off | `*` + `allow_credentials(true)` is rejected by browsers |
| `Access-Control-Allow-Methods` | `GET, POST, DELETE, OPTIONS` (`HEAD` handled by axum's `get`) | matches implemented routes |
| `Access-Control-Allow-Headers` | mirror `Access-Control-Request-Headers`. openai-js sends `authorization`, `content-type`, plus `x-stainless-arch`, `x-stainless-lang`, `x-stainless-os`, `x-stainless-package-version`, `x-stainless-retry-count`, `x-stainless-runtime`, `x-stainless-runtime-version`, `x-stainless-timeout`, and optionally `openai-organization` / `openai-project` | an explicit allow-list will silently miss a header when the SDK adds one; users otherwise have to null out headers manually (documented Gemini workaround) |
| `Access-Control-Expose-Headers` | `x-request-id`, `retry-after`, `x-ratelimit-*` | `APIError.requestID = headers.get('x-request-id')` (openai-node `error.ts`); unexposed headers read as `null` |
| `Access-Control-Max-Age` | 600 s | avoids a preflight per token-stream request |
| Config | `CORS_ALLOW_ORIGINS` (`*` default, comma list otherwise) | lets a test assert the restricted case |

### SSE

| Property | Current | Verdict |
| --- | --- | --- |
| `content-type: text/event-stream` | set by axum (`sse.rs:97`) | OK |
| `cache-control: no-cache` | set by axum (`sse.rs:98`) | OK |
| `X-Accel-Buffering: no` | missing | add - required when anyone puts nginx in front; harmless otherwise |
| Compression | no `CompressionLayer` today | OK, but must stay that way for `text/event-stream`; gzip buffers whole blocks and kills pacing. Guard with a test, not a comment |
| Keep-alive frames | `KeepAlive::new().interval(15s).text("ping")` -> `:ping\n\n` (`sse.rs:547-551` -> `:288-294`) | **Safe.** It is an SSE comment, not a `data:` frame. openai-node drops it (`if (line.startsWith(':')) return null`), `reqwest-eventsource` drops it, browser `EventSource` drops it, and `index.html` drops it (only handles `data:`). No change needed; do not "fix" it to `data: ping`, which would break every parser. Note it currently never fires: min pacing is 1 token/s (`NonZeroU32`) < 15 s, so it only becomes visible once TTFT/latency injection lands |
| Terminator | `data: [DONE]` | required by openai-node (`startsWith('[DONE]')`) and async-openai (`message.data == "[DONE]"`, exact) - keep the payload exactly `[DONE]`, no trailing text |
| Framing | one `data:` line per event, blank-line separated (axum) | OK |
| UTF-8 boundaries | **broken.** `create_chat_completion()` calls `tokenizer.decode(&buffer)` per token; tiktoken-rs returns `Err` for a partial multi-byte sequence (`patched_tiktoken.rs:205-206`), and the handler does `tracing::error!(...); return;` - the task exits **without** the final chunk and without `[DONE]`. Any emoji / CJK / accented text in a fixture silently truncates the stream (clients see a short message, no error). The subsequent `&decoded[rendered.len()..]` slice is also a panic risk if a lossy prefix shrinks | must fix; see W2 |
| Transfer | HTTP/1.1 chunked, no `content-length` | OK |
| `EventSource` API | cannot be used at all: it is GET-only and cannot set `Authorization`. Note that `GET /v1/chat/completions` exists but is the *list* endpoint | document that GUIs must use `fetch` + `ReadableStream` (as `index.html` does) |

## 3. Auth

Design: two modes, permissive default so nothing regresses.

| Env | Default | Behaviour |
| --- | --- | --- |
| `AUTH_MODE` | `permissive` | header ignored entirely (today's behaviour) |
| `AUTH_MODE=strict` | - | `Authorization: Bearer <key>` must be present and `<key>` must be in `AUTH_KEYS` |
| `AUTH_KEYS` | `sk-mock-key` | comma-separated accepted keys |
| `AUTH_FORBIDDEN_KEYS` | empty | keys that return `403` instead of `401` (exercises Jan's 403 branch) |

Applies to `/chat/completions*` **and** `/models*` (Jan and Open WebUI validate keys via `/models`). `GET /` and `/health` stay open.

Missing header -> `401`:

```json
{
  "error": {
    "message": "You didn't provide an API key. You need to provide your API key in an Authorization header using Bearer auth (i.e. Authorization: Bearer YOUR_KEY).",
    "type": "invalid_request_error",
    "param": null,
    "code": null
  }
}
```

Present but unknown key -> `401`:

```json
{
  "error": {
    "message": "Incorrect API key provided: sk-b***ad. You can find your API key at https://platform.openai.com/account/api-keys.",
    "type": "invalid_request_error",
    "param": null,
    "code": "invalid_api_key"
  }
}
```

Notes:
- All four `Error` members are required by `openapi.yaml:23268-23284` and openai-node reads `code`/`param` off the body (`error.ts`). The current `ErrorBody` (`routes.rs`) emits only `message` + `type` -> `code` and `param` come back `undefined`.
- Also send `WWW-Authenticate: Bearer` on 401 and a `x-request-id` header on every response (openai-node stores it on errors; useful for correlating logs).
- Never echo the full key in the message - mask as above.

## 4. Model catalogue

`GET /v1/models` (and `/models`) response - exactly the spec shape (`openapi.yaml:26541` + `:27386`; matches `async_openai::types::Model`):

```json
{
  "object": "list",
  "data": [
    { "id": "mock-gpt-4o", "object": "model", "created": 1735689600, "owned_by": "no-llm-api" },
    { "id": "mock-reasoner", "object": "model", "created": 1735689600, "owned_by": "no-llm-api" }
  ]
}
```

`GET /v1/models/{model}` returns the single `Model` object, `404` with `code: "model_not_found"` otherwise (spec path `openapi.yaml:6376`).

Configuration - fixture-driven with an env override:

| Source | Precedence | Notes |
| --- | --- | --- |
| `MODELS_PATH` (JSON file, default `data/models.json`) | 1 | full catalogue incl. simulation profile |
| `MODELS` (comma list of ids) | 2 | quick override for CI |
| distinct `model` values in the loaded parquet dataset | 3 (fallback) | guarantees a non-empty list with zero config |

`data/models.json` entry (only the four spec fields are serialised into `/v1/models`; the rest drives simulation and is exposed on a non-spec debug route `GET /_mock/models` so no client ever sees unknown keys):

```json
{
  "id": "mock-gpt-4o",
  "created": 1735689600,
  "owned_by": "no-llm-api",
  "context_window": 128000,
  "max_output_tokens": 16384,
  "capabilities": { "tools": true, "vision": true, "audio": false, "reasoning": false },
  "latency": { "ttft_ms": 350, "tokens_per_second": 45, "jitter_ms": 40 }
}
```

How model id influences simulation:

| Field | Effect |
| --- | --- |
| `context_window` / `max_output_tokens` | prompt token count over budget -> `400` `context_length_exceeded`; output truncation -> `finish_reason: "length"` |
| `capabilities.tools` | request with `tools` against a non-tool model -> `400` `invalid_request_error` (mirrors real providers) |
| `capabilities.vision` | image content parts against a text-only model -> `400` |
| `capabilities.reasoning` | when true, emit `delta.reasoning_content` before `delta.content` (the form `@ai-sdk/openai-compatible` and Open WebUI recognise for DeepSeek-R1-style models) |
| `latency` | per-model TTFT + tokens/s override of the global `TOKENS_PER_SECOND`; makes "fast vs slow model" UX testable |
| unknown id | `404` `model_not_found` in strict mode, fall back to the first catalogue entry in permissive mode (`MODEL_STRICT=false` default, so existing fixtures keep working) |

## 5. Abort / cancel

What happens today (`create_chat_completion()`): the response body is a `ReceiverStream` fed by a detached `tokio::spawn`, channel capacity 32. On browser abort axum drops the body -> receiver drops -> the next `sender.send(...).await` returns `Err` and the content loop `return`s. So cancellation *does* work, but:

- up to 32 already-queued events must be discarded before the producer notices, and the producer sleeps `1/rate` between each iteration -> at `TOKENS_PER_SECOND=1` the task keeps pacing for ~32 s after the user pressed Stop;
- the final-chunk / usage / `[DONE]` sends use `let _ = ...` and cannot observe closure;
- nothing is logged or counted, so a test cannot prove the work stopped.

Target behaviour:

1. Drive the stream from the consumer instead of a detached task (`async_stream::stream!` or a `futures::stream::unfold`) so dropping the body stops the pacing immediately - no channel, no orphan task.
2. If the spawn design is kept, use channel capacity 1 and check `sender.is_closed()` before each `sleep`.
3. Increment a `streams_cancelled` counter and `tracing::debug!` on disconnect, so a test can assert it.
4. No error frame on client abort - openai-node treats abort as a clean exit (`isAbortError(e) -> return`).

## 6. Other things that commonly break mock servers

| # | Issue | Current state | Fix |
| --- | --- | --- | --- |
| 1 | Unknown path returns empty non-JSON 404 | axum default fallback (no `.fallback(..)` in `routes.rs:50-53`) - body empty, no `content-type` | JSON fallback with `code: "unknown_url"`; Jan/Open WebUI surface "404 on every request" when the `/v1` suffix is missing, and a JSON body makes that diagnosable |
| 2 | Malformed JSON body | axum `Json` rejection -> `400`/`422` with a plain-text body | custom `JsonRejection` handler emitting the OpenAI envelope with `type: "invalid_request_error"`, `param` set to the offending field |
| 3 | Missing `OPTIONS` | no route matches -> `405` | `CorsLayer` answers preflight before routing |
| 4 | gzip on SSE | none today | keep it that way; if a `CompressionLayer` is ever added, exclude `text/event-stream` |
| 5 | Wrong `created` unit | `as_secs()` (`service.rs:385-390`) - correct | add a regression assertion (`created` within +/- 60 s of now, 10 digits) |
| 6 | `id` prefix | `chatcmpl-{uuid}` (`service.rs:189`) - correct prefix | keep; assert `^chatcmpl-` in tests |
| 7 | Response echoes non-spec request fields | `ChatCompletionResponse` (`model.rs:243-284`) serialises `response_prefix`, `logit_bias`, `stream_options`, `temperature`, `top_p`, `stop`, `seed`, `tool_choice`, `response_format`, `request_id`, `modalities` - none have `skip_serializing_if`, so they all appear as `null` | add `skip_serializing_if = "Option::is_none"`; keep the fields for the store/parquet round-trip but stop putting them on the wire |
| 8 | `stream: null` | `pub stream: bool` with `#[serde(default)]` (`model.rs:82`) rejects an explicit JSON `null` with a 422 | accept `Option<bool>` and treat `null` as `false` |
| 9 | Tool-call `arguments: ""` | final chunk copies `tool_calls` verbatim from the fixture | never emit an empty `arguments`; use `"{}"` - Vercel AI SDK issue #6687 shows an empty string silently kills tool execution and truncates the stream |
| 10 | Incomplete `usage` | `usage` is non-optional on the response and `null` on non-final chunks (spec-correct per `async-openai/types/chat.rs:891`) | keep; ensure `prompt_tokens + completion_tokens == total_tokens` - AI SDK issue #12477 crashes on partial usage |
| 11 | `index.html` served from CWD | `serve_index()` reads `index.html` relative to the process CWD -> 500 when run from elsewhere | embed with `include_str!` |
| 12 | No `/health` | - | `GET /health` -> `{"status":"ok"}`, no auth; used by Docker/CI readiness before the smoke tests |
| 13 | `index.html` sends no `Authorization` | true | add a key field to the page so it also exercises strict mode |

## 7. Prioritized work items

| P | ID | What | Why for mock-GUI testing | Files | Effort | Acceptance |
| --- | --- | --- | --- | --- | --- | --- |
| P0 | W1 | `GET /models` + `GET /models/{model}` (root and `/v1`), catalogue from `MODELS_PATH` -> `MODELS` -> dataset models | Open WebUI "Verify Connection", LibreChat `fetch: true`, Jan provider save + "Test keys" all call it; without it three of five clients cannot select a model | new `src/http/models.rs`, `src/http/routes.rs`, `src/config.rs`, `data/models.json` | M | `curl -s /v1/models` validates against `ListModelsResponse` (`object == "list"`, every item has `id/object/created/owned_by`); new compat test asserts `async_openai::Client::models().list()` deserialises and returns >= 1 model |
| P0 | W2 | Fix multi-byte token streaming: accumulate `decode_bytes`, emit only complete UTF-8, and on any decode error still emit the final chunk + `[DONE]` | Today a single emoji/CJK char in a fixture truncates the SSE stream with no error and no `[DONE]` - the worst possible failure for a GUI test | `src/http/routes.rs`, `src/tokenizer.rs` | M | test streaming a fixture containing `"caf\u00e9 \U0001F642 \u65E5\u672C\u8A9E"`: concatenated deltas equal the source string byte-for-byte, and the last frame is `data: [DONE]`; a forced decode error still yields a terminating `[DONE]` |
| P0 | W3 | `CorsLayer` (tower-http 0.6, `cors` feature): mirror origin, mirror request headers, methods `GET/POST/DELETE/OPTIONS`, expose `x-request-id`/`retry-after`/`x-ratelimit-*`, `max-age 600`, configurable via `CORS_ALLOW_ORIGINS` | Any browser-hosted GUI (Vercel AI SDK, openai-js, our own page on a Vite origin) fails at preflight today | `Cargo.toml`, `src/http/routes.rs`, `src/config.rs`, `README.md` | S | `OPTIONS /v1/chat/completions` with `Origin` + `Access-Control-Request-Headers: authorization,content-type,x-stainless-lang` returns 2xx with `access-control-allow-origin` and all requested headers echoed in `access-control-allow-headers`; test asserts this |
| P0 | W4 | Spec-shaped error envelope everywhere: `{error:{message,type,param,code}}`, plus JSON 404 fallback and a `JsonRejection` handler | GUIs show raw bodies; an empty 404 or a text 400 is undiagnosable, and openai-node reads `code`/`param` | `src/http/routes.rs` (`ErrorBody`, `.fallback()`), new `src/http/error.rs` | S | `curl /v1/nope` -> 404 `application/json` with `code == "unknown_url"`; `curl -d '{'` -> 400 JSON with `type == "invalid_request_error"`; every error body has all four `Error` keys present (null where unknown) |
| P1 | W5 | Auth middleware: `AUTH_MODE=permissive\|strict`, `AUTH_KEYS`, `AUTH_FORBIDDEN_KEYS`; `WWW-Authenticate` on 401; `x-request-id` on all responses | Lets a GUI test the invalid-key path deliberately; Jan's key-rotation logic branches on 401/403/429 | new `src/http/auth.rs`, `src/config.rs`, `src/http/routes.rs` | S | strict mode: no header -> 401 `code: null`; bad key -> 401 `code: "invalid_api_key"`; forbidden key -> 403; good key -> 200; permissive mode unchanged (existing 15 tests still pass) |
| P1 | W6 | Immediate cancel: consumer-driven stream (or capacity-1 channel + `is_closed()` check), `streams_cancelled` counter, debug log | "Stop generating" must actually stop work; at low token rates the mock keeps pacing for tens of seconds | `src/http/routes.rs` | M | test starts a stream at `TOKENS_PER_SECOND=1`, drops the response after 2 events, and asserts the cancel counter increments within 1.5 s |
| P1 | W7 | Stop serialising `null` non-spec response fields (`skip_serializing_if`), accept `stream: null` | Reduces the chance a strict zod/pydantic client rejects the payload; `stream: null` currently 422s | `src/model.rs` | S | golden-JSON test: serialised non-streamed response contains no `response_prefix`/`logit_bias`/`stream_options` keys; `{"stream":null,...}` returns 200 |
| P1 | W8 | Per-model simulation profile (context window, capabilities, latency incl. TTFT) driven by `data/models.json`; `GET /_mock/models` debug view | Makes "slow model", "tools unsupported", "context exceeded" reproducible from the GUI without code edits | `src/config.rs`, `src/service.rs`, `src/http/models.rs` | M | request with `tools` against a `tools:false` model -> 400; a model with `latency.ttft_ms=1500` shows first byte >= 1.4 s (test measures wall clock); oversized prompt -> 400 `context_length_exceeded` |
| P1 | W9 | `X-Accel-Buffering: no` on SSE + a test asserting SSE is never compressed | Prevents the classic "works with curl, buffers behind nginx" report and locks out a future `CompressionLayer` mistake | `src/http/routes.rs` | S | streaming response headers contain `x-accel-buffering: no` and no `content-encoding` even when the request sends `Accept-Encoding: gzip` |
| P2 | W10 | `GET /health` (unauthenticated) + embed `index.html` via `include_str!` | CI/Docker readiness probe; running the binary from another CWD currently 500s on `/` | `src/http/routes.rs` | S | `curl /health` -> 200 `{"status":"ok"}`; `GET /` returns HTML when the process is started from a different working directory |
| P2 | W11 | `reasoning_content` deltas for `capabilities.reasoning` models | Open WebUI and the AI SDK both render a "thinking" block from it; needed to test that UI | `src/model.rs`, `src/http/routes.rs`, `src/dataset.rs` | M | streaming a reasoning model yields >= 1 chunk with `choices[0].delta.reasoning_content` before the first `delta.content`, and `[DONE]` still terminates |
| P2 | W12 | Never emit empty tool-call `arguments`; stream them incrementally | AI SDK #6687: `arguments: ""` silently breaks tool execution | `src/http/routes.rs`, `src/dataset.rs` | M | tool-call fixture streams >= 2 `delta.tool_calls` frames whose concatenated `arguments` parse as JSON; no frame carries `arguments: ""` |
| P2 | W13 | `429` + `Retry-After` injection (`INJECT_RATE_LIMIT_EVERY_N`) | Jan retries only on 401/403/429; GUIs need a way to show rate-limit UI | `src/http/auth.rs` or a small middleware | S | every Nth request returns 429 with `retry-after: 1` and `code: "rate_limit_exceeded"` |

## 8. Manual smoke test

Assumes `cargo run` with `BIND_ADDRESS=127.0.0.1:8080`. On Windows use `curl.exe` from Git Bash / WSL, or replace single quotes with double quotes and `\` with backtick line continuations in PowerShell.

```bash
BASE=http://127.0.0.1:8080/v1
KEY=sk-mock-key

# 1. health + model discovery (what Open WebUI / LibreChat / Jan do first)
curl -s http://127.0.0.1:8080/health
curl -s -H "Authorization: Bearer $KEY" $BASE/models | jq '.object, (.data | length), .data[0]'
# expect: "list", >=1, {id, object:"model", created:<10-digit>, owned_by}

# 2. CORS preflight with the headers openai-js actually sends
curl -s -i -X OPTIONS $BASE/chat/completions \
  -H 'Origin: http://localhost:5173' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: authorization,content-type,x-stainless-lang,x-stainless-retry-count'
# expect: 200/204 + access-control-allow-origin + access-control-allow-headers covering all four

# 3. non-streamed completion
curl -s $BASE/chat/completions -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"model":"mock-gpt-4o","messages":[{"role":"user","content":"hello"}]}' \
  | jq '.id, .object, .created, .choices[0].finish_reason, .usage'
# expect: id matches ^chatcmpl-, object=="chat.completion", created ~= now, usage totals consistent

# 4. streamed completion - must be unbuffered, uncompressed, and end with [DONE]
curl -sN --no-buffer -D - $BASE/chat/completions \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' -H 'Accept-Encoding: gzip' \
  -d '{"model":"mock-gpt-4o","messages":[{"role":"user","content":"hello"}],"stream":true,
       "stream_options":{"include_usage":true}}' | tail -5
# expect headers: content-type: text/event-stream, cache-control: no-cache, NO content-encoding
# expect body: penultimate frames carry finish_reason then usage; last frame is exactly "data: [DONE]"

# 5. multi-byte regression (W2) - currently truncates
curl -sN $BASE/chat/completions -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"model":"mock-gpt-4o","messages":[{"role":"user","content":"emoji please"}],"stream":true}' \
  | grep -c '^data: \[DONE\]$'
# expect: 1  (fixture reply must contain "cafe\u0301 emoji CJK" style content)

# 6. error envelopes
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' $BASE/nope
curl -s $BASE/chat/completions -H 'Content-Type: application/json' -d '{'  | jq .error
curl -s -i $BASE/models -H 'Authorization: Bearer sk-wrong' | head -1   # strict mode only -> 401
```

Minimal browser check (open DevTools, run from an origin other than the server, e.g. a Vite page on `:5173`):

```js
const res = await fetch('http://127.0.0.1:8080/v1/chat/completions', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json', Authorization: 'Bearer sk-mock-key' },
  body: JSON.stringify({
    model: 'mock-gpt-4o',
    messages: [{ role: 'user', content: 'hello' }],
    stream: true,
  }),
  signal: AbortSignal.timeout(5000), // proves cancel does not hang the server
});
if (!res.ok) throw new Error(JSON.stringify(await res.json())); // must be {error:{message,type,param,code}}

const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
let buf = '', out = '', done = false;
while (!done) {
  const { value, done: fin } = await reader.read();
  if (fin) break;
  buf += value;
  const frames = buf.split('\n\n'); buf = frames.pop();
  for (const frame of frames) {
    for (const line of frame.split('\n')) {
      if (line.startsWith(':')) continue;            // :ping keep-alive - must be tolerated
      if (!line.startsWith('data:')) continue;
      const data = line.slice(5).trim();
      if (data === '[DONE]') { done = true; continue; }
      out += JSON.parse(data).choices?.[0]?.delta?.content ?? '';
    }
  }
}
console.assert(done, 'stream ended without [DONE]');
console.log(out);
```

Must hold: no CORS error in the console, `done === true`, and `out` equals the fixture text exactly (including any non-ASCII characters).

`EventSource` is deliberately not used - it is GET-only and cannot send `Authorization`; every real GUI uses `fetch` + `ReadableStream` here, and `index.html` already does.
