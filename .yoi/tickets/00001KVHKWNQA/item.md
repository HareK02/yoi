---
title: 'Plugin: add Rust PDK and embedded authoring templates for Component Model Tools'
state: 'closed'
created_at: '2026-06-20T04:16:14Z'
updated_at: '2026-06-20T05:53:20Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'pdk', 'component-model', 'authoring', 'templates', 'sdk', 'no-crates-io']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T04:52:58Z'
---

## Background

Yoi can now discover Plugin packages, register Tool surfaces, execute sandboxed WASM/Component Model Tool Plugins, enforce Plugin grants, expose `https` / `fs` host APIs, and inspect Plugin state through CLI. The next gap is authoring: independent Plugin developers should not need to write raw WIT/ABI glue or copy ad-hoc examples by hand.

This Ticket adds a first-party Rust PDK for Component Model Tool Plugins and embeds a starter template into Yoi resources. It deliberately does not publish to crates.io yet and does not fetch remote templates. The PDK/API and WIT world are still young, so out-of-tree authors should initially use a git rev or generated local path dependency rather than depending on a public semver crate.

## Requirements

- Add a Rust PDK crate inside the Yoi workspace.
  - Suggested package name: `yoi-plugin-pdk`.
  - Suggested path: `crates/plugin-pdk`.
  - Target Component Model Tool Plugins, not raw core-Wasm as the primary authoring path.
- PDK must be guest-side only.
  - Do not depend on host runtime crates such as `pod`, `llm-worker`, `tui`, or `client`.
  - Keep dependencies minimal, e.g. `serde`, `serde_json`, and WIT binding support.
  - Do not expose ambient fs/network/env authority.
- Provide ergonomic Tool helpers.
  - Typed JSON input parsing.
  - Typed JSON output serialization.
  - Structured `ToolError` / error-code helpers.
  - `ToolContext` containing at least the tool name.
  - Helper equivalent to `run_json_tool` that returns the ToolOutput JSON expected by the Yoi runtime.
- Provide Component Model binding glue.
  - Re-export or wrap generated WIT bindings enough that an author does not need to hand-write raw pointer/length ABI code.
  - Prefer a minimal `Guest` impl helper first; add a macro only if it is simpler and testable.
- Include embedded authoring templates under runtime resources.
  - Suggested path: `resources/plugin/templates/rust-component-tool/`.
  - Template includes `Cargo.toml`, `src/lib.rs`, `plugin.toml`, and a short README.
  - Template should be usable by a future `yoi plugin new` command without network access.
- Template dependency policy:
  - In checkout/dev mode, template may use a local path dependency to `crates/plugin-pdk`.
  - For out-of-tree authors, docs/template comments should show a git `rev` dependency pattern.
  - Do not publish or require crates.io for this Ticket.
  - Do not use `curl | sh` or remote template fetch.
- Include at least one example or fixture Plugin using the PDK.
  - Echo-style Component Model Tool is enough.
  - It should compile to a component artifact through the documented toolchain or, if full component build automation is not yet available, be covered by a clear fixture/test boundary.
- Update Plugin development docs.
  - Explain that Component Model + PDK is the preferred authoring path.
  - Explain that raw core-Wasm ABI is compatibility/transitional.
  - Explain why crates.io publication and remote templates are intentionally deferred.

## Acceptance criteria

- Workspace contains a guest-side `yoi-plugin-pdk` crate.
- A Rust Component Model Tool Plugin can use the PDK to implement a JSON Tool without raw pointer/length ABI code.
- PDK-produced Tool success output is accepted by the existing Yoi Plugin Tool runtime.
- PDK-produced Tool errors are bounded and represented as ordinary Tool result/error content.
- PDK does not grant or imply authority; host-side Plugin grants still decide `https` / `fs` / Tool execution access.
- Embedded `rust-component-tool` template exists in runtime resources and is suitable for future `yoi plugin new` expansion.
- No crates.io publication is required or performed.
- No remote template fetch is implemented.
- Tests cover:
  - PDK JSON input/output happy path;
  - PDK error output path;
  - PDK sample/template compiles or fixture is validated;
  - sample Plugin can be executed by Yoi component runtime if feasible in current test infrastructure;
  - PDK has no host-runtime crate dependency.
- Validation: focused PDK/plugin tests, `cargo fmt --check`, relevant `cargo check` / `cargo test`, `git diff --check`, and `nix build .#yoi` because workspace/package/resources may change.

## Non-goals

- Publishing `yoi-plugin-pdk` to crates.io.
- Remote template fetch or `curl | sh` setup.
- `yoi plugin new/check/pack` CLI implementation.
- Multi-language PDKs.
- Service / Ingress authoring.
- WebSocket / inbound HTTP bridge support.
- Replacing Plugin grants with PDK-side checks.

## Related work

- `00001KVG0HR9M` — Objective: Plugin platform roadmap.
- `00001KVG0HR96` — Plugin Component Model runtime.
- `00001KVFD3YSV` — Plugin read-only CLI inspection.
- `00001KVFDX9AF` — Plugin https host API.
- `00001KVFDX9AY` — Plugin fs host API.
- `docs/development/plugin-development.md` — current Plugin development guide.
