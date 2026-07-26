# Upstream OpenAPI specification

The gitignored `openapi.yaml` at the repository root is a fetched copy of the OpenAI REST API
specification from [openai/openai-openapi](https://github.com/openai/openai-openapi). It is upstream
reference material, not a description of this mock. The implemented behavior lives in
[`chat-completions-scope.md`](chat-completions-scope.md), and the reviewable generated subset lives in
[`chat-completions.openapi.yaml`](chat-completions.openapi.yaml).

## Refreshing it

```powershell
scripts\fetch-openapi.ps1           # fetch and record provenance
scripts\fetch-openapi.ps1 -Check    # report staleness only, exit 1 if stale
```

```bash
scripts/fetch-openapi.sh            # same, POSIX
scripts/fetch-openapi.sh --check
REF=<sha> scripts/fetch-openapi.sh  # pin to a specific upstream commit
```

Each fetch rewrites `openapi.provenance.json` with the upstream commit sha, its
date, the fetch time and the file's sha256. Read that file to find out how old
the local copy is.

## Why the commit sha is the version

Upstream leaves `info.version` at `2.3.0` across unrelated changes, and the
repository has only two tags (`1.3.0`, `2.0.0`) that do not track the spec's
evolution. `info.version` is therefore useless as a staleness signal - two copies
nine months apart both claim `2.3.0`. Use `upstream_commit` and `sha256`.

Upstream's default branch is `main`. `raw.githubusercontent.com` still serves a
legacy `master` alias, but the commits API returns 404 for it, so both scripts
default to `main`.

## What is tracked, and what is fetched

| File | Tracked | What it is |
| --- | --- | --- |
| `docs/spec/chat-completions.openapi.yaml` | yes | The pruned extract: the five paths this mock implements plus their transitively referenced schemas, 144 KB. Generated, never hand-edited. |
| `openapi.provenance.json` | yes | URL, upstream commit, date and sha256 of the full document the extract came from. |
| `openapi.yaml` | no | The full 2.7 MB upstream document. Fetch it with `scripts/fetch-openapi.ps1` (or `.sh`) when you need to grep the whole surface. |

Regenerate the extract after every refresh:

```bash
pwsh scripts/fetch-openapi.ps1          # or: bash scripts/fetch-openapi.sh
cargo run --bin spec_extract            # rewrites the tracked extract
cargo test --test spec                  # asserts it is self-contained
```

`tests/spec.rs` fails if the extract loses a path or cited schema, references a schema it does not
contain, disagrees with the recorded provenance, turns the normalized `i64` seed bounds into
strings, leaves implemented-surface citations aimed at the untracked document, or grows beyond
150 KiB. That is what keeps the pruned copy trustworthy.

The full upstream file stays untracked because it is large and changes outside this project. The
provenance record and generated extract make upstream changes explicit, reviewable, and reproducible
without treating the entire external specification as repository-owned product documentation.
