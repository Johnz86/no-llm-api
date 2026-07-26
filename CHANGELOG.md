# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning is described in
[`docs/versioning.md`](docs/versioning.md).

**Wire behaviour** is called out separately in every entry. A change there alters the
bytes a GUI receives, so it can break a consumer's snapshot even when nothing in the
configuration surface moved.

## [Unreleased]

### Wire behaviour

- `POST /chat/completions` returns the lean `CreateChatCompletionResponse` shape. The
  request echoes it used to carry (`temperature`, `top_p`, `stop`, `stream_options`,
  `request_id`, ...) now appear only on the stored object returned by `GET` and list.
- A response `message.content` always serialises as a string; structured parts move to
  `content_parts` on `GET /chat/completions/{id}/messages`.
- Streamed deltas omit absent members instead of sending `null`, open with a role-only
  frame, close with `delta: {}` plus `finish_reason`, and carry `service_tier`. `audio`
  no longer appears in a delta.
- Tool calls stream as `ChatCompletionMessageToolCallChunk`: `index` on every fragment,
  `id`/`type`/`function.name` on the first only, and `arguments` split across at least
  two fragments that are never empty strings.
- Reasoning replies emit `reasoning_content` deltas before any content, and report
  `usage.completion_tokens_details.reasoning_tokens`.
- `n > 1` returns indexed alternatives and streams them interleaved, with one terminal
  frame per choice.
- Errors always carry all four members of the spec `Error` object, including explicit
  nulls, and unknown routes answer `404` JSON rather than an empty body.
- Identity (`id`, `created`, `request_id`, `system_fingerprint`) is derived from the
  request, so identical requests are byte-identical. `system_fingerprint` is `fp_` plus
  ten hex digits of the plan digest.
- Multi-byte replies stream correctly: a token that ends inside a character no longer
  truncates the stream without a terminator.

### Added

- `GET /models`, `GET /models/{model_id}`, `GET /health`, `GET /ready`, and an optional
  `GET /metrics` with six counters behind `--metrics`.
- The `/_mock` control plane: read, replace, patch and reset the live scenario, inspect
  model profiles, and read a redacted request log.
- Seven behaviour scenarios, per-request `X-Simulate-*` directives, and fault injection
  (`stall`, `drop`, `sse_error`, `http_error`, `slow_then_recover`).
- Optional bearer auth in `off`, `any-bearer` and `keys` modes, with a forbidden-key
  `403` path.
- CORS that mirrors origin and requested headers, and `x-request-id` on every response.
- YAML fixture sets in `fixtures/` with a `fixtures` binary that lints and builds them.
- A distroless container image plus `docker-compose.yml` (Open WebUI) and
  `docker-compose.demo.yml`.
- `x-no-llm-api-version` on every response, and `--print-config`.
- A Playwright browser suite that boots the server and verifies incremental SSE rendering and
  spec-shaped error display in the embedded demo page.
- A dependency-free `health --url` command and container healthcheck suitable for the distroless image.
- A scheduled and manually dispatchable Open WebUI compatibility smoke that verifies model
  discovery and an exact streamed answer through Open WebUI's own API proxy.

### Changed

- The README and technical documentation now describe the completed repository in present tense;
  obsolete roadmap and implementation-planning documents are removed.
- Live proxying is behind the `live` feature. The default build links no HTTP client.
- Request fields are typed: `stop`, `response_format`, `tools`, `tool_choice`, `audio`,
  `modalities`, `reasoning_effort`, `service_tier`, `logit_bias` (`i8`) and `seed`
  (`i64`). Malformed values answer `400` instead of being echoed back.
- Fixture selection is a pure function of the request; the rotation and cursor counters
  are gone.

### Fixed

- Streams no longer abort without a final chunk or `[DONE]` when a reply contains emoji,
  CJK or combining marks.
- Dropping a response now cancels the simulation instead of pacing tokens into a closed
  channel.
- `regenerate_dataset` silently did nothing when the target existed; it now takes
  `--force` and refuses otherwise.
- The embedded demo now checks HTTP status and stream presence, drains the final decoder bytes,
  and displays the API error message instead of attempting to parse error JSON as SSE.
- The Docker build now includes the YAML files embedded by the fixture module.
- The dependency-policy configuration now handles the intentionally private root crate, Arrow's
  CC0 dependency, and Parquet's unavoidable archived `paste` dependency explicitly.
- The real `async-openai` contract now covers streamed refusal deltas and the `content_filter`
  terminal reason, and CI executes the all-feature test suite on both supported operating systems.
