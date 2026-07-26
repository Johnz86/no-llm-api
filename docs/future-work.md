# Future development foundation

This document records product directions that remain intentionally outside the completed
`no-llm-api` implementation. It is a starting point for a future development cycle, not a roadmap,
commitment, or description of current behavior.

## Stable baseline

The current product is a deterministic, single-process test double for OpenAI Chat Completions. Its
core value is reproducible fixture selection, realistic streaming, controlled failure simulation,
and offline operation by default. Future work preserves these properties unless a new product scope
explicitly replaces them.

The following constraints remain useful design boundaries:

- Identical requests produce identical results without counters, rotation, shared state, or wall
  clock input in derived identity mode.
- The default build cannot contact a paid upstream service.
- New wire behavior has executable contract tests and deterministic fixtures.
- Simulation logic remains independent from HTTP route handling.
- Client-driven requirements take precedence over broad API-surface imitation.

## Candidate product directions

### Responses API

Support for `POST /v1/responses` is the largest adjacent product direction. It requires its own input
model, output item graph, event taxonomy, streaming state machine, tool-call representation,
persistence rules, and compatibility tests. It belongs in a separate design cycle rather than as an
alias over Chat Completions.

Start this work only when a target client depends on Responses API behavior. Begin with a recorded
wire contract and the smallest event subset that completes that client's normal chat flow. Preserve
unknown-event forward compatibility and avoid translating through loosely typed JSON values.

### Additional OpenAI-compatible endpoints

Some clients may require endpoints before or alongside chat:

- `/v1/embeddings` for retrieval and document workflows.
- `/v1/moderations` for input preflight and moderation UI.
- `/v1/audio/speech` and `/v1/audio/transcriptions` for voice clients.

Embeddings and moderation can use deterministic synthetic values. Audio requires multipart input,
binary output, media fixtures, and substantially different test infrastructure. Each endpoint enters
scope only with a concrete consumer and captured request/response examples.

### Stateful conversations and shared persistence

The server currently expects clients to resend conversation history and keeps explicitly stored
completions in process memory. Future clients may require conversation affinity, durable storage, or
coordination across replicas.

A stateful design must define ownership, expiry, reset semantics, deterministic replay, concurrent
updates, and failure recovery before choosing a database. Session metadata must not silently alter
fixture selection for existing stateless clients.

### Richer structured-output validation

Fixtures can represent JSON and structured-output responses, while runtime JSON Schema enforcement
is outside the current scope. A future implementation may validate fixture output against a request's
`json_schema`, synthesize deterministic validation failures, and expose schema-specific diagnostics.

This direction needs a bounded JSON Schema dialect, explicit behavior for unsupported keywords, and
tests that prevent the validator dependency from changing otherwise identical response bytes.

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

Open WebUI and the real `async-openai` client provide the current external contracts. A future cycle
may add continuously tested flows for LibreChat, Jan, openai-js, the Vercel AI SDK, or other clients.
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
