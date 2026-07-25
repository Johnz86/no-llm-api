# Upstream OpenAPI specification

`openapi.yaml` at the repository root is a verbatim copy of the OpenAI REST API
specification from [openai/openai-openapi](https://github.com/openai/openai-openapi).
It is reference material, not a description of this mock. What this mock actually
implements is in [`chat-completions-scope.md`](chat-completions-scope.md).

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

## Historical note: the two files this replaced

Until 2026-07-25 the repository carried two stale October 2025 snapshots with
confusing lineage. Both are now gone.

| File | What it was | Fate |
| --- | --- | --- |
| `openapi.yaml` (1.31 MB, OpenAPI 3.0.0, 99 paths) | the old codegen-oriented 3.0.0 down-conversion that upstream no longer publishes; no `gpt-5`, no `/chatkit`, no `/videos` | replaced by the current 3.1.0 spec |
| `openapi.documented.yml` (2.21 MB, OpenAPI 3.1.0, 129 paths) | the richer documented variant, since renamed upstream to plain `openapi.yaml` | deleted; every plan citation into it was re-anchored to the refreshed `openapi.yaml` first |

Neither was unusable: on the Chat Completions surface specifically they were one
or two fields behind (`moderation`, `verbosity`). The real problem was that
nothing recorded where they came from or when, and the tracked one was the worse
of the two.

Line numbers cited in `docs/plans/*` were re-anchored to upstream commit
`5c044be3bf3a`. Refreshing the spec shifts them again, so each citation also
names the schema or path it points at - prefer the name. Roadmap item R46 still
proposes replacing this 2.7 MB copy with a pruned chat-completions extract, which
would make the citations stable.
