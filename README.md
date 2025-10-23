# No LLM API

## Overview
- Mock implementation of key OpenAI Chat Completions endpoints.
- Serves deterministic assistant messages loaded from a parquet dataset.
- Streams responses via SSE with a configurable tokens-per-second rate.

## Getting Started
- `cargo build` to compile the service.
- `cargo run` to start the HTTP server (defaults to `127.0.0.1:8080`).

## Configuration
- `BIND_ADDRESS`: address passed to `TcpListener::bind` (default `127.0.0.1:8080`).
- `TOKENS_PER_SECOND`: streaming speed budget (default `30`).
- `DATASET_PATH`: parquet file path for scripted conversations (default `data/conversations.parquet`). Missing files are generated from bundled fixtures on startup.

## API Surface
- `GET /v1/chat/completions`
- `POST /v1/chat/completions`
- `GET /v1/chat/completions/{completion_id}`
- `POST /v1/chat/completions/{completion_id}`
- `DELETE /v1/chat/completions/{completion_id}`
- `GET /v1/chat/completions/{completion_id}/messages`

The same routes are also exposed without the `/v1` prefix for direct compatibility with the provided OpenAPI document.

## Testing
- `cargo test` executes dataset and service integration checks.

## Notes
- Streaming completions emit SSE chunks at the configured rate and automatically end with `[DONE]`.
- Only completions created with `"store": true` are persisted for retrieval, update, deletion, and message history queries.
