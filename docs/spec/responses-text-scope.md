# Responses text contract freeze

## Status

This document freezes the Responses text contract. The repository serves non-streamed objects and
typed SSE events at `POST /v1/responses`; the executable corpus in
[`responses-text-contract/`](responses-text-contract/) defines both boundaries.

The locked `openai` 6.49.0 JavaScript client exercises response objects, strict structured output,
and reasoning-first streamed events in `e2e/responses-client.spec.ts`. The response includes the
SDK-required `output_text` aggregation and standard response members in addition to the frozen
minimal corpus.

The contract is pinned to the OpenAI OpenAPI document at commit
`5c044be3bf3a42854e99e34616564eeb2124a317` from 2026-07-23, full-document SHA-256
`b58d6cd94c881bdfd6a940bdc4db009e2c9b455accf8fd6a8b712458bc30c0da`. The first official-client
gate targets the OpenAI JavaScript/TypeScript SDK package `openai` version `6.49.0`. Updating either
pin requires reviewing this scope, every corpus case, and the contract tests in the same change.

The upstream [migration guide](https://developers.openai.com/api/docs/guides/migrate-to-responses)
uses output items rather than Chat choices and recommends Responses for new projects. The
[streaming guide](https://developers.openai.com/api/docs/guides/streaming-responses) defines typed
events. The [reasoning guide](https://developers.openai.com/api/docs/guides/reasoning) exposes
opt-in summaries but not raw reasoning tokens, and the
[structured-output guide](https://developers.openai.com/api/docs/guides/structured-outputs) places
the Responses format under `text.format`.

## First-slice boundary

The frozen surface contains one generation with these output forms:

- an assistant `message` item with one or more `output_text` or `refusal` parts;
- a `reasoning` item with public `summary_text` parts;
- strict structured output carried as the authored bytes of an `output_text` part;
- stable response, item, content-part, output-index, and sequence identities;
- completed, incomplete, failed, cancelled, and in-progress lifecycle states;
- exact usage with separate input, visible output, and reasoning token counts.

Tool items, hosted tools, MCP, skills, images, audio, realtime sessions, background execution, and
WebSocket transport remain outside this slice. The types added later must reserve exhaustive enum
extension points for them without accepting arbitrary pass-through JSON as a shortcut.

## Reasoning policy

Only observable public behavior is a compatibility target:

| Fixture value | Responses destination | Policy |
| --- | --- | --- |
| `reasoning_summary` | `reasoning.summary[].text` and summary events | Public and supported when requested. |
| `reasoning_encrypted` | `reasoning.encrypted_content` | Opaque replay token; never inspected, logged, or rendered as text. |
| `reasoning_trace` | Existing Chat `reasoning_content` extension only | Synthetic test data, never mapped into a Responses summary. |
| `reasoning_tokens` | `usage.output_tokens_details.reasoning_tokens` | Exact fixture override or deterministic tokenizer result. |

Effort is validated against a model capability profile. An exact authored effort variant wins,
then an explicitly declared fixture fallback. If neither exists, planning fails with a fixture
coverage error; the simulator never fabricates a more or less capable answer.

## Normal event state machine

Every event carries a zero-based, contiguous `sequence_number`. A normal stream has one
`response.created`, one terminal response event, and no event after the terminal event. Responses
streams do not use Chat Completions' `[DONE]` sentinel.

| From | Event | To | Invariant |
| --- | --- | --- | --- |
| absent | `response.created` | response in progress | Response identity becomes fixed. |
| created | `response.in_progress` | response in progress | Output is initially empty. |
| response | `response.output_item.added` | item in progress | `output_index` and item id become fixed. |
| reasoning item | `response.reasoning_summary_part.added` | summary part in progress | Summary text starts empty. |
| summary part | `response.reasoning_summary_text.delta` | summary part in progress | Non-empty deltas concatenate in order. |
| summary part | `response.reasoning_summary_text.done` | summary text complete | Full text equals concatenated deltas. |
| summary part | `response.reasoning_summary_part.done` | summary part complete | Completed part equals the non-streamed part. |
| message item | `response.content_part.added` | content part in progress | Part id is its item/index pair. |
| output-text part | `response.output_text.delta` | content part in progress | Non-empty deltas concatenate byte-for-byte. |
| output-text part | `response.output_text.done` | output text complete | Full text equals concatenated deltas. |
| content part | `response.content_part.done` | content part complete | Completed part equals the non-streamed part. |
| item | `response.output_item.done` | item complete | Completed item equals its non-streamed output item. |
| response | `response.completed` | terminal completed | Embedded response equals the non-streamed response. |

`response.incomplete`, `response.failed`, `response.cancelled`, and `error` are terminal alternatives
whose exact payload snapshots are added with their implementation slices. Reasoning summary events
finish before visible output begins in the initial renderer. Later interleaving is allowed only for
a separately authored case backed by a pinned upstream contract.

## Structured-output policy

The first validator uses `jsonschema` 0.49.x with default features disabled so HTTP resolution,
file resolution, `reqwest`, and TLS do not enter the offline dependency graph. The supported strict
subset is deliberately small: object, array, string, number, integer, boolean, and null types;
properties; required members; `additionalProperties: false`; enum; nested arrays and objects; and
`anyOf` for unions and nullable values. Remote references and unsupported keywords fail during
fixture linting.

Strict fixture/schema mismatch is a build error. Runtime-invalid output exists only in a fixture
variant explicitly marked negative and selected explicitly. The compiler stores authored JSON bytes
and the parsed value separately so byte reconstruction and semantic validation remain independent.

## State and selection decisions

- `previous_response_id` requires an immutable stored predecessor. A response created with
  `store: false` is continued by replaying all prior output items, including opaque encrypted
  reasoning where present.
- Explicit case and variant selection is a public mock-only test control through
  `X-Simulate-Case`, `X-Simulate-Variant`, and matching `x_simulate` request members. It never skips
  protocol validation and never exists on a proxied upstream request.
- Adding `/v1/responses` is additive while marked experimental. It does not change the `1.0` Chat
  contract. Declaring stable Responses compatibility requires the next major version because its
  wire snapshots, selection inputs, and state rules then become compatibility guarantees.
- Known invalid fields fail through the common error envelope. Unknown request fields remain
  forward-compatible. Dataset authoring errors never become runtime API errors.

## Compatibility policy

Natural Chat matching keeps its current prefix, suffix, last-user, and digest fallback ladder. New
semantic fixtures are indexed in a separate schema-version-2 namespace and cannot enter the legacy
fallback candidate set. Importing a Chat fixture records its existing selection key and compiled
bytes; a compatibility report must show no changed legacy fallback result before an import is
accepted.

Within the new namespace, exact case/variant selection is stable across corpus growth. Digest
fallback is versioned by dataset revision, and a fixture build reports every known prompt whose
fallback target changes. Identifiers use named FNV-1a digest domains so adding one identifier does
not perturb another. File enumeration, map iteration, request order, counters, wall time, and system
randomness never participate.

## Executable corpus

The manifest pins provenance, the official SDK, and all cases. Each case contains the request,
non-streamed response, and complete ordered event payloads for its streamed equivalent:

- `text.json` freezes ordinary text and the minimal message lifecycle;
- `reasoning-summary.json` freezes public summary events before visible answer events and separate
  reasoning-token usage;
- `structured-output.json` freezes strict JSON request shape and awkward stream boundaries inside a
  JSON string.

`tests/responses_contract.rs` proves contiguous sequence numbers, legal state ordering, one terminal
event, delta reconstruction, equality between terminal and non-streamed responses, reasoning
visibility, structured semantic equality, and provenance/SDK pinning. `tests/responses_api.rs`
exercises the production event schedule through the in-process HTTP server.
