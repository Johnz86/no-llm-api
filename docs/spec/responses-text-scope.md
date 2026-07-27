# Responses text contract freeze

## Status

This document freezes the Responses text contract. The repository serves non-streamed objects and
typed SSE events at `POST /v1/responses`; the executable corpus in
[`responses-text-contract/`](responses-text-contract/) defines both boundaries.

The locked `openai` 6.49.0 JavaScript client exercises response objects and text, reasoning-first,
strict structured-output, and refusal streams in `e2e/responses-client.spec.ts`. The frozen corpus
is generated from and checked byte-for-byte against the production response renderer and event
schedule. The same client verifies immutable storage, predecessor continuation, typed errors,
non-completed lifecycle states, conversation conflicts, and conversation resource/item operations.

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

## Implemented boundary

The frozen surface contains one generation with these output forms:

- an assistant `message` item with one or more `output_text` or `refusal` parts;
- a `reasoning` item with public `summary_text` parts;
- strict structured output carried as the authored bytes of an `output_text` part;
- stable response, item, content-part, output-index, and sequence identities;
- completed, incomplete, failed, cancelled, and in-progress lifecycle states;
- exact usage with separate input, visible output, and reasoning token counts.

Tool items, hosted tools, MCP, skills, images, audio, realtime sessions, background execution, and
WebSocket transport are outside this contract. Their pinned request controls deserialize as typed
values where required for forward compatibility, but unsupported behavior fails explicitly instead
of passing arbitrary JSON through the simulator.

## Reasoning policy

Only observable public behavior is a compatibility target:

| Fixture value | Responses destination | Policy |
| --- | --- | --- |
| `reasoning_summary` | `reasoning.summary[].text` and summary events | Public and supported when requested. |
| `reasoning_encrypted` | Sealed `reasoning.encrypted_content` when explicitly included | Authored material contributes to an opaque envelope; it is never exposed, logged, or rendered as text. |
| `reasoning_trace` | Existing Chat `reasoning_content` extension only | Synthetic test data, never mapped into a Responses summary. |
| `reasoning_tokens` | `usage.output_tokens_details.reasoning_tokens` | Exact fixture override or deterministic tokenizer result. |

Effort is validated against a model capability profile. An exact authored effort variant wins,
then an explicitly declared fixture fallback. If neither exists, planning fails with a fixture
coverage error; the simulator never fabricates a more or less capable answer.

`include` is a closed enum matching the pinned upstream values. This text surface supports only
`"reasoning.encrypted_content"`; every other known include value fails with `unsupported_parameter`,
and an unknown include value fails deserialization. Without the supported include, reasoning items
and stream events omit `encrypted_content`. With it, the simulator emits an `enc_v1_...` envelope
that seals the response resource digest, the emitted reasoning-token count, and a digest of the
fixture-authored opaque material. The envelope is deterministic authentication and linkage metadata,
not encrypted chain-of-thought and not a reversible representation of authored reasoning.

`max_output_tokens` accepts the pinned schema minimum of 16. Reasoning tokens consume the budget
before visible text or refusal tokens. Exhaustion truncates only on tokenizer boundaries, updates
usage to the emitted budget, marks output items incomplete, and terminates the response with
`incomplete_details.reason = "max_output_tokens"` in object and streamed forms.

The selected model profile also limits output and input. A requested output cap above the profile's
`max_output_tokens` fails with `max_output_tokens_exceeded`; when the request omits the cap, the
model ceiling is the effective budget. Canonical input tokens plus carried reasoning tokens may
equal the model's `context_window`; a larger context fails with `context_length_exceeded` before
planning, storage, or conversation mutation. The same canonical history therefore produces the
same limit result in stateless replay, predecessor, and conversation modes.

## Identity and usage accounting

Semantic plan identity is derived from the canonical conversation and semantic request controls,
including model, effort, response format, and requested output budget. It does not change for
transport, persistence, or representation projections such as streaming, storage, metadata,
parallel-tool echo state, reasoning-summary projection, or the encrypted-reasoning include. Fault
selection uses this semantic plan identity, so equivalent representations do not select different
injected behavior.

Response resource identity is a separate `responses-resource-v3` digest over the semantic plan and
all state that can change the immutable response body: instructions, request and effective output
budgets, model, linkage, reasoning/text settings, supported include state, persistence, tools,
carried reasoning usage, and metadata. Stream transport is excluded because its terminal embedded
Response equals the non-streamed object. A repeated resource id must map to byte-identical stored
content; storage returns `response_store_invariant` if that invariant is ever violated.

Input usage is the tokenizer count of the resolved canonical turns plus cumulative carried
reasoning tokens. Stored predecessors retain canonical turns and cumulative reasoning usage;
conversation items and explicit replay reconstruct the same values. Each prior reasoning generation
is counted once, including across multiple generations, and the current generation adds its emitted
reasoning tokens only to output usage. Stateless replay, predecessor continuation, and conversation
continuation consequently report equal usage for equal semantic history.

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

`response.incomplete`, `response.failed`, `response.cancelled`, and `error` are terminal alternatives.
Authored lifecycle fixtures exercise response errors, incomplete details, response/item status
separation, staged errors, deliberate drops, and consumer cancellation. Reasoning summary events
finish before visible output begins. Interleaved reasoning and visible output is outside the pinned
event contract.

## Structured-output policy

The validator uses `jsonschema` 0.48.x with default features disabled so HTTP resolution, file
resolution, `reqwest`, and TLS do not enter the offline dependency graph. Fixture-owned schemas
accept the documented offline keyword allow-list: local `$defs`/`$ref`, types, properties, required,
items, additional properties, enum/const, numeric/string/array bounds, pattern/format, and
all/any/one/not composition. Remote references and every keyword outside that list fail during
fixture linting.

Strict fixture/schema mismatch is a build error. Runtime-invalid output exists only in a fixture
variant explicitly marked negative and selected explicitly. The compiler stores authored JSON bytes
and the parsed value separately so byte reconstruction and semantic validation remain independent.

## State and selection decisions

- `store: true` saves an immutable process-local Response object after request validation and
  pre-response fault checks. `GET /v1/responses/{response_id}` retrieves the exact object and
  `DELETE` returns `{id, object: "response", deleted: true}`; stateless responses are not
  retrievable.
- `GET /_mock/responses` exposes only sorted response identity, model, status, linkage, and aggregate
  item/turn counts. It never returns input, output text, reasoning, metadata, or credentials.
  `POST /_mock/reset` clears this process-local state together with the request log.
- Conversation resources use deterministic content-derived `conv_` identities. A Response can
  reference the resource by string or `{id}`; its existing turns are prepended during planning and
  the completed input/reasoning/output items are appended as one deterministic group. The item
  collection supports typed batches of 1–20 messages, retrieval, deletion, `asc`/`desc` ordering,
  `after` cursors, and bounded pagination. Identical batch retries reuse item identities without
  duplication, and deleted semantic items leave future planning context.
- `previous_response_id` requires an available immutable stored predecessor. The predecessor's
  canonical turns are prepended for semantic matching and its complete public object participates
  in child plan identity, so branches require no counters or request ordering. Missing and deleted
  predecessors fail before planning with `previous_response_not_found`. A `store: false` response
  is stateless and cannot be referenced by id. The `state-expired` scenario keeps stored objects
  inspectable while making continuation from an existing object fail reproducibly with
  `previous_response_expired`; lookup happens first, so unknown and deleted ids remain
  `previous_response_not_found` under every scenario. Expiry never consults wall-clock time or
  mutates storage.
- Stateless input accepts ordinary assistant messages as semantic history without requiring replay
  metadata. Opaque replay accepts only a completed reasoning item immediately followed by the
  completed assistant message emitted with it. The intact `enc_v1_...` envelope must authenticate
  and name the same resource digest as the reasoning item id. Trailing reasoning, intervening items,
  consecutive reasoning items, missing or partial messages, duplicate ids, raw `reasoning_text`,
  altered envelopes, and mismatched pairs fail against `input` before fixture selection.
- Process-local response and conversation stores have no implicit time- or capacity-based eviction.
- A Response associated with a conversation commits only if the conversation still has the exact
  item generation used to plan it. Overlapping writes may have one winner; every stale writer gets
  `409 conversation_conflict` and must retry. No ordering is promised between distinct concurrent
  requests. Sequential identical requests are separate turns and both succeed.
  Objects leave the store only through their typed delete route or `POST /_mock/reset`. Continuation
  cycles are structurally impossible because a child can only reference an already stored immutable
  predecessor, and conversation linkage cannot be combined with `previous_response_id`.
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
fallback is versioned by dataset revision. `fixtures compatibility-report` compares compiled
artifacts and fails for changed match/default/fallback assignments, existing output bytes, removed
variants, legacy payload bytes, or a new fixture/case that overlaps an existing digest-fallback
class. Candidate-set findings name the added case, affected baseline case, and overlapping
interface/model/effort/format class. An added explicit non-default variant is reported but remains
compatible because it cannot enter case selection. Identifiers use named FNV-1a digest domains so
adding one identifier does not perturb another. File enumeration, map iteration, request order,
counters, wall time, and system randomness never participate.

Response output items expose typed identity, kind, and message-content accessors shared by rendering,
conversation ownership, and routes. Stream schedules carry fault-stage metadata separately from
their serialized event objects; stage selection never reparses event names. The `tool` fault stage
is reserved and currently has no output-item implementation. Existing `msg_` and `rs_` identities,
output indexes, event ordering, and terminal objects are compatibility invariants.

## Executable corpus

The manifest pins provenance, the official SDK, and all cases. Each case contains the request,
non-streamed response, and complete ordered event payloads for its streamed equivalent:

- `text.json` freezes ordinary text and the minimal message lifecycle;
- `reasoning-summary.json` freezes public summary events before visible answer events and separate
  reasoning-token usage;
- `structured-output.json` freezes strict JSON request shape and awkward stream boundaries inside a
  JSON string.

`tests/responses_contract.rs` independently runs the production compiler, renderer, router, and SSE
schedule against every frozen object and event. It also proves contiguous sequence numbers, legal
state ordering, one terminal event, delta reconstruction, terminal/non-streamed equality, reasoning
visibility, structured semantic equality, and provenance/SDK pinning. `tests/responses_api.rs` adds
lifecycle, state, replay, Unicode, zero-visible-output, concurrency, and staged-fault coverage.
