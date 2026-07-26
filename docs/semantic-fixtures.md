# Semantic fixtures

Schema-v2 semantic fixtures author text, reasoning, and structured outcomes independently from the
legacy Chat parquet dataset. They are an implementation input for the future semantic compiler and
do not affect runtime selection yet.

Validate the built-in documents with:

```bash
cargo run --locked --bin fixtures -- lint-semantic
```

Validate another directory without loading the built-ins:

```bash
cargo run --locked --bin fixtures -- lint-semantic --input path/to/fixtures
```

Each YAML document declares `schema_version: 2`, a canonical lower-kebab-case id, description,
tags, supported interfaces, matching turns, capability requirements, and one or more cases. A case
contains typed constraints and a sorted variant map. It names a required default variant and may
name an explicit fallback variant.

An outcome variant may contain a public reasoning summary, a synthetic Chat-only reasoning trace,
opaque encrypted reasoning, exact reasoning-token usage, and exactly one visible output: ordinary
answer text, refusal text, or a structured JSON value. Reasoning and structured output must be
declared in the fixture's requirements. Structured cases also name their response format.

The linter rejects unknown fields, duplicate keys and values, non-canonical or duplicate ids,
unsupported schema versions, missing user turns, empty outcomes, invalid default/fallback
references, undeclared interfaces or capabilities, ambiguous effort coverage, conflicting visible
outputs, YAML anchors and aliases, and unquoted date-like scalars. Files and maps are sorted before
later compilation so filesystem and map iteration order cannot affect artifacts.

The built-in examples live under `fixtures/v2/`:

- `basic-text.yaml` contains a shared Chat/Responses text outcome;
- `reasoning-effort.yaml` contains low, medium, and high authored effort variants;
- `structured-output.yaml` contains a strict JSON-shaped semantic value.

The legacy files directly under `fixtures/` remain the source for the shipped Chat dataset. Use
`fixtures lint` and `fixtures build` for them; never send schema-v2 files through the legacy parquet
builder.
