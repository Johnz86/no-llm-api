# Text and reasoning simulation plan

## Status and intent

This document tracks the staged text and reasoning implementation beyond version 1.0.0. The server
provides deterministic offline Chat Completions and non-streamed Responses while the remaining work
extends the Responses transport and state model.

Milestones T0 through T3 are complete. The pinned scope, decisions, event/state table, and executable
wire corpus live in `docs/spec/responses-text-scope.md` and `docs/spec/responses-text-contract/`.
Schema-v2 fixtures, deterministic compilation, canonical requests, semantic plans, redacted explain
output, golden plan snapshots, and reproducible artifacts live under `fixtures/v2`, `sim`, and the
fixture CLI. Explicit per-request selectors render those plans through the existing Chat response,
SSE, and storage paths with effort variants, schema validation, reasoning-first token budgets,
diagnostic headers, and stage-aware faults. The legacy matching ladder remains isolated and
byte-stable. The non-streamed Responses route renders text, refusals, public reasoning summaries,
and schema-validated structured output through the locked official JavaScript client. Milestones T4
through T6 remain future work; Responses streaming, state, tools, and control-plane diagnostics are
not implemented yet.

The first future delivery stays text-first. It establishes the shared item model, deterministic
reasoning policy, state transitions, scenario vocabulary, and validation rules that later tool,
MCP, skill, image, audio, and realtime simulators can reuse. It does not execute a model or attempt
to synthesize plausible language dynamically.

The compatibility target follows public OpenAI wire behavior, not private model internals. OpenAI
describes Responses as an item-oriented API with one generation, distinct message and function-call
items, multi-turn continuation, and an event-rich stream. It also recommends Responses for new
reasoning and tool workflows. See the official [Responses migration guide](https://developers.openai.com/api/docs/guides/migrate-to-responses),
[streaming guide](https://developers.openai.com/api/docs/guides/streaming-responses),
[reasoning guide](https://developers.openai.com/api/docs/guides/reasoning), and
[structured-output guide](https://developers.openai.com/api/docs/guides/structured-outputs).
The implementation should pin an upstream OpenAPI provenance revision and client versions before
coding; this document intentionally avoids freezing volatile model names or undocumented semantics.

## Product outcomes

The text-first simulator should let an application test, with no network and no inference:

- ordinary assistant text, markdown, code, citations, refusals, truncation, and empty output;
- short and long reasoning phases with configurable effort, summaries, usage, pacing, and faults;
- strict JSON and JSON Schema output, including intentional schema violations;
- Chat Completions and Responses representations of the same authored semantic exchange;
- stateless replay, `previous_response_id` continuation, stored conversations, and explicit replay
  of prior output items;
- deterministic response lifecycle and streaming events, including interruptions and recovery;
- model capability validation and stable differences between model profiles;
- reproducible scenario selection in single requests, multi-turn tests, concurrent runs, and CI.

The core assertion is stronger than “the same prompt usually gets the same answer”: the same
canonical request, dataset revision, scenario, model profile, seed, and prior state produce the same
semantic plan, identifiers, usage, event sequence, and terminal state. Timing configuration may
change delivery timestamps and gaps, but not event payload bytes.

## Current baseline versus proposed behavior

| Concern | Shipped 1.0.0 behavior | Proposed text-first behavior |
| --- | --- | --- |
| Endpoint | Chat Completions and stored-chat routes | Add a separately typed Responses surface; do not alias it to Chat routes |
| Response unit | One or more chat choices | One Responses output item graph and one generation |
| Selection | Model plus normalized message history; prefix, suffix, last-user, digest fallback | Canonical semantic input plus explicit variant constraints; preserve the existing Chat ladder |
| Reasoning input | `reasoning_effort` is typed but may be ignored | Validate effort against model capabilities and resolve an authored effort variant or policy |
| Reasoning output | A leading `<think>...</think>` becomes `reasoning_content`; it streams before visible content | First-class reasoning fixture data, optional summaries, explicit visibility, usage, item/status events |
| Structured output | `response_format` is typed and echoed; a JSON fixture exists | Validate fixture payloads against JSON mode/schema and support deterministic valid/invalid variants |
| State | Clients resend messages; `store: true` persists Chat records in process | Add response objects, continuation links, conversation state, expiry/reset semantics, and replay modes |
| Streaming | Chat chunk deltas and `[DONE]` | Keep Chat SSE unchanged; add typed Responses lifecycle/item/content events in documented order |
| Faults | HTTP error, stall, drop, SSE error, slow/recover | Add stage- and event-addressed faults, incomplete/cancelled states, malformed structured output |
| Datasets | Conversation YAML compiled to parquet | Versioned semantic fixture schema compiled into endpoint-specific plans and validation reports |

Existing Chat selection, response bytes, snapshots, routes, and default scenarios remain compatible
unless a future major release deliberately changes them. New Responses support must not silently
reinterpret current fixture files or make the offline dependency graph network-capable.

## Architecture

### Shared semantic plan

Introduce an endpoint-neutral immutable plan between fixture selection and transport rendering:

```text
CanonicalRequest
  -> SelectionKey + VariantConstraints
  -> FixtureCase + OutcomeVariant
  -> SemanticResponsePlan
  -> ChatRenderer | ResponsesRenderer
  -> JSON body | ordered SSE events
```

`SemanticResponsePlan` should contain only fully resolved facts:

- selected fixture set, case, turn, outcome variant, and match explanation;
- model profile and effective controls;
- ordered output nodes: reasoning, assistant text, refusal, structured value, and later tool nodes;
- terminal status and reason;
- precomputed token accounting and deterministic identities;
- a stream schedule expressed as logical stages and pieces, not sleeps;
- fault decisions and their exact logical injection point.

The plan is immutable and serializable for diagnostics and golden tests. Renderers must not perform
selection, consult storage, sample randomness, or infer missing content. HTTP routes only validate
transport, invoke the service, and render the result.

### Proposed modules and ownership

- `sim::canonical`: normalize Chat messages and Responses input items into a stable semantic input
  while preserving role, part type, item type, tool linkage, and structured data boundaries.
- `sim::select`: retain the current matching ladder and add explicit case/variant resolution. Any
  new selector is a pure function with a versioned algorithm identifier.
- `sim::plan`: build the endpoint-neutral response plan, usage, identity, and terminal state.
- `sim::reasoning`: resolve effort, summary, visibility, reasoning token budget, and reasoning-stage
  faults from model, fixture, scenario, and request controls.
- `sim::structured`: compile and validate supported JSON Schemas and deterministic invalid variants.
- `sim::stream`: consume a resolved schedule and emit logical pieces. Endpoint renderers map pieces
  to their own event vocabularies.
- `responses::model` and `responses::request`: typed wire vocabulary isolated from existing Chat
  models.
- `responses::render`: response-object and SSE event state machine.
- `responses::store`: response/conversation records and continuation indexes; do not overload the
  existing Chat completion store with loosely typed values.
- `http::routes`: thin mounts, validation, auth, headers, and common error conversion only.

Use exhaustive enums for item, event, status, effort, summary, and fault kinds. Keep unknown request
members forward-compatible at the deserialization boundary, but never model known protocol members
as generic JSON merely to avoid defining their invariants.

## Request and control model

### Canonical input

Canonicalization must retain behaviorally meaningful differences:

- endpoint family and endpoint schema revision;
- model id and model capability profile revision;
- developer/system instructions separately from user content;
- ordered message or item roles, content-part types, and normalized textual content;
- tool-call ids and tool-output linkage, even before tool execution is implemented;
- structured-output name, strictness, and canonicalized schema;
- effective reasoning configuration;
- explicit continuation (`previous_response_id`), conversation id, or replayed prior items;
- requested storage, streaming, maximum output, and include fields;
- fixture namespace, scenario name/revision, seed, and explicit case selector.

Normalization may fold Unicode case and whitespace only where the current matching contract already
does so for natural-language matching. It must not normalize JSON string values, code, schemas,
tool arguments, ids, or text where byte differences are material. Canonical JSON uses recursively
sorted object keys, preserves array order and numeric representation rules, and is length-framed
before entering the repository FNV-1a digest.

### Control precedence

Resolve effective behavior once, in this order from strongest to weakest:

1. per-request simulation directive;
2. fixture outcome variant selected by explicit case id or request constraints;
3. active scenario;
4. model simulation profile;
5. server defaults.

Protocol request fields such as reasoning effort are inputs, not simulation directives. A fixture
may require or forbid particular protocol values. An explicit `X-Simulate-*` directive may choose a
fault or case but must never bypass normal protocol validation.

### Explicit fixture selection

Add an opt-in, test-oriented selector that is safe in parallel suites:

- header `X-Simulate-Case: <stable-case-id>` and equivalent `x_simulate.case` body member;
- optional `X-Simulate-Variant: <stable-variant-id>`;
- error on an unknown id or on a variant incompatible with the request;
- response headers exposing dataset revision, case id, variant id, match kind, and plan digest.

Explicit selection makes negative and fault tests independent of digest fallback and fixture-set
growth. Natural matching remains the user-facing default. Never use global “next response” queues.

## Reasoning and effort simulation

### Principle

Simulate the public reasoning contract and observable application effects, not hidden chain of
thought. Author fixtures as one or more of:

- `reasoning_summary`: user-visible summary text intended for supported summary fields/events;
- `reasoning_trace`: synthetic test-only reasoning text for compatibility surfaces such as the
  repository's existing `reasoning_content` extension;
- `reasoning_encrypted`: opaque deterministic bytes or fixture text for replay-path testing;
- `reasoning_tokens`: explicit accounting override when an exact client assertion matters;
- `answer`: visible assistant content.

The fixture format must label visibility and destination. Never derive a public trace by exposing an
internal field, and never assume all models or endpoints reveal the same reasoning representation.

### Effort resolution

Represent effort as an open compatibility layer: a typed set supported by each model profile plus an
`unsupported` validation path. Do not hard-code one universal list because public model capabilities
evolve. Store the raw wire spelling alongside the resolved enum so a future compatible value can be
accepted deliberately after contract review.

Each fixture case may provide variants such as `none`, `low`, `medium`, `high`, and a default. The
resolution algorithm is deterministic:

1. validate the requested effort against the selected model profile;
2. choose an exact authored effort variant when available;
3. otherwise choose the fixture's declared fallback variant;
4. if fallback is forbidden, return a fixture-coverage error rather than fabricating content.

Effort does not automatically improve an answer. Authored variants make observable differences
explicit. A model profile may map effort to reasoning delay, reasoning token count, summary detail,
and default visible-answer variant, but all mappings are data, versioned, and testable.

### Reasoning lifecycle scenarios

Cover at least these cases:

- no reasoning requested and a direct answer;
- low/medium/high effort variants with the same answer but different reasoning usage and time;
- effort variants with intentionally different final answers for application eval fixtures;
- reasoning summary only, synthetic trace only, both, and neither;
- reasoning completes before answer text begins;
- interleaved public summary and output where the target protocol documents it;
- long silent thinking delay followed by a fast answer;
- progressive reasoning summaries and a final answer;
- reasoning reaches output limit before visible content;
- visible answer truncates after reasoning completes;
- refusal after reasoning, without leaking the trace into content;
- reasoning-stage stall, transport drop, documented error event, cancellation, and recovery retry;
- continuation that reuses prior reasoning state and continuation with state unavailable;
- opaque encrypted reasoning replay accepted, missing, duplicated, corrupted, or linked to the wrong
  response;
- unsupported effort for a model and conflicting effort/summary controls;
- usage with zero visible tokens but non-zero reasoning tokens.

## Text output scenario catalogue

Create fixture families around application behavior rather than model marketing names.

### General presentation

- one-line answer, multi-paragraph prose, lists, tables, nested markdown, links, and citations;
- fenced code with language tags, embedded backticks, leading/trailing whitespace, and no newline;
- Unicode grapheme clusters, combining marks, emoji, right-to-left text, CJK, and token boundaries
  that split multi-byte UTF-8;
- empty content, whitespace-only content, extremely long output, and a response exactly at the token
  limit;
- stop-sequence termination before, across, and after a token boundary;
- refusal only, refusal plus safe alternative, and content-filter terminal state;
- multiple Chat choices remain a Chat-only compatibility case; Responses yields one generation.

### Instruction and multi-turn behavior

- developer instruction changes style while the user prompt remains identical;
- user correction, clarification, pronoun reference, and carry-forward constraints;
- repeated user text at different turns resolves to the correct scripted reply;
- assistant-prefill or prior assistant content where accepted by the endpoint;
- resend-full-history, continue-by-response-id, continue-by-conversation, and replay-items forms
  resolve to the same authored semantic turn when their effective context is equivalent;
- branched continuations from one prior response produce stable independent child ids;
- deleted, expired, unknown, cross-conversation, and cyclic continuation references fail clearly;
- concurrent identical continuations do not race or generate different payloads;
- a stored response retrieved later matches its creation bytes and status.

### Structured output

- JSON object mode with valid object, arrays nested inside it, escaped Unicode, and large integers;
- strict schema covering required/optional fields, enums, arrays, nested objects, unions, and nulls;
- valid response with a different JSON key order but identical semantic value, when byte order is not
  part of the fixture contract;
- deterministic schema violations: missing required field, additional field, wrong type, enum miss,
  range miss, invalid JSON, truncated JSON, refusal instead of JSON, and schema compilation failure;
- streaming splits inside JSON strings, escapes, numbers, property names, and UTF-8 without changing
  the final fixture bytes;
- schema requested but the selected fixture has no compatible structured variant;
- schema name/strictness differences participate in variant compatibility and plan identity.

The first schema implementation should support a documented subset backed by a maintained validator
crate. Unsupported keywords fail at fixture lint or request validation; they are never silently
ignored in strict mode. Store both the authored JSON bytes and parsed value so tests can distinguish
wire-byte assertions from semantic assertions.

### Limits and accounting

- maximum output smaller than reasoning, exactly reasoning, and reasoning plus part of answer;
- cached input, reasoning, visible output, accepted/rejected prediction, and total details where the
  selected wire contract supports them;
- fixture-provided exact usage and tokenizer-derived usage, with a clear precedence rule;
- context-window rejection before planning;
- deterministic accounting across streamed and non-streamed forms;
- model profile with different context and output ceilings but identical fixture content.

## Responses-style object and streaming contract

### Minimal text-first output items

Start with a deliberately small, typed subset:

- response object with stable id, creation data, model, status, error/incomplete details, output,
  usage, metadata, and continuation/conversation linkage required by target clients;
- reasoning item with supported summary/content representation and status;
- assistant message item containing ordered output-text or refusal content parts;
- no tool items in the first milestone, but reserve exhaustive extension points rather than generic
  pass-through objects;
- deterministic `output_text` convenience aggregation derived only from output-text parts.

An endpoint-neutral plan may render to either API, but the wire shapes remain independent. In
particular, do not emulate Responses by wrapping a Chat completion or expose Chat's `n` choices.

### Stream state machine

Capture the exact event names and required fields from the pinned upstream schema during
implementation. The logical sequence should cover:

1. response created/in-progress;
2. output item added;
3. content part added;
4. reasoning summary/content deltas and completion events when present;
5. output text or refusal deltas and completion events;
6. content part completed;
7. output item completed;
8. response completed, incomplete, failed, or cancelled.

Every event receives a monotonic deterministic sequence number if required by the target contract.
Item ids, output indexes, content indexes, and response id remain stable. A normal stream has one
terminal response event and no events afterward. Reconstructing all deltas must equal the
non-streamed object. Cancellation stops pacing immediately and records a deterministic terminal
state only where the protocol permits the server to send one before disconnection.

Do not assume Responses streams end with Chat's `[DONE]`; render the terminal convention of the
pinned Responses contract. Unknown incoming include values should follow public validation rules,
while unknown received event types in test helpers should be preserved for forward-compatible
client assertions.

## Dataset and fixture format

### Versioned semantic YAML

Introduce a new schema version rather than adding ambiguous optional columns to the existing Chat
turn format. A conceptual shape is:

```yaml
schema_version: 2
id: release-review
description: Reasoned release assessment with strict JSON alternative.
match:
  models: [mock-reasoner]
  turns:
    - role: user
      text: Assess the release.
cases:
  - id: prose
    constraints:
      endpoint: [chat_completions, responses]
      effort: [low, medium, high]
    variants:
      low:
        reasoning_summary: Checked status and blockers.
        answer: The release is ready.
      high:
        reasoning_summary: Checked status, blockers, rollback, and ownership.
        answer: The release is ready, with rollback owner confirmed.
    finish: completed
  - id: strict-json
    constraints:
      endpoint: [responses]
      response_format: release_status
    output:
      json: {status: green, blockers: 0}
    finish: completed
```

The final schema should avoid YAML features that make canonicalization surprising. Reject aliases,
duplicate keys, implicit non-string map keys, unknown members, ambiguous dates, invalid JSON
arguments, and fixture ids that collide after normalization.

### Compiled dataset

The fixture builder should:

- lint source files and report file, case, variant, and field paths;
- validate all declared model, endpoint, effort, response-format, and scenario references;
- compile schemas once and validate every compatible structured output;
- tokenize reasoning and visible parts independently;
- precompute canonical keys, digests, variant tables, expected usage, and fixture provenance;
- emit a deterministic artifact independent of file enumeration and map iteration order;
- provide `lint`, `build`, `explain`, and `snapshot-plan` commands;
- embed a small built-in corpus while allowing an external dataset path;
- include schema version, selection algorithm version, tokenizer revision, source digest, and build
  tool version in artifact metadata.

Sort source files, cases, variants, maps, and compiled rows explicitly. Adding fixtures may still
change fallback selection, so expose a compatibility report listing prompts whose fallback changes.

### Explainability

Add a read-only control-plane explanation for a request or plan:

- canonicalized input (with credentials and sensitive fields redacted);
- matching rung and candidate set;
- selected case/variant and why each constraint matched;
- control precedence and effective values;
- digest inputs and final plan digest;
- logical stream schedule and fault decision;
- usage source (fixture or tokenizer-derived).

This output is diagnostic only and never participates in selection.

## Scenario model and fault injection

Separate reusable behavior scenarios from semantic fixture outcomes. Extend scenarios with named
logical stages: `request`, `reasoning`, `output`, `terminal`, and later `tool`. Address faults by
stage plus an event count, token count, logical item id, or elapsed simulated duration.

Text-first built-ins should include:

- `reasoner-fast`: short reasoning delay, fast answer;
- `reasoner-deliberate`: long first-reasoning delay and slow reasoning summaries;
- `reasoning-burst`: silent reasoning followed by an output burst;
- `structured-strict`: valid schema output with awkward stream boundaries;
- `structured-invalid`: a named deterministic schema violation;
- `reasoning-stall`: stall before visible output;
- `reasoning-drop`: disconnect during reasoning;
- `output-drop`: disconnect after valid reasoning but mid-answer;
- `incomplete-limit`: terminal incomplete state caused by output budget;
- `state-expired`: continuation points to a deterministically unavailable predecessor;
- `slow-then-recover`: stage-specific pacing recovery without byte changes.

Fault decisions use the seeded digest of canonical request, scenario revision, case, variant, and
logical fault site. A probability is deterministic, not random per call. Explicit fault directives
select the same logical site across chunk-size changes whenever possible. Invalid/malformed payload
faults must be opt-in and named so normal compatibility tests never accidentally receive them.

## Persistence and multi-turn state

Define three explicit modes:

- **stateless replay:** the request contains all prior semantic inputs/items; nothing is saved;
- **response continuation:** `previous_response_id` resolves an immutable predecessor and appends a
  deterministic child response;
- **conversation state:** a conversation id owns an ordered item log and responses append to it.

The storage contract must define:

- whether `store: false` responses can be continued and how replayed opaque reasoning is handled;
- immutable response payloads versus mutable conversation membership;
- id derivation from parent, canonical input, selected plan, and branch ordinal that is derived from
  input rather than a global counter;
- deterministic ordering for simultaneous appends and conflict behavior for identical/different
  requests;
- deletion, reset, not-found, ownership mismatch, expiry, and capacity eviction;
- process-local persistence first; durable or distributed storage remains out of scope.

Avoid wall-clock expiry in deterministic tests. Express expiry as an explicit scenario decision,
logical generation, or injected control. Clock mode may model real TTL separately, but derived mode
must reproduce availability from the same inputs and initial store snapshot.

## Validation and error behavior

Validate in layers and map every failure through the repository's common OpenAI-shaped error
envelope where applicable:

1. JSON and known-field type validation;
2. cross-field request rules;
3. model capability and limit validation;
4. continuation/conversation linkage validation;
5. fixture case and variant compatibility;
6. schema support and structured-output validation;
7. plan consistency before any bytes are written.

Negative tests should assert status, complete error JSON, `param`, `code`, request/version headers,
and absence of partial persistence. A stream failure after headers uses the pinned event contract;
pre-stream failures use HTTP errors. Dataset authoring errors are build-time diagnostics, not runtime
API errors.

## Acceptance test matrix

### Determinism

- 64 concurrent identical non-streamed requests yield one distinct response body and plan digest;
- 64 concurrent identical streams yield one distinct ordered event transcript;
- reruns in fresh processes and on supported operating systems reproduce ids, payloads, and events;
- shuffled fixture source enumeration compiles byte-identical artifacts;
- timing and chunk-size changes reconstruct identical semantic output;
- equivalent stateless, predecessor, and conversation continuations select the same semantic turn;
- branching and concurrent continuations remain stable without counters or request order.

### Wire and clients

- maintain all existing Chat tests and snapshots unchanged;
- add contract snapshots for each minimal Responses object and every normal event type;
- exercise the real locked official client for streamed/non-streamed text, reasoning summaries,
  structured output, continuation, storage, retrieval, and errors;
- verify unknown request fields remain forward-compatible and known invalid values fail;
- assert no compression/buffering, immediate disconnect cancellation, stable headers, and terminal
  event rules;
- compare reconstructed stream objects with non-streamed objects field by field.

### Reasoning and structured output

- effort validation and exact/fallback/no-fallback variant resolution;
- reasoning-before-content ordering, Unicode boundaries, zero visible output, and usage accounting;
- summaries/traces never leak into visible content or unsupported fields;
- each supported JSON Schema feature has valid and invalid fixtures;
- each named violation fails in the declared way and at the declared stage;
- token limits during reasoning and output yield correct partial content, status, and usage.

### State and faults

- continuation success, unknown/deleted/expired parent, wrong conversation, duplicate replay, branch,
  and reset;
- no partial store mutation on validation failure or pre-stream fault;
- drop/stall/error/cancel at reasoning, output, and terminal boundaries;
- retrying the same request reproduces the same fault unless a different explicit attempt key is
  part of the canonical request;
- control-plane logs redact sensitive content and opaque reasoning payloads.

### Quality gates

In addition to repository-wide formatting, tests, Clippy, denial, browser, and packaging gates,
introduce fixture-schema golden tests, official-client compatibility tests, compiled-artifact
reproducibility tests, and a default dependency-tree assertion that Responses support adds no
network client to the offline build.

## Milestones

### T0: Freeze the target contracts

- choose a dated upstream OpenAPI provenance and one official SDK version per target language;
- record minimal non-streamed and streamed text/reasoning/structured examples;
- write the Responses scope document and compatibility policy;
- decide the supported reasoning fields without exposing undocumented chain of thought.

Exit: reviewed wire snapshots and an event/state table exist before production models are added.

### T1: Semantic fixture schema and plan

- implement schema version 2, linter, deterministic compiler, canonical request, variant resolver,
  semantic plan, and explain output;
- import representative existing Chat fixtures without changing their current compiled artifact;
- add general text, multi-turn, reasoning-effort, and structured-output fixture families.

Exit: plan snapshots and artifact hashes reproduce across shuffled inputs and fresh processes.

### T2: Strengthen Chat text and reasoning

- consume the semantic plan behind the existing Chat renderer;
- make reasoning effort and structured-output behavior explicit and testable;
- preserve current Chat wire snapshots, matching ladder, stored routes, and SSE invariants;
- add stage-aware faults without changing existing scenario meanings.

Exit: old tests are byte-stable and the new effort/schema/fault matrix passes.

### T3: Minimal Responses text surface

- add typed create-response requests, response objects, message/reasoning items, usage, and errors;
- implement non-streamed text, refusal, reasoning summary, and structured output;
- add deterministic explicit case/variant selection and response diagnostics.

Exit: the locked official client completes all non-streamed text-first contract tests offline.

### T4: Responses streaming

- implement the pinned event state machine and schedule renderer;
- add delta reconstruction, event snapshots, stage faults, pacing, and cancellation;
- verify event order and terminal states with both repository and official-client harnesses.

Exit: streamed and non-streamed semantic objects are equivalent for the full scenario matrix.

### T5: Continuation and conversation state

- add response store, `previous_response_id`, conversation logs, replayed items, reset/deletion,
  deterministic expiry scenarios, branching, and concurrency semantics;
- expose read-only state diagnostics with redaction.

Exit: all continuation forms, branches, retries, and failures are reproducible under concurrency.

### T6: Stabilize the extension boundary

- extract output-item and stage abstractions needed by future tool/MCP/skill and media plans;
- publish fixture authoring examples, compatibility guarantees, and migration notes;
- run browser consumers and packaging checks, and review whether the change requires a 2.0.0 wire
  compatibility boundary.

Exit: later output item types can be added without changing text item identities or event ordering.

## Explicit non-goals

- Running an LLM, heuristic prompt completion, templated “AI-like” text, or nondeterministic
  generation.
- Reproducing or exposing private chain of thought, undocumented reasoning internals, or exact model
  intelligence differences.
- Claiming all OpenAI endpoints, models, schema keywords, event types, or historical SDK versions are
  supported; scope is pinned and executable.
- Implementing tool execution, MCP servers, skills, image generation, speech, dictation, or realtime
  audio in the text-first milestones. The plan only leaves typed extension points for them.
- Mapping Responses to Chat objects as an implementation shortcut.
- Durable database storage, distributed consistency, cross-process coordination, or production API
  proxying as a prerequisite.
- Using counters, fixture rotation, wall clock, system randomness, map iteration order, or
  `DefaultHasher` for selection, identity, variant choice, fault choice, or state ordering.
- Relaxing the offline default build, credential protections, loopback control-plane default, common
  error envelope, or existing Chat compatibility to accelerate the new endpoint.

## Decisions required before implementation

1. Which dated upstream schema and official SDK versions define the initial Responses contract?
2. Which reasoning representations are public compatibility targets, and which remain synthetic
   test-only extensions?
3. Should strict structured-output mismatch be a fixture-build failure by default, with explicitly
   named runtime-invalid variants as the only exception?
4. Does response continuation require `store: true`, and how should `store: false` opaque reasoning
   replay be represented without leaking or logging it?
5. Which JSON Schema subset is supportable with a maintained Rust validator and stable diagnostics?
6. Is explicit case selection part of the public mock API or restricted to the control/test plane?
7. Does adding Responses constitute the next major version immediately, or can it ship as an
   additive experimental feature before the compatibility guarantee is declared?

Resolve these decisions in the scope specification and executable tests. Do not encode answers only
in code comments or fixture conventions.
