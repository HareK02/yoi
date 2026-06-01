# 夜居 / Yoi agent

Yoi is an agent runtime for building, running, and orchestrating LLM Pods while preserving explicit history, scoped capabilities, and developer-controlled workflows.

Runtime surfaces use `yoi`, `.yoi`, `~/.yoi`, and `YOI_*`.

## Documentation map

Start with [`docs/README.md`](docs/README.md). The repository documentation is intentionally small: it should explain current design intent and development workflow, not preserve every old plan or external research note.

Important entry points:

- [`docs/design/overview.md`](docs/design/overview.md) — architecture and crate ownership map.
- [`docs/design/context-history.md`](docs/design/context-history.md) — the rules for history, context, and prompt-cache-safe inputs.
- [`docs/design/pod-session-state.md`](docs/design/pod-session-state.md) — why Pod metadata, session logs, and live runtime hints are separate.
- [`docs/development/work-items.md`](docs/development/work-items.md) — work item and ticket workflow.
- [`docs/development/validation.md`](docs/development/validation.md) — validation expectations.

## Development

This repository dogfoods Yoi to develop Yoi. Work is tracked through `work-items/` and `./tickets.sh`; git history plus work item files are the authoritative record of state changes.

Common local checks:

```sh
./tickets.sh doctor
git diff --check
cargo test --workspace
```

E2E testing with real spawned processes is not yet designed. Avoid adding compatibility layers or broad formatting churn only to reduce short-term edit size; prefer clear boundaries and type safety.
