# Implemented Chat Completions contract

This document defines the Chat Completions behavior implemented by `no-llm-api`. It distills the
relevant OpenAI schemas into the contract exercised by this repository. Everything applies to both
the root routes (`/chat/...`) and their OpenAI-compatible aliases under `/v1/chat/...`.

## Endpoints

- `GET /chat/completions`
  - Lists stored completions (only requests created with `store: true`).
  - Query parameters: `model` (string), `metadata[key]=value` filters,
    `after` (cursor id), `limit` (default 20, cap 100), `order` (`asc`|`desc`,
    default `asc`).
  - Response body: `ChatCompletionList`.

- `POST /chat/completions`
  - Creates a completion from a `ChatCompletionRequest`.
  - Honors `stream: true` + optional `stream_options.include_usage` by sending
    `text/event-stream` chunks shaped like `ChatCompletionChunk`, followed by
    a terminal `[DONE]`.
  - When `store` is truthy the completion and message history become eligible
    for listing, retrieval, updates, deletion, and message pagination.
  - Synchronous response body: `ChatCompletion`.

- `GET /chat/completions/{completion_id}`
  - Returns the persisted `ChatCompletion` for the id, or a 404 error payload.

- `POST /chat/completions/{completion_id}`
  - Updates metadata via `{"metadata": { ... }}`; response mirrors the updated
    `ChatCompletion`.

- `DELETE /chat/completions/{completion_id}`
  - Deletes the stored record and returns `ChatCompletionDeleted`.

- `GET /chat/completions/{completion_id}/messages`
  - Query parameters: `after`, `limit` (default 20, cap 100), `order`
    (`asc`|`desc`, default `asc`).
  - Response body: `ChatCompletionMessageList`.

## Payload Snapshots

### ChatCompletionRequest

- `model` (string, required) — accepts OpenAI model ids.
- `messages` (array, required) — list of `ChatCompletionRequestMessage`.
- `modalities` (string array, optional) — e.g. `["text"]`, `["text","audio"]`.
- `max_completion_tokens` (integer, optional) — use as upper bound on generated
  tokens. Accept legacy alias `max_tokens`.
- `temperature`, `top_p` (numbers, nullable) — sampling controls.
- `frequency_penalty`, `presence_penalty` (numbers, nullable) — repetition
  modifiers.
- `stop` (string or array, optional) — stop sequences.
- `seed` (integer, optional) — deterministic sampling hint.
- `metadata` (object, optional) — arbitrary key/value pairs persisted when
  `store` is true.
- `response_format` (object, optional) — text, json schema, or json object
  requests.
- `tools` (array, optional) + `tool_choice` (object/string, optional) +
  `parallel_tool_calls` (boolean, optional) — function calling contract.
- `stream` (boolean, optional, default false).
- `stream_options` (object, optional) — currently `{"include_usage": true}`
  is the only documented flag.
- `store` (boolean, optional, default false).
- Additional passthrough fields: `user`, `service_tier`, `reasoning_effort`,
  `audio`, `logit_bias`, `response_prefix`, and deprecated `function_call` are
  accepted but may be ignored by the mock.

### ChatCompletionRequestMessage

- `role` — one of `system`, `developer`, `user`, `assistant`, `tool`,
  `function`.
- `content`
  - Text (string), or
  - Array of typed parts (e.g. `{type:"text",text:"..."}`,
    `{type:"image_url", image_url:{url:"..."}}`,
    `{type:"input_audio", input_audio:{data:"...",format:"mp3"}}`,
    `{type:"file", file:{file_id:"..."}}`).
- Optional `name`, `tool_calls`, `function_call` fields align with spec.

### ChatCompletion

- `id` (`chatcmpl-*`), `object` (`chat.completion`), `created` (unix epoch),
  `model`, `system_fingerprint`, `service_tier`, `request_id` (nullable),
  `metadata` (object or `{}`).
- `usage` — see below.
- `choices` — array of indexed `ChatCompletionChoice` values, with `n` deterministic alternatives
  when requested.
- Optional passthrough echoes: `top_p`, `temperature`, `frequency_penalty`,
  `presence_penalty`, `seed`, `response_format`, `tool_choice`, `tools`,
  `parallel_tool_calls`, `reasoning`, `audio`.

### ChatCompletionChoice

- `index` (integer), `finish_reason` (`stop`, `length`, `tool_calls`,
  `content_filter`, `function_call`), `message` (`ChatCompletionResponseMessage`),
  optional `logprobs`.

### ChatCompletionResponseMessage

- `role` (`assistant`, `tool`, etc.), `content` (string or `null` when tool
  calls are returned), optional `refusal`, `tool_calls`, `function_call`,
  `audio`.

### ChatCompletionUsage

- `prompt_tokens`, `completion_tokens`, `total_tokens` (integers).
- With `stream_options.include_usage`, the final chunk surfaces this payload.

### ChatCompletionChunk (Streaming)

- Same `id`, `object: "chat.completion.chunk"`, `created`, `model`.
- `choices`: each has `index`, `delta` (`ChatCompletionChunkDelta`),
  optional `finish_reason`, optional `logprobs`.
- `delta` includes incremental `role`, `content`, `reasoning_content`, `refusal`, `tool_calls`, and
  `function_call` members when relevant.
- The final SSE choice frame is
  `data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}` (with the selected finish
  reason), followed by `data: [DONE]`.

### ChatCompletionList

- `object: "list"`, `data` (array of `ChatCompletion`), `first_id`, `last_id`,
  `has_more`.

### ChatCompletionMessageList & StoredMessage

- `object: "list"`, `data` (array of stored messages), `first_id`, `last_id`,
  `has_more`.
- Each stored message: `id`, `role`, optional `content`, optional `name`.

### ChatCompletionDeleted

- `id`, `object: "chat.completion.deleted"`, `deleted: true`.

### Error Envelope

- Every error returns `{"error":{"message":"...","type":"...","param":null,"code":null}}` with
  all four members present and an appropriate HTTP status.

## Tracking & Persistence Rules

- Only completions with `store: true` are persisted.
- Metadata updates fully replace the stored map (spec behavior).
- Listing and message pagination use cursor-based slicing: drop entries up to
  and including `after`, then return up to `limit` items and signal `has_more`
  when more remain.
- Sorting supports ascending (oldest first) and descending (newest first)
  orderings.
- `model` and `metadata[key]=value` filters apply before pagination.
- `request_id`, `seed`, and other tuning values returned from creation remain stable when fetching
  the same completion later.

## Runtime guarantees

- Fixture selection is a pure function of the model and normalized conversation. Identical requests
  produce identical response bodies and SSE transcripts in derived identity mode, including under
  concurrency.
- Token pacing changes delivery time without changing the sequence or bytes of the reconstructed
  response.
- Unknown request members remain forward-compatible and are ignored. Known members are strongly
  typed and validated; invalid types and documented range violations return the standard error
  envelope.
- Every response exposes `x-request-id` and `x-no-llm-api-version`. Completion responses also expose
  `x-simulate-match` for fixture-selection diagnostics.
- The real `async-openai` 0.41.1 client exercises non-streamed, streamed, usage, tool-call, refusal,
  and model-discovery contracts in the default test suite.
