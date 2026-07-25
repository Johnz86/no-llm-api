# Versioning

`no-llm-api` is a test double. What consumers depend on is not a Rust API but the bytes
on the wire and the switches that shape them, so the version communicates those.

## What each part means

| Change | Bump |
| --- | --- |
| The bytes a client receives change: a field appears or disappears, frame ordering changes, a status or error code changes | minor while `0.x`, major from `1.0` |
| A flag or environment variable is removed or its default changes | minor while `0.x`, major from `1.0` |
| A scenario's pacing or fault behaviour changes | minor |
| A new endpoint, flag, scenario or fixture is added without altering existing output | minor |
| A bug fix that makes output match the OpenAI spec more closely | patch, and always listed under **Wire behaviour** |
| Internal refactoring, docs, tests, dependency bumps with no observable change | patch |

While the version is `0.x`, the minor number carries breaking changes. That is the
honest signal for a project whose wire shape is still converging.

## How a consumer pins behaviour

1. Pin the image tag or the crate version.
2. Read `x-no-llm-api-version` from any response, or `GET /health`, to assert which
   build answered - useful when a CI job talks to a container it did not start.
3. Snapshot the transcripts you depend on. `tests/snapshots/` in this repository is the
   same technique: unredacted, so a diff means the wire changed.

## Release checklist

1. `cargo test` and `cargo clippy --all-targets -- -D warnings` on both the default and
   `live` feature sets.
2. Move `CHANGELOG.md`'s `[Unreleased]` section under the new version, keeping the
   **Wire behaviour** subsection first.
3. Bump `version` in `Cargo.toml`; `x-no-llm-api-version`, `/health` and `/ready` read
   it from `CARGO_PKG_VERSION`, so nothing else needs editing.
4. Tag `vX.Y.Z`. The release workflow builds the five target binaries and publishes the
   container image to GHCR.
