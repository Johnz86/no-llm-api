# No LLM API

`no-llm-api` is a deterministic, offline test double for OpenAI Chat Completions and the text-first
Responses API. It gives applications realistic HTTP responses and paced Chat SSE streams without
loading a model, making a network call, or consuming API credit. Outputs come from versioned
fixtures and are stable across processes, machines, and concurrent requests.

The repository includes the reusable Rust library, the server CLI, fixture authoring tools, an
opt-in live recorder, a browser test page, container packaging, and an Open WebUI compatibility
stack. The default binary contains no upstream HTTP client or TLS stack.

## Quick start

```bash
cargo build
cargo run --bin no-llm-api
```

The binary listens on `127.0.0.1:8080` by default. On first launch it materialises
`data/conversations.parquet` from the bundled fixtures. Send a normal OpenAI-compatible request:

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"mock-gpt-4o","messages":[{"role":"user","content":"Summarize the sprint update."}]}'
```

Add `"stream":true` and use `curl -N` to observe the paced SSE transcript.

The experimental non-streamed Responses surface uses the same semantic fixtures:

```bash
curl http://127.0.0.1:8080/v1/responses \
  -H 'Content-Type: application/json' \
  -d '{"model":"mock-reasoner","input":"Which release should ship?","reasoning":{"effort":"high","summary":"auto"}}'
```

It supports text, public reasoning summaries, refusals, and validated structured output. Responses
streaming, persistence, continuation, output limits, and tools return explicit unsupported-parameter
errors until their dedicated implementation slices land.

## Request behaviour

Every request follows the same deterministic pipeline:

1. The server validates the OpenAI-shaped request and selects a fixture from the requested model and
   normalized conversation. Case and whitespace do not affect matching.
2. Multi-turn conversation-prefix matches take precedence over suffix matches and exact last-user
   matches. An unknown prompt falls back to a stable fixture selected with FNV-1a.
3. The selected fixture supplies content, refusals, reasoning, tool or function calls, audio
   metadata, and finish reasons. The configured tokenizer calculates usage and stream pieces.
   Request limits such as `n` and `max_completion_tokens` shape the returned choices without
   introducing nondeterminism.
4. A non-streamed request receives one JSON completion. A streamed request receives OpenAI-shaped
   SSE deltas, one terminal choice frame per choice, an optional final usage frame, and `[DONE]`.
5. Scenario and per-request directives control timing and faults. They never change which fixture is
   selected.

Derived identity mode makes completion ids, timestamps, payload request ids, fingerprints, response
bodies, and complete SSE transcripts reproducible. There are no counters, fixture rotation, or
wall-clock inputs in this mode. The `x-simulate-match` response header reports the matching rung used
for a request.

## Configuration

Every setting is a flag with an environment fallback. `no-llm-api --help` is the full reference and
`no-llm-api --print-config` prints the resolved configuration as JSON without starting the server.

| Flag | Variable | Description | Default |
| --- | --- | --- | --- |
| `--bind-address` | `BIND_ADDRESS` | Socket address passed to `TcpListener::bind`. | `127.0.0.1:8080` |
| `--tokens-per-second` | `TOKENS_PER_SECOND` | Streaming token budget; controls SSE pacing. | `30` |
| `--dataset-source` | `DATASET_SOURCE` | `parquet` (deterministic fixtures) or `live` (proxy real OpenAI/Azure). | `parquet` |
| `--dataset-path` | `DATASET_PATH` | Path to the parquet fixture to load (and optionally append to). | `data/conversations.parquet` |
| `--tokenizer` | `TOKENIZER_MODEL` | Tokenizer preset loaded via `tiktoken-rs` (`cl100k-base`, `o200k-base`, `p50k-base`, `p50k-edit`, `r50k-base`). | `cl100k-base` |
| `--models-path` | `MODELS_PATH` | JSON catalogue backing `GET /models`. Accepts a bare array or `{ "data": [...] }`. | `data/models.json` when present |
| `--models` | `MODELS` | Comma-separated model ids; used when no catalogue file is available. | built-in list |
| `--cors-origins` | `NO_LLM_CORS_ORIGINS` | `*` mirrors the request `Origin`; otherwise a comma-separated allow-list. | `*` |
| `--scenario` | `NO_LLM_SCENARIO` | Behaviour profile: a built-in name or a path to a YAML file. | `default` |
| `--seed` | `NO_LLM_SEED` | Seed for every simulated random decision. | `0` |
| `--identity-mode` | `NO_LLM_IDENTITY_MODE` | `derived` (reproducible ids and `created`) or `clock`. | `derived` |
| `--auth-mode` | `NO_LLM_AUTH_MODE` | `off`, `any-bearer`, or `keys`. | `off` |
| `--auth-keys` | `NO_LLM_AUTH_KEYS` | Comma-separated accepted bearer keys for `keys` mode. | - |
| `--auth-forbidden-keys` | `NO_LLM_AUTH_FORBIDDEN_KEYS` | Keys that answer `403` instead of `401`. | - |
| `--control-plane` | `NO_LLM_CONTROL_PLANE` | Expose `/_mock`. Defaults to on for loopback binds only. | loopback only |
| `--control-token` | `NO_LLM_CONTROL_TOKEN` | Bearer token the control plane requires when set. | - |
| `--live-record` | `LIVE_RECORD` | Persist live completions into a parquet file. | `false` |
| `--live-record-path` | `LIVE_RECORD_PATH` | Output parquet when recording live sessions (falls back to `DATASET_PATH`). | - |
| `--metrics` | `NO_LLM_METRICS` | Expose six Prometheus counters at `GET /metrics`. | `false` |
| `--print-config` | - | Print the resolved configuration as JSON and exit. | - |

## Scenarios

A scenario is a behaviour profile: pacing and failure injection. Seven are built in, and
`--scenario ./my-profile.yaml` loads one from disk.

| Name | Behaviour |
| --- | --- |
| `default` | Deterministic pacing at the configured rate, no faults. |
| `fast` | As quick as the transport allows. |
| `slow` | 1.2 s to first token, 4 tokens/s - spinners and cancel buttons become visible. |
| `realistic` | Short think, jittery stream, small opening burst. |
| `flaky` | Half of all streams die mid-flight with an error frame, then `[DONE]`. |
| `rate-limited` | Every request answers `429` with `Retry-After`. |
| `outage` | Every request answers `503`. |

Per-request overrides beat the scenario, which makes parallel tests safe. Send an `X-Simulate-*`
header or an `x_simulate` object in the request body:

```bash
curl -N http://127.0.0.1:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -H 'X-Simulate-Fault: http_error;status=429;retry_after=3' \
  -d '{"model":"mock-gpt-4o","messages":[{"role":"user","content":"hi"}]}'
```

Recognised directives: `fault` (`stall`, `drop`, `sse_error`, `http_error`, `slow_then_recover`)
with `status`, `retry_after`, `after_ms`, `after_frames`, `rate`, and optional `stage`
(`reasoning`, `output`, or `terminal`); plus `ttft_ms`,
`tokens_per_second`, `jitter_ms`, `chunk_tokens` and `burst_frames`.

Schema-v2 text, reasoning, and structured-output cases are selected explicitly with
`X-Simulate-Case: fixture-id/case-id` and optional `X-Simulate-Variant`, or the equivalent `case`
and `variant` body members. The body wins over headers. Successful responses expose the dataset
revision, selected case and variant, match kind, and plan digest in `X-Simulate-*` headers. Requests
without a semantic selector continue to use the legacy Chat matching ladder.

## Control plane

`/_mock` is on by default for loopback binds only, and requires a bearer token when
`--control-token` is set.

| Route | Purpose |
| --- | --- |
| `GET /_mock/scenario` | The live profile. |
| `PUT /_mock/scenario` | Replace it wholesale. |
| `PATCH /_mock/scenario` | Merge a partial profile, member by member. |
| `POST /_mock/reset` | Restore the profile the process started with and clear the log. |
| `GET /_mock/models` | Catalogue including simulation profiles. |
| `GET /_mock/requests` | The last 100 requests, with credentials redacted. |

## Health, metrics, and logs

`GET /health` reports process liveness without reading the dataset. `GET /ready` reports whether the
server has fixtures available and includes the active version, scenario, tokenizer, model count, and
stream rate. Both routes bypass API authentication and simulated faults. The `health --url` CLI
subcommand probes readiness without requiring `curl`.

Set `NO_LLM_METRICS=true` to expose six Prometheus counters at `GET /metrics`: requests,
completions, streams, cancelled streams, injected faults, and error responses. Compact structured
request logs use `RUST_LOG` filtering and never record credential values.

## Docker

The image is distroless and runs as `nonroot`, with the sample dataset baked in so it needs no
writable volume:

```bash
docker build -t no-llm-api:local .
docker run --rm --read-only -p 8080:8080 no-llm-api:local
```

`docker compose up` starts the mock plus Open WebUI on <http://127.0.0.1:3000> pointed at it;
`docker compose -f docker-compose.demo.yml up` starts the mock alone with the bundled page.

Because the container binds `0.0.0.0`, `/_mock` stays off unless you set `NO_LLM_CONTROL_PLANE=true`,
and a token is expected when you do.

## Live mode is opt-in at build time

The default build links no HTTP client at all - no `reqwest`, no TLS stack - so an offline mock
cannot reach a paid API even if credentials are present in its environment. Proxying a real backend
requires the `live` feature:

```bash
DATASET_SOURCE=live cargo run --features live --bin no-llm-api
cargo run --features live --bin recorder -- --input recordings.json
```

Supply credentials for either OpenAI (`OPENAI_API_KEY`, optional `OPENAI_API_BASE`, `OPENAI_ORG_ID`,
`OPENAI_PROJECT_ID`) or Azure OpenAI (`AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_ENDPOINT`,
`AZURE_OPENAI_DEPLOYMENT_NAME`, optional `AZURE_OPENAI_API_VERSION`). Azure configuration takes
precedence when its endpoint is present. Live responses are converted back into the same mock schema
used by fixture mode.

Without it, `DATASET_SOURCE=live` exits with an explanatory error rather than starting a server that
cannot proxy.

Live mode is bounded on purpose: 60 s per request, at most 4 concurrent upstream calls, and at most
500 upstream calls per process. Credentials are wrapped so they cannot be logged or serialised, and
recorded rows pass a redaction sweep before they reach parquet, because model output can quote a key
a user pasted into a prompt. Recorded datasets are gitignored (`/data/live/`, `/data/live.parquet`,
`/data/recordings.parquet`).

## Model catalogue and per-model behaviour

`MODELS_PATH` accepts a bare array or `{ "data": [...] }`. Only the four spec fields reach a client;
the rest drives simulation and is visible on `GET /_mock/models`:

```json
[
  {
    "id": "mock-reasoner",
    "created": 1735689600,
    "owned_by": "no-llm-api",
    "context_window": 128000,
    "capabilities": {
      "tools": true,
      "vision": false,
      "audio": false,
      "reasoning": true,
      "reasoning_efforts": ["low", "medium", "high"],
      "structured_output": true
    },
    "latency": { "ttft_ms": 800, "tokens_per_second": 12, "jitter_ms": 60 }
  }
]
```

A model's `latency` block paces its requests whenever the active scenario expresses no opinion, so
"the reasoning model is slower" is true without a restart. An explicit scenario, and then a
per-request directive, still win.

## Reasoning content

A fixture reply that starts with `<think>...</think>` is split: the tag content becomes
`reasoning_content` on the message and streams as `reasoning_content` deltas *before* any visible
content, and its tokens are reported in `usage.completion_tokens_details.reasoning_tokens`. The
thinking tag never leaks into `content`.

## Working with datasets

Fixtures are authored as YAML in `fixtures/`, one file per conversation, and compiled to parquet:

```bash
cargo run --bin fixtures -- lint                       # check every set
cargo run --bin fixtures -- build --force               # rewrite data/conversations.parquet
cargo run --bin fixtures -- build --input ./my-fixtures --output data/mine.parquet
```

A set looks like this; block scalars are what make markdown replies comfortable to write:

```yaml
id: conv-markdown
description: "Markdown a GUI must render."
turns:
  - role: user
    content: Show me a markdown-heavy answer.
  - role: assistant
    finish_reason: stop
    content: |
      ## Heading
      1. item
```

The linter rejects a set that does not end with an assistant reply, an assistant turn with no
payload or no `finish_reason`, a non-assistant turn that declares one, tool-call arguments that are
not valid JSON, and any unknown field - a typo fails the build instead of being silently dropped.
Twelve built-in sets cover markdown, truncation, refusal with `content_filter`, JSON output,
reasoning traces, an empty reply, audio metadata, tool and function calls, multi-byte text and a
~2k-token reply. They are embedded in the binary, so a fresh clone needs no files.

### Regenerate bundled sample

```bash
cargo run --bin regenerate_dataset -- --force
```

This recreates `data/conversations.parquet` with the enriched schema (tool calls, refusals, usage, audio metadata, finish reasons, etc.).

### Record new live conversations

```bash
cargo run --features live --bin recorder -- \
  --input recordings.json \
  --output data/recordings.parquet \
  --tokenizer cl100k_base
```

`recordings.json` is an array of objects shaped like:

```jsonc
[
  {
    "conversation_id": "conv-live-001",
    "request": {
      "model": "gpt-4o",
      "messages": [
        { "role": "user", "content": { "text": "Summarise the sprint update." } }
      ]
    }
  }
]
```

The CLI replays each request via async-openai, serialises the full response (including streaming metadata) into the expanded parquet rows, and appends them to the target file. It honours the same credential environment variables as live mode.

## API surface

The server mirrors the primary Chat Completions endpoints, exposed both at the root and under `/v1`:

- `GET /chat/completions`
- `POST /chat/completions`
- `GET /chat/completions/{completion_id}`
- `POST /chat/completions/{completion_id}`
- `DELETE /chat/completions/{completion_id}`
- `GET /chat/completions/{completion_id}/messages`
- `GET /models`
- `GET /models/{model_id}`
- `GET /health` and `GET /ready` (unversioned)

`GET /` is also unversioned and serves the bundled test page. The page is embedded in the binary, so
it works from any working directory.

Responses include usage data, tool and function call metadata, reasoning content, finish reasons,
audio attachments, and refusal text when the fixture supplies them.

Errors always use the spec envelope with all four members present, including explicit nulls, because
clients read `code` and `param` straight off the body:

```json
{ "error": { "message": "...", "type": "invalid_request_error", "param": null, "code": null } }
```

Every response carries `x-request-id`; a caller-supplied value is echoed back. CORS mirrors the request
origin and the requested headers, so SDK-specific headers never need an allow-list update.

Only completions created with `"store": true` enter the in-memory completion store. Stored records
support retrieval, metadata replacement, deletion, filtering, cursor pagination, and message
pagination for the lifetime of the process.

## Testing

- `cargo test` exercises dataset helpers, service logic, store behaviour, the HTTP surface
  (`tests/http_api.rs`), the SSE transcript contract (`tests/sse.rs`) and the async-openai client
  contract (`tests/async_openai.rs`). No environment variables and no upstream network access are
  required.
- `tests/support/sse.rs` is the shared transcript parser and assertion harness: exactly one trailing
  `[DONE]`, one `finish_reason` on the last chunk carrying a choice, stable chunk ids, usage-frame
  position, and median inter-frame pacing.
- `npm --prefix e2e test` starts the server, drives the embedded page in Chromium, and exercises
  non-streamed Responses with the locked official JavaScript client. It verifies incremental
  rendering, exact fixture text, browser-console hygiene, API errors, reasoning summaries, and
  structured output. Install the browser once with `npx --prefix e2e playwright install chromium`.
- `no-llm-api health --url http://127.0.0.1:8080/ready` is the dependency-free readiness probe
  used by the distroless image, where `curl` is unavailable.
- `bash scripts/smoke-open-webui.sh` boots the real Compose stack, verifies that Open WebUI's model
  selector discovers the mock, and incrementally renders the exact fixture answer. The same
  check runs weekly and on demand in the Open WebUI compatibility workflow because its image is
  too large for every pull-request job.

## Documentation

- [`docs/README.md`](docs/README.md) indexes the maintained technical documentation.
- [`docs/spec/chat-completions-scope.md`](docs/spec/chat-completions-scope.md) defines the implemented
  Chat Completions contract.
- [`docs/spec/upstream-openapi.md`](docs/spec/upstream-openapi.md) explains the tracked OpenAPI extract
  and its refresh procedure.
- [`docs/versioning.md`](docs/versioning.md) defines compatibility and the release process.
- [`CHANGELOG.md`](CHANGELOG.md) records wire-behaviour and repository changes.
- [`AGENTS.md`](AGENTS.md) contains contributor and coding-agent conventions.

The crate also exposes the `no_llm_api` library so integration tests and custom binaries can reuse
the same configuration, dataset, simulation, service, and router implementations as the server.
