# docs

Planning documents for making `no-llm-api` battle ready as a deterministic mock OpenAI Chat
Completions backend for chat-GUI testing.

**Start here: [`plans/00-roadmap.md`](plans/00-roadmap.md).** It is the single entry point - a
deduplicated work-item table across all five plans, ordered milestones with runnable exit criteria,
the first three commits, non-goals, and the open decisions where the plans disagree. Only open a
numbered plan when you need the evidence behind a roadmap row.

| Document | Question it answers |
| --- | --- |
| [`plans/00-roadmap.md`](plans/00-roadmap.md) | What do we do, in what order, and what is already decided? Merges 01-05 and resolves their conflicts. |
| [`plans/01-api-surface-gap-analysis.md`](plans/01-api-surface-gap-analysis.md) | Where does our wire shape differ from the OpenAI spec and `async-openai` 0.41.1? Field-by-field request/response/chunk diff, error-envelope gaps, missing endpoints, and the two hard client-parse breaks. |
| [`plans/02-simulation-engine.md`](plans/02-simulation-engine.md) | How do we make the fake LLM deterministic and expressive? Selection/normalization rules, multi-turn matching, the behaviour matrix, fault and timing injection, fixture authoring, and correct tokenization. |
| [`plans/03-chat-gui-compatibility.md`](plans/03-chat-gui-compatibility.md) | What do real clients actually require? Per-client wire matrix (Open WebUI, LibreChat, Jan, Vercel AI SDK, openai-js), CORS and SSE constraints, auth design, model catalogue, cancel semantics, and a runnable smoke test. |
| [`plans/04-testing-and-ci.md`](plans/04-testing-and-ci.md) | How do we prove any of it? Honest coverage audit, test pyramid and layout convention, SSE assertion harness, snapshot determinism, fixture drift, and the GitHub Actions workflow. |
| [`plans/05-operability-and-packaging.md`](plans/05-operability-and-packaging.md) | How is it configured, observed, shipped and maintained? CLI/config overhaul, scenario bundles, logging and health, Dockerfile and compose, live-mode hardening, repo hygiene, and versioning policy. |

All five plans are planning-only and were written against the tree as of 2026-07; every claim carries
a `file:line` or schema-name citation. Where a citation points at `openapi.documented.yml`, note that
the file is untracked (see roadmap decision D10).
