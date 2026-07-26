# Semantic fixtures

Schema-v2 semantic fixtures author text, reasoning, and structured outcomes independently from the
legacy Chat parquet dataset. Explicit per-request selectors render these plans through the Chat
completion/SSE transports and the non-streamed Responses endpoint. Chat requests without a selector
continue to use the legacy parquet matching ladder; Responses requests use semantic matching.

Validate the built-in documents with:

```bash
cargo run --locked --bin fixtures -- lint-semantic
```

Validate another directory without loading the built-ins:

```bash
cargo run --locked --bin fixtures -- lint-semantic --input path/to/fixtures
```

Compile a reproducible artifact:

```bash
cargo run --locked --bin fixtures -- build-semantic --output data/semantic-fixtures.json
```

Given a typed Chat request saved as JSON, inspect either its redacted selection explanation or its
complete immutable plan:

```bash
cargo run --locked --bin fixtures -- explain-semantic --request request.json
cargo run --locked --bin fixtures -- snapshot-semantic --request request.json
```

All semantic commands accept `--input`; build and planning commands also accept `--models` for an
explicit model catalogue. `--case fixture-id/case-id` and `--variant variant-id` provide stable,
request-local test selection without a mutable response queue.

Each YAML document declares `schema_version: 2`, a canonical lower-kebab-case id, description,
tags, supported interfaces, matching turns, capability requirements, and one or more cases. A case
contains typed constraints and a sorted variant map. It names a required default variant and may
name an explicit fallback variant.

An outcome variant may contain a public reasoning summary, a synthetic Chat-only reasoning trace,
opaque encrypted reasoning, exact reasoning-token usage, and exactly one visible output: ordinary
answer text, refusal text, or exact JSON bytes under `structured_output.json`. The compiler keeps
those authored bytes alongside their parsed semantic value. Reasoning and structured output must be
declared in the fixture's requirements. Structured cases also name their response format.

For an explicitly selected Chat structured-output case, the authored semantic value is validated
against `response_format.json_schema.schema` before response headers or storage. Invalid schemas and
non-matching values use the common error envelope with `param: "response_format"`. Validation uses
the locked `jsonschema` crate without its network features, so schemas must be self-contained; local
definitions and references work, while remote reference retrieval is intentionally unavailable in
the offline build. Authored JSON bytes remain unchanged after validation.

The linter rejects unknown fields, duplicate keys and values, non-canonical or duplicate ids,
unsupported schema versions, missing user turns, empty outcomes, invalid default/fallback
references, undeclared interfaces or capabilities, ambiguous effort coverage, conflicting visible
outputs, YAML anchors and aliases, and unquoted date-like scalars. Files and maps are sorted before
later compilation so filesystem and map iteration order cannot affect artifacts.

The built-in examples live under `fixtures/v2/`:

- `basic-text.yaml` contains a shared Chat/Responses text outcome;
- `reasoning-effort.yaml` contains low, medium, and high authored effort variants;
- `multi-turn-correction.yaml` requires a complete ordered conversation before selecting its reply;
- `structured-output.yaml` contains a strict JSON-shaped semantic value.
- `legacy-*.yaml` documents byte-preserving imports of representative markdown, reasoning, and
  structured-output conversations from the shipped Chat fixtures.

The legacy files directly under `fixtures/` remain the source for the shipped Chat dataset. Use
`fixtures lint` and `fixtures build` for them; never send schema-v2 files through the legacy parquet
builder.

## Minimal Responses fixture

Start a Responses-only case with an exact authored outcome:

```yaml
schema_version: 2
id: release-note
description: One stable Responses answer used by a client test.
tags: [text]
interfaces: [responses]
match:
  models: [mock-gpt-4o]
  turns:
    - role: user
      text: Write the release note.
cases:
  - id: concise
    constraints:
      interfaces: [responses]
    default_variant: default
    variants:
      default:
        answer: The release is ready.
    terminal: completed
```

Place it under `fixtures/v2/`, lint it, and send the exact matching request. The selector is useful
while developing a case and remains a stable test control when several cases share similar turns:

```bash
cargo run --locked --bin fixtures -- lint-semantic

curl http://127.0.0.1:8080/v1/responses \
  -H 'Content-Type: application/json' \
  -H 'X-Simulate-Case: release-note/concise' \
  -d '{"model":"mock-gpt-4o","input":"Write the release note."}'
```

Add `reasoning_summary`, `reasoning_tokens`, and the fixture-level `requirements.reasoning: true`
for public reasoning. Use `structured_output.json` plus
`requirements.structured_output: true` for byte-exact JSON. Do not put private chain-of-thought in
`reasoning_summary`; `reasoning_trace` exists only for the legacy Chat compatibility path and is
never exposed by Responses.

## Migration and compatibility checklist

1. Leave legacy YAML/parquet fixtures unchanged unless the Chat wire contract intentionally changes.
2. Use `interfaces: [responses]` for a new Responses-only case; declare both interfaces only when
   the same authored outcome is intentionally shared.
3. Keep fixture, case, and variant ids stable after consumers snapshot them. Add a variant instead
   of renaming one, and keep `default_variant` explicit.
4. Preserve authored structured JSON bytes. Changing whitespace or key order changes streamed and
   non-streamed snapshots even when the parsed value is equal.
5. Run the semantic linter, artifact reproducibility tests, Responses contract/API tests, and the
   official-client suite before accepting a fixture migration.
6. Review response ids, output indexes, SSE ordering, usage, and terminal state together. A fixture
   change is not text-only when any of those observable values move.

Schema-v2 corpus growth cannot alter exact case/variant selection. Digest fallback is revisioned and
may change only with an explicit dataset revision and reviewed compatibility report. New output-item
types append through the typed item/stage boundary; they must not renumber existing message or
reasoning items in an established case.

## Semantic compiler kernel

`sim::canonical` converts a typed Chat request into a stable protocol-neutral representation. It
retains role boundaries, text versus content parts, tool linkage, response format, reasoning effort,
limits, storage, streaming, and seed controls. Objects are recursively key-sorted and every digest
uses the repository's framed FNV-1a implementation with a named version domain.

`sim::plan` selects a compatible fixture and case, resolves an exact effort variant or declared
fallback, validates model capabilities, and produces immutable reasoning, text, refusal, or
structured output nodes. Structured nodes contain both the authored JSON bytes and parsed value.
Dataset, scenario, model-profile, and simulation-seed revisions participate in plan identity.

The plan explanation exposes fixture/case/variant selection, effective revision controls, request
digest, and redacted turn shapes. It reports content kinds and byte lengths but never prompt or
reasoning text. The fixture CLI exposes planning directly; the HTTP surface invokes the same planner
when `X-Simulate-Case` or `x_simulate.case` is present.

`sim::artifact` sorts every unordered input, rejects overlapping equal-priority cases and unknown
or incapable models, tokenizes reasoning and visible output independently, and emits compiler,
selection, tokenizer, and source revisions. The resulting JSON bytes reproduce across shuffled
source enumeration and fresh compiler processes.
