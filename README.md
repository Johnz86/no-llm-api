# No LLM API

A lean mock of the OpenAI Chat Completions API that serves deterministic responses from parquet fixtures, supports live proxying through the real async-openai client, and provides tooling to capture/curate datasets for regression testing.

## Quick Start

```bash
cargo build
cargo run
```

The binary listens on `127.0.0.1:8080` by default. On first launch it materialises `data/conversations.parquet` using bundled fixtures.

## Configuration

Environment variables control runtime behaviour:

| Variable | Description | Default |
| --- | --- | --- |
| `BIND_ADDRESS` | Socket address passed to `TcpListener::bind`. | `127.0.0.1:8080` |
| `TOKENS_PER_SECOND` | Streaming token budget; controls SSE pacing. | `30` |
| `DATASET_SOURCE` | `parquet` (deterministic fixtures) or `live` (proxy real OpenAI/Azure). | `parquet` |
| `DATASET_PATH` | Path to the parquet fixture to load (and optionally append to). | `data/conversations.parquet` |
| `TOKENIZER_MODEL` | Tokenizer preset loaded via `tiktoken-rs` (`cl100k_base`, `o200k_base`, `p50k_base`, `p50k_edit`, `r50k_base`). | `cl100k_base` |
| `LIVE_RECORD` | `true`/`1` to persist live completions into a parquet file. | `false` |
| `LIVE_RECORD_PATH` | Output parquet when recording live sessions (falls back to `DATASET_PATH`). | - |

When `DATASET_SOURCE=live`, supply credentials for either OpenAI (`OPENAI_API_KEY`, optional `OPENAI_API_BASE`, `OPENAI_ORG_ID`, `OPENAI_PROJECT_ID`) or Azure OpenAI (`AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_DEPLOYMENT_NAME`, optional `AZURE_OPENAI_API_VERSION`). The server converts live responses back into the mock schema so existing clients continue to work.

## Working with Datasets

### Regenerate bundled sample

```bash
cargo run --bin regenerate_dataset
```

This recreates `data/conversations.parquet` with the enriched schema (tool calls, refusals, usage, audio metadata, finish reasons, etc.).

### Record new live conversations

```bash
cargo run --bin recorder -- \
  --input recordings.json \
  --output data/recordings.parquet \
  --tokenizer cl100k_base
```

`recordings.json` should be an array of objects shaped like:

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

## Running in Live Mode

1. Export the relevant OpenAI or Azure OpenAI credentials.
2. Launch the server with `DATASET_SOURCE=live`. For example:

   ```bash
   DATASET_SOURCE=live \
   LIVE_RECORD=1 \
   LIVE_RECORD_PATH=data/live.parquet \
   cargo run
   ```

   The service proxies requests to the configured backend, optionally storing each completion to the parquet file for deterministic replays.

## API Surface

The server mirrors the primary Chat Completions endpoints, exposed both at the root and under `/v1`:

- `GET /chat/completions`
- `POST /chat/completions`
- `GET /chat/completions/{completion_id}`
- `POST /chat/completions/{completion_id}`
- `DELETE /chat/completions/{completion_id}`
- `GET /chat/completions/{completion_id}/messages`

Responses include usage data, tool/function call metadata, finish reasons, audio attachments, and refusal text when available.

## Testing

- `cargo test` exercises dataset helpers, service logic, and store behaviour.
- Async-openai compatibility tests are opt-in to avoid external dependencies. Set the environment variable before running:

  ```bash
  ASYNC_OPENAI_COMPAT=1 cargo test tests::async_openai -- --nocapture
  ```

  The suite spins up the Axum server in-process, drives it with async-openai (non-streamed, streamed with usage, and tool-call scenarios), and asserts that the client deserialises responses without warnings.

## Notes

- Streaming completions emit SSE chunks at the configured token rate and always terminate with `[DONE]`. When `stream_options.include_usage=true`, a final usage chunk is delivered.
- Only completions created with `"store": true` are persisted for later retrieval, metadata updates, or message pagination.
- The repository exposes itself as a library (`no_llm_api`) so integration tests and custom binaries can reuse internal modules.
