# Technical documentation

The [repository README](../README.md) is the canonical product and operations guide for
`no-llm-api`. It describes the current request behavior, configuration, scenarios, fixture workflow,
live mode, containers, and verification commands.

The documents in this directory maintain the contracts that need more detail than the README:

| Document | Purpose |
| --- | --- |
| [`spec/chat-completions-scope.md`](spec/chat-completions-scope.md) | The HTTP, payload, streaming, and persistence behavior implemented by the mock. |
| [`spec/chat-completions.openapi.yaml`](spec/chat-completions.openapi.yaml) | The generated, reviewable OpenAPI extract for the implemented paths and their schemas. |
| [`spec/responses-text-scope.md`](spec/responses-text-scope.md) | The pinned, executable text/reasoning contract for the next Responses implementation slice; no Responses route is shipped yet. |
| [`spec/upstream-openapi.md`](spec/upstream-openapi.md) | Provenance, refresh, extraction, and validation of the upstream OpenAPI reference. |
| [`versioning.md`](versioning.md) | Consumer-facing compatibility rules and the release checklist. |

[`../CHANGELOG.md`](../CHANGELOG.md) is the historical record. Completed implementation plans are
not retained as product documentation; the current behavior is defined by the README, the distilled
scope, the tracked OpenAPI extract, and executable tests.
