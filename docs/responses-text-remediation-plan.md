# Responses Text and Reasoning Remediation Plan

## Status

This document defines the remaining work required to complete the deterministic
Responses text and reasoning implementation. It records the findings of an
independent implementation review and the resulting software architecture.

The work is organized as dependency-ordered, independently testable slices.
The implementation is not considered finalized until every acceptance criterion
in this document passes.

## Objectives

The remediation establishes these properties:

- semantic fixture selection remains deterministic and independent of transport;
- every distinct immutable Response representation has a distinct resource
  identity;
- persisted responses are idempotent and cannot become request-order dependent;
- ordinary assistant history and explicit output-item replay are both supported;
- opaque reasoning can be requested, replayed, and accounted for without
  exposing or interpreting reasoning content;
- stateless, predecessor, and conversation continuations use equivalent context
  and token-accounting rules;
- model context and output limits are enforced consistently;
- missing and expired predecessor states remain distinguishable;
- fixture compatibility reports detect changes to digest fallback selection;
- maintained documentation describes the implemented contract in present tense.

## Validated implementation gaps

### Response identity and immutable storage

Response, reasoning, and message identities are derived from the semantic plan
digest. The returned Response object also varies with values that are not part of
that digest, including `store`, metadata, `parallel_tool_calls`, reasoning summary
projection, linkage, and state-derived usage.

As a result, different Response bodies can receive the same response and item
identifiers. Storage currently retains the first object for a duplicate ID, so a
later POST can return a body that differs from the body returned by GET for that
ID. Concurrent variants make the persisted result request-order dependent. A
stateless request can also reuse the ID of an earlier stored request and appear
retrievable.

This violates immutable storage, exact retrieval, deterministic identity, and
concurrency guarantees.

### Assistant history and replay item classification

Every assistant-role input message is currently treated as an output-item replay.
The validator consequently requires a resource ID, completed status, and output
parts even for an ordinary assistant text message in a fully resent history.

The input model must distinguish easy assistant history from explicit resource
replay based on replay markers and content shape.

### Digest-fallback compatibility reporting

Compatibility reporting compares existing case declarations and compiled output
bytes. Adding a compatible fixture or case is reported as additive even though it
changes the sorted fallback candidate set and can redirect unknown prompts to a
different case.

Candidate-set changes that affect a compatible fallback class must be treated as
selection compatibility changes.

### Replay adjacency

Reasoning replay validation keeps a pending reasoning identifier but does not
enforce a strict adjacent pair. Unrelated input can intervene, another reasoning
item can overwrite the pending item, and input can end with an unmatched reasoning
item.

Reasoning and its matching assistant output must form an immediate, complete
pair.

### Fixture-authored encrypted reasoning

The renderer can emit fixture-authored opaque reasoning verbatim, while replay
validation accepts only a synthetic value derived from the reasoning item ID.
The fixture schema therefore permits output that the server cannot subsequently
accept as input.

The emitted and accepted opaque formats must use one simulator-owned policy.

### Pinned `include` behavior

The request model does not implement the pinned
`reasoning.encrypted_content` include value. Encrypted reasoning is emitted
automatically instead of being an opt-in additional field.

The include value must be typed, validated, canonicalized, and applied equally to
streamed and non-streamed responses.

### Predecessor expiry classification

The deterministic expiry scenario is evaluated before predecessor lookup. Under
that scenario, an arbitrary unknown ID is classified as expired rather than not
found.

Existence must be resolved before deterministic expiry is applied.

### Model limits

Model profiles declare context-window and maximum-output-token limits, but the
Responses route does not enforce them. The fields are currently descriptive
rather than behavioral.

### Reasoning usage accounting

Conversation continuation carries stored reasoning tokens, while stateless item
replay omits reasoning from usage accounting. Predecessor continuation can add a
parent cumulative total to already canonicalized history, double-counting prior
context.

All continuation modes need one context representation and one accounting
formula.

### Documentation state

The existing implementation plan declares overlapping milestone ranges complete
while retaining decisions described as unresolved before implementation. Resolved
contract facts need to move into maintained documentation, and obsolete planning
language needs to be removed.

## Architecture

### Separate semantic identity from resource identity

The simulator maintains two distinct identity layers:

```text
canonical semantic request
        |
        v
semantic selection and plan digest
        |
        v
response representation key
        |
        v
resource digest
        |
        v
response and item IDs, timestamps, immutable stored body
```

The semantic plan identity selects the fixture, semantic variant, output content,
and deterministic fault behavior. It continues to exclude transport and
persistence controls.

The resource identity describes the immutable Response representation. It
includes the plan digest and every effective value that changes the returned
Response object:

- `store`;
- metadata;
- `parallel_tool_calls`;
- effective reasoning summary projection;
- supported include projections;
- predecessor or conversation linkage;
- carried usage context;
- effective model and output limit;
- any other response-visible field introduced later.

The `stream` flag is excluded. Streaming and non-streaming forms of the same
request must reconstruct the same terminal object and use the same resource
identity.

The diagnostic `x-simulate-plan-digest` remains the semantic plan digest and
therefore remains stable when only representation controls change.

Storage accepts an existing resource ID only when the complete immutable Response
and stored continuation context are identical. A same-ID insertion with different
content is an explicit internal invariant violation, not a successful no-op.

### Resolve continuation context once

Responses orchestration introduces a resolved input-context value equivalent to:

```text
ResolvedResponseContext {
    turns,
    carried_reasoning_tokens,
    linkage,
}
```

Stateless history, `previous_response_id`, and conversation continuation all
produce this representation before semantic plan compilation. Input usage follows
one formula:

```text
input tokens =
    tokens(canonical semantic turns)
    + carried reasoning tokens
```

Stored responses retain cumulative carried reasoning tokens separately from
visible turns. Conversation state uses the same conceptual representation.
Stateless reasoning replay obtains its accounting metadata from a validated opaque
envelope.

### Use a versioned opaque reasoning envelope

The simulator owns the opaque reasoning envelope. It binds:

- the related resource identity;
- the deterministic reasoning-token count;
- a digest of any fixture-authored opaque material;
- a format version for future migration.

Validation interprets only the envelope framing and simulator metadata. It never
interprets, logs, or exposes authored reasoning material. Canonicalization hashes
the complete wire value as a single opaque string.

Fixture-authored opaque material contributes to the envelope digest rather than
being emitted verbatim. This makes every emitted value replayable under the same
validation policy.

## Implementation slices

### 1. Separate semantic plans from response resource identity

Suggested commit:

```text
Separate semantic plans from response resource identity
```

Affected areas:

- `src/responses.rs`;
- `src/sim/identity.rs`, or a Responses-specific identity module;
- `src/store.rs`;
- `tests/responses_api.rs`;
- `tests/determinism.rs`.

Implementation:

1. Preserve the existing semantic plan digest for selection, rendering decisions,
   and diagnostics.
2. Define a canonical Response representation key containing the plan digest and
   every effective body-shaping value.
3. Derive the resource digest, response ID, item IDs, and deterministic timestamps
   from the representation key.
4. Explicitly exclude `stream` from the representation key.
5. Make stored insertion idempotent only when both the Response body and stored
   continuation context are equal.
6. Surface same-ID/different-content insertion as an invariant failure.

Acceptance criteria:

- Metadata variants receive different IDs when their bodies differ.
- `store` variants cannot share an ID and accidentally share retrievability.
- `parallel_tool_calls` variants receive different IDs when echoed differently.
- Reasoning summary projections receive different IDs when output arrays differ.
- Include projections receive different IDs when object fields differ.
- All representation-only variants retain the same semantic plan digest when
  semantic behavior is unchanged.
- `stream: false` and `stream: true` produce the same terminal Response and
  resource identity.
- Repeating an identical stored request is byte-identical.
- Concurrent body variants cannot make POST and GET disagree.
- A unit test proves storage rejects a same-ID/different-body insertion.

### 2. Accept assistant history and enforce replay item pairs

Suggested commit:

```text
Accept assistant history and enforce replay item pairs
```

Affected areas:

- `src/responses.rs`;
- replay validation currently located in `src/http/routes.rs`;
- `tests/responses_api.rs`.

Move replay classification and validation into a typed helper owned by the
Responses domain where practical.

Rules:

- An assistant message without replay markers and with ordinary text or
  `input_text` content is assistant history.
- An ID, output part, or replay status opts an assistant item into strict resource
  replay validation.
- A reasoning replay item must be followed immediately by its matching completed
  assistant output item.
- A replay pair must use matching deterministic identity linkage.
- Duplicate replay IDs are rejected.
- Raw reasoning text is rejected.
- Partial or non-completed replay status is rejected.

Acceptance criteria:

- A fully resent multi-turn history containing ordinary assistant text succeeds
  and selects the expected semantic result.
- Assistant prefill behavior follows the pinned supported contract.
- A valid reasoning and assistant replay pair succeeds.
- Trailing reasoning fails before selection.
- Intervening user, system, developer, or assistant history fails a pending pair.
- Consecutive reasoning items fail.
- Mismatched suffixes or linkage fail.
- Duplicate IDs fail.
- Raw reasoning and partial output statuses fail.

### 3. Gate opaque reasoning behind typed include controls

Suggested commit:

```text
Gate opaque reasoning behind typed include controls
```

Affected areas:

- `src/responses.rs`;
- `src/sim/responses_stream.rs`;
- replay validation;
- the frozen Responses contract corpus;
- official JavaScript client tests.

Implementation:

1. Add a typed request include enum for pinned supported values.
2. Support `reasoning.encrypted_content`.
3. Reject unsupported known include values through the common error envelope with
   `param: "include"`.
4. Without the include, omit `encrypted_content` from response objects and stream
   events rather than serializing `null`.
5. With the include, emit the versioned simulator-owned opaque envelope.
6. Bind fixture-authored opaque material through its digest instead of emitting
   the authored value directly.
7. Canonicalize the effective include projection as part of resource identity.
8. Update the frozen reasoning request to opt into encrypted content explicitly.

Acceptance criteria:

- Default reasoning output contains no encrypted-content member.
- Requested encrypted content appears consistently in object and stream forms.
- Emitted encrypted content replays across fresh server processes.
- Fixture-authored opaque reasoning round-trips.
- Missing, malformed, altered, duplicated, and wrong-pair envelopes fail with
  deterministic errors.
- Stream reconstruction exactly matches the non-streamed terminal object.
- Official-client coverage exercises both omitted and requested include behavior.

### 4. Unify reasoning usage across continuation modes

Suggested commit:

```text
Unify reasoning usage across continuation modes
```

Affected areas:

- `src/responses.rs`;
- `src/store.rs`;
- `src/conversations.rs`;
- `src/http/routes.rs`;
- `tests/responses_api.rs`.

Implementation:

1. Replace ad hoc `prior_input_tokens` handling with
   `ResolvedResponseContext`.
2. Resolve stateless, predecessor, and conversation input into canonical turns,
   carried reasoning tokens, and linkage before compilation.
3. Store cumulative carried reasoning tokens alongside canonical turns.
4. Remove parent-total-plus-history accounting that can double-count visible
   input.
5. Extract simulator accounting metadata from validated opaque replay envelopes.
6. Calculate input usage with the single shared formula.

Acceptance criteria:

- Equivalent history represented statelessly, by predecessor, or by conversation
  selects the same semantic turn and reports equal input usage.
- Multi-generation continuation does not double-count earlier input.
- Reasoning-only parents carry nonzero reasoning usage.
- Visible history, refusals, and multipart history preserve typed framing.
- Stream terminal usage equals non-streamed usage.
- Concurrent conversation behavior remains deterministic.

### 5. Enforce Responses model token ceilings

Suggested commit:

```text
Enforce Responses model token ceilings
```

Affected areas:

- `src/http/routes.rs`;
- `src/responses.rs`;
- `src/models.rs`;
- test model fixtures;
- `tests/responses_api.rs`.

Validation order:

1. Resolve the selected model profile.
2. Reject `max_output_tokens` above the model output ceiling.
3. Resolve state and canonical context without mutation.
4. Reject input exceeding the model context window.
5. Compile the semantic plan.
6. Apply the model output ceiling when the request does not provide a smaller
   budget.
7. Validate final plan consistency before persistence or streaming.

Acceptance criteria:

- Values equal to the context and output ceilings succeed.
- Values above the ceilings return stable common-envelope errors with exact
  parameters and codes.
- Oversized stateless, predecessor, and conversation contexts fail consistently.
- A fixture larger than the effective model output ceiling becomes
  deterministically incomplete.
- Limit-validation failures do not mutate response or conversation stores.
- Streamed and non-streamed limit behavior is identical.

### 6. Classify missing predecessors before deterministic expiry

Suggested commit:

```text
Classify missing predecessors before deterministic expiry
```

Affected areas:

- `src/http/routes.rs`;
- `tests/responses_api.rs`.

Implementation:

1. Resolve the predecessor from immutable storage.
2. Return not found for an unknown or deleted predecessor under every scenario.
3. Apply deterministic expiry only to an existing stored predecessor.
4. Keep expiry evaluation read-only and deterministic.

Acceptance criteria:

- Unknown and deleted IDs return `previous_response_not_found` under every
  scenario.
- An existing predecessor returns `previous_response_expired` only under the
  expiry scenario.
- An expired predecessor remains directly retrievable according to the documented
  simulator contract.
- Repeated requests return byte-identical error bodies.

### 7. Report semantic fallback candidate-set drift

Suggested commit:

```text
Report semantic fallback candidate-set drift
```

Affected areas:

- `src/sim/artifact.rs`;
- the fixture build and compatibility CLI;
- `tests/semantic_fixtures.rs`.

The compatibility report distinguishes:

- changes to existing compiled variant bytes;
- changes to declared defaults or fallback variants;
- additive explicit outcome variants that do not alter selection;
- fixture or case candidate-set changes that can alter digest fallback;
- optional named probe-request assignment changes.

Candidate-set growth is conservatively incompatible whenever it changes a
compatible digest-fallback class. A finite probe corpus is useful diagnostics but
is not sufficient proof that modulo assignments remain unchanged. Adding only a
non-default explicit outcome variant may remain additive and compatible when it
cannot enter default fallback selection.

Acceptance criteria:

- Reordered fixture sources remain compatible.
- Adding a compatible fallback case is detected and fails compatibility checking.
- Adding an incompatible case that cannot enter the same fallback class does not
  report a false assignment change.
- Adding an explicit non-default variant is reported but remains compatible.
- Changed defaults, constraints, output bytes, and removals continue to fail.
- The report identifies the affected fallback class and candidate-set change.

### 8. Finalize the remediated Responses text contract

Suggested commit:

```text
Finalize the remediated Responses text contract
```

Affected documentation:

- `README.md`;
- `docs/spec/responses-text-scope.md`;
- `CHANGELOG.md`;
- `docs/plans/01-text-reasoning-simulation.md`;
- version files only when cutting the release.

Actions:

1. Describe semantic and resource identity in present tense.
2. Document typed include behavior and the opaque envelope policy without
   exposing internal reasoning.
3. Document continuation context, usage accounting, model limits, and expiry
   classification.
4. Regenerate the frozen contract corpus through production code and update its
   provenance.
5. Transfer maintained contract facts from the completed implementation plan into
   the Responses scope document.
6. Remove contradictory milestone and pre-implementation decision language.
7. Remove the obsolete completed implementation plan when all maintained facts
   have been transferred.
8. Record the wire corrections under `[Unreleased]`.
9. Leave unrelated future-work plan documents untouched.

Under the repository versioning policy, these changes are patch-level corrections
toward the pinned OpenAI contract while Responses remains experimental. The next
release is `1.0.1`, with `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` updated
together when the release is cut. Release notes must warn that Responses IDs and
encrypted-reasoning snapshots change.

Acceptance criteria:

- Documentation describes only implemented behavior in present tense.
- No roadmap section simultaneously claims completion and unresolved design.
- Frozen corpus cases are generated through production request handling.
- Documentation tests and repository link checks pass.
- Version metadata is internally consistent when the release is cut.

## Verification flow

After each slice:

1. Run the narrow unit and integration tests covering the changed behavior.
2. Run `cargo fmt`.
3. Run `cargo clippy --all-targets --all-features` and treat warnings as failures.
4. Run `cargo test` and any relevant all-feature test target.
5. Review snapshots and frozen-corpus diffs before accepting them.
6. Commit only after the slice is independently green.

Before merge:

- run all required Rust formatting, lint, test, and documentation gates;
- run the official Responses client contract suite;
- run determinism and concurrency tests;
- verify the default dependency tree remains free of live HTTP dependencies;
- run package and fresh-clone checks;
- compare the regenerated frozen contract corpus in full;
- verify Windows and Ubuntu CI;
- confirm that unrelated future-work documents are not included.

## Completion criteria

The remediation is complete when all eight slices are committed, every acceptance
criterion passes, the frozen contract corpus reflects production behavior, and
the maintained documentation no longer relies on the superseded implementation
plan. At that point the text and reasoning implementation can be described as
complete for its documented scope.
