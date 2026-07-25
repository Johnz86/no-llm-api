# Upcoming Task: Rich Dataset & E2E Validation

## Goal
Bring the mock `no-llm-api` server to feature parity with the async-openai client by enriching stored conversations, adding a recording pipeline, and validating behaviour end-to-end via async-openai.

## Deliverables
- **Dataset schema upgrade**: extend parquet storage to capture optional fields such as finish reasons, refusals, tool/function calls, audio metadata, usage breakdowns, and any other surfaced response data.
- **Recording workflow**: provide a CLI/feature flag that can invoke real OpenAI/Azure endpoints (keys via `.env`) using async-openai, capture responses, and persist them as deterministic parquet fixtures.
- **Configurable runtime**: add settings to switch between prerecorded parquet fixtures and live proxy/record modes, ensuring defaults remain deterministic.
- **Field population**: ensure service/store paths read/write the richer fields with sensible fallbacks so async-openai models deserialize without loss.
- **End-to-end tests**: implement integration tests that boot the Axum server, target it with async-openai (streaming & non-streaming scenarios, including tool calls), and assert full compatibility.
- **Docs & samples**: document the workflow in `README.md` and ship at least one curated parquet fixture demonstrating the expanded capabilities.

## Notes
- `tiktoken-rs` already depends on async-openai, so we can reuse the client without extra dependencies.
- Keep fixtures redact-safe and reproducible; favour deterministic scripts for CI.
- Gate live-network tests behind an explicit environment flag so CI remains offline.
