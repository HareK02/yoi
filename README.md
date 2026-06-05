# 夜居 / Yoi agent

Yoi is an agent runtime for building, running, and orchestrating LLM Pods while preserving explicit history, scoped capabilities, and developer-controlled workflows.

## 1. Yoi agent

Yoi focuses on long-running agent operation rather than one-off prompt execution. A named Pod can keep durable session history, run with explicit tool and filesystem authority, delegate bounded work to child Pods, and be inspected or restored through CLI/TUI surfaces.

Main highlights:

- Named long-running **Pods** with durable session and metadata records.
- Explicit tool permissions and filesystem scopes.
- Multi-agent orchestration with scoped coder/reviewer Pods.
- Profile, Manifest, and prompt-based runtime configuration.
- Local Tickets and workflow files for auditable project coordination.
- TUI and CLI entry points, including a multi-Pod dashboard.

Yoi is actively dogfooded in this repository. Public APIs, configuration formats, and workflows may still change.

## 2. Quick Install

From source:

```sh
cargo build --release -p yoi
./target/release/yoi --help
```

With Nix:

```sh
nix build .#yoi
./result/bin/yoi --help
```

## 3. Getting Started

```sh
yoi --help
yoi
yoi --multi
yoi --pod <name>
yoi pod --help
```

Typical flow:

1. Configure providers, models, profiles, prompts, and scopes.
2. Start or attach to a named Pod from the CLI/TUI.
3. Use explicit tools and scoped delegation for multi-agent work.
4. Record project work through Tickets, workflow files, and git history.

Runtime surfaces use `yoi`, `.yoi`, `~/.yoi`, and `YOI_*`.

## 4. Documentation

Start with [`docs/README.md`](docs/README.md).

Key docs:

- [`docs/design/overview.md`](docs/design/overview.md) — architecture and crate ownership map.
- [`docs/design/context-history.md`](docs/design/context-history.md) — history/context invariants.
- [`docs/design/pod-session-state.md`](docs/design/pod-session-state.md) — Pod identity, metadata, and session logs.
- [`docs/design/profiles-manifests-prompts.md`](docs/design/profiles-manifests-prompts.md) — Profiles, Manifests, and prompt resources.
- [`docs/design/tool-permissions-scope.md`](docs/design/tool-permissions-scope.md) — tool policy and filesystem scope.
- [`docs/development/work-items.md`](docs/development/work-items.md) — Ticket workflow and project records.
- [`docs/development/validation.md`](docs/development/validation.md) — validation expectations.

## 5. Development

This repository dogfoods Yoi to develop Yoi. Work is tracked through `.yoi/tickets/` and `yoi ticket ...`; git history plus Ticket files are the authoritative project record.

Common checks:

```sh
yoi ticket doctor
git diff --check
cargo fmt --check
cargo check --workspace --all-targets
cargo test --workspace
```

Optional Nix validation:

```sh
nix build .#yoi --no-link
```

E2E testing with real spawned processes is not yet designed. Keep changes scoped, preserve durable authority boundaries, and prefer clear type-safe structure over short-term compatibility layers.

License: MIT. See [`LICENSE`](LICENSE).
