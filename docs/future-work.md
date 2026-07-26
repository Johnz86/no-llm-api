# Future development foundation

This document records product directions that remain intentionally outside the completed
`no-llm-api` implementation. It is a starting point for a future development cycle, not a roadmap,
commitment, or description of current behavior.

## Stable baseline

The current product is a deterministic, single-process test double for OpenAI Chat Completions and
the experimental text/reasoning Responses and Conversations surface. It includes typed SSE,
reasoning effort and summaries, opaque reasoning replay, strict fixture-owned structured-output
schemas, predecessor and conversation state, lifecycle/fault simulation, and official-client tests.
Future work preserves reproducible selection, realistic streaming, controlled failure simulation,
and offline operation unless a new product scope explicitly replaces those properties.

The following constraints remain useful design boundaries:

- Identical requests produce identical results without counters, rotation, shared state, or wall
  clock input in derived identity mode.
- The default build cannot contact a paid upstream service.
- New wire behavior has executable contract tests and deterministic fixtures.
- Simulation logic remains independent from HTTP route handling.
- Client-driven requirements take precedence over broad API-surface imitation.

## Candidate product directions

### Responses tools, MCP, and skills

The next Responses expansion is typed tool behavior: function calls and outputs, hosted-tool items,
MCP discovery/approval/call lifecycles, and skill references or inline skill payloads. This requires
fixture schemas for call graphs, deterministic ids and arguments, request-controlled approval and
failure paths, reserved tool-stage SSE events, and replay validation across multiple turns.

Implement these as typed output/input items and event schedules. Do not tunnel them through generic
JSON, reuse Chat tool deltas, or expose a capability in the model catalogue before its normal,
streaming, error, and official-client cases are executable.

### Images, audio, and realtime conversation

Image generation needs deterministic binary fixtures, format/size/quality controls, revised item
events, and byte/hash assertions. Speech generation and Whisper-style transcription need binary and
multipart transports, media datasets, timestamps, and platform-independent golden assets.

Realtime or conversational audio adds WebSocket session state, duplex event ordering, interruption,
turn detection, cancellation, and reproducible timing. It should follow the completed request/response
audio endpoints rather than being introduced as a loosely simulated socket.

### Additional OpenAI-compatible endpoints

Some clients may require endpoints before or alongside chat:

- `/v1/embeddings` for retrieval and document workflows.
- `/v1/moderations` for input preflight and moderation UI.
- `/v1/audio/speech` and `/v1/audio/transcriptions` for voice clients.

Embeddings and moderation can use deterministic synthetic values. Audio requires multipart input,
binary output, media fixtures, and substantially different test infrastructure. Each endpoint enters
scope only with a concrete consumer and captured request/response examples.

### Durable and distributed state

Responses, predecessors, and conversations currently live in process memory with explicit reset,
deletion, and optimistic conflict semantics. Future clients may require durable storage,
conversation affinity, or coordination across replicas.

A stateful design must define ownership, expiry, reset semantics, deterministic replay, concurrent
updates, and failure recovery before choosing a database. Session metadata must not silently alter
fixture selection for existing stateless clients.

### Broader structured-output dialects

The current compiler intentionally allows a bounded offline JSON Schema keyword set and explicit
negative variants. Future demand may justify more draft keywords, grammars, or schema dialects.
Every addition needs a pinned dialect, offline reference rules, compatibility reporting, and proof
that validator upgrades do not change existing authored bytes or error envelopes.

### Expanded observability

The current Prometheus surface intentionally contains a small set of counters. Possible additions
include latency histograms, structured trace correlation, OpenTelemetry export, dashboards, and
profiling endpoints.

Observability remains opt-in and must not add nondeterministic payload fields, expose credentials, or
pull a telemetry stack into the default build without a demonstrated operational need.

### Deployment and distribution

The current distribution targets a single process, release binaries, and a distroless container.
Future demand may justify crates.io publication, signed artifacts, an SBOM, Kubernetes manifests, a
Helm chart, or multi-architecture container publication.

Clustered deployment is a product change rather than packaging alone because stored completions,
control-plane state, request logs, and metrics are process-local. Multi-replica support therefore
depends on an explicit shared-state contract.

### Broader client compatibility

Open WebUI, `async-openai`, and the official JavaScript `openai` client provide the current external
contracts. A future cycle may add continuously tested flows for LibreChat, Jan, the Vercel AI SDK,
or other clients.
Each integration earns permanent automation only when it exercises behavior not already covered by
the protocol-level suite.

## Entry criteria for new work

Before promoting a candidate into active development, record:

1. The consumer and user workflow that require it.
2. The exact HTTP or streaming transcript to reproduce.
3. The smallest compatible surface and explicit non-goals.
4. Determinism, security, dependency, and offline-mode consequences.
5. Acceptance tests at the model, protocol, and real-client levels.
6. Documentation and versioning impact for existing consumers.

A proposal that cannot answer these questions remains exploratory. This keeps the repository a
purposeful test product instead of an incomplete clone of every upstream endpoint.
