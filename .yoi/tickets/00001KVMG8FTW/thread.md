<!-- event: create author: LocalTicketBackend at: 2026-06-21T07:10:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T07:15:41Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T07:17:14Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket body は `host_api.https` 廃止、`host_api.request` 統一、manifest-declared URL/request target permissions、enablement grant 照合、runtime fail-closed、local/private target 明示 grant、broad/arbitrary URL 表示、docs/tests/diagnostics 更新まで具体化されている。
- `readiness: implementation_ready` で、relations / orchestration plan に blocker はない。
- Current active implementation `00001KVMFFYVX` は Workspace web control plane bootstrap で、主対象は backend/frontend/store/Nix packaging。This Ticket の主対象は plugin manifest/pod runtime/plugin CLI/docs/tests で直接の semantic blocker はない。過去のユーザー指示「blocker無いなら並列に」に従い、並列実装可能と判断する。
- Orchestrator worktree is clean on `orchestration` at `f164483e` で、対象 Ticket 用 worktree / branch は未作成。
- Visible Pods に対象 Ticket の child Pod は存在しない。

Evidence checked:
- Ticket body / thread / artifacts via `TicketShow` and direct `item.md` read。
- `TicketRelationQuery(00001KVMG8FTW)`: no relations / blockers。
- `TicketOrchestrationPlanQuery(00001KVMG8FTW)`: no records。
- `ListPods`: active child is only `yoi-coder-00001KVMFFYVX`; no child for this Ticket。
- Orchestrator git state / worktree list / branch list checked from `/home/hare/Projects/yoi/.worktree/orchestration` only。
- Bounded code map:
  - `crates/manifest/src/plugin.rs`: `PluginGrantConfig.https`, `PluginHttpsGrant`, `PluginHostApi::Https`, permission/grant resolution/tests。
  - `crates/pod/src/feature/plugin.rs`: `PluginHttps*` runtime request path, `yoi:host/https@1.0.0` / raw wasm `yoi:https` imports, URL validation, request bounds, credential header checks, public-IP guard, allowlist checks, plugin tests。
  - `crates/yoi/src/plugin_cli.rs`: inspection formatting for configured HTTPS grants。
  - `docs/development/plugin-development.md`: active `host_api.https` / `grants.https` docs。

IntentPacket:

Intent:
- Replace public/model/config-facing `host_api.https` with URL-permission based one-shot `host_api.request`.
- Keep existing safe outbound request behavior where applicable, but generalize schemes/targets so explicit manifest + enablement grants can authorize loopback/private/local targets.
- Keep WebSocket / SSE / persistent connections out of `request`.

Binding decisions / invariants:
- Do not add backward compatibility aliases for `host_api.https`, `PluginHttps*`, or `grants.https` in active APIs unless explicitly escalated and reapproved。
- Model/config-facing naming must be `request`; internal names should also avoid `PluginHttps*` unless truly private transitional code is justified and not exposed。
- Runtime authorization requires both manifest-declared request target permission and enablement grant for that target。
- Grant-only without manifest request must fail closed or be explicitly diagnosed as unsafe/unused override; do not silently expand authority。
- Requested-but-ungranted target must fail closed before network I/O。
- Localhost/loopback/private/local targets are not ambient; they require manifest declaration and enablement grant。
- Arbitrary URL / broad network access must be visibly distinguished from normal target grants in inspection/diagnostics。
- Embedded credentials, credential-like headers, request/response bounds, external-content untrusted treatment, and no hidden context injection remain mandatory。
- WebSocket URL / upgrade / persistent stream must be rejected or explicitly unsupported by `request`。
- Existing HTTPS request use cases must continue under `host_api.request` with explicit request permission/grant。

Requirements / acceptance criteria:
- Active API naming uses `host_api.request` / request grant naming。
- Plugin manifest statically declares request target permissions readable from manifest alone。
- Enablement config grants request targets and is matched against manifest-declared targets。
- Runtime checks method/scheme/host/port/path prefix against declared+granted URL permission。
- `http://localhost` / loopback request can be allowed only with explicit declaration+grant。
- Existing public HTTPS use case works as request。
- Broad/arbitrary URL is supported only with clear broad display/diagnostic if implemented。
- `yoi plugin show` / static inspection distinguishes requested, granted, denied/missing, and broad request permissions。
- Docs/templates/tests/diagnostics are updated to request naming and WebSocket separate-capability policy。

Implementation latitude:
- Exact Rust/TOML type names are up to Coder, but active names should be request-oriented, e.g. `PluginRequestGrant`, `PluginRequestTarget`, `host_api.request`.
- Regex support is optional. If added, it must include review-readable normalized display/warning/label and tests for broad/opaque handling。
- Request target schema may start with exact scheme/host/optional port/method/path prefix. Keep permission review human-readable。
- Internal runtime can reuse/refactor existing HTTPS client/request code, but reviewer should see active API renaming and policy changes。
- Raw wasm/component import migration may choose new import names with tests; if keeping an internal compatibility import is unavoidable, escalate before committing.

Escalate if:
- Compatibility alias for old `host_api.https` / `grants.https` seems required。
- Local/private target policy would open without both manifest declaration and grant。
- Arbitrary URL access becomes visually indistinguishable from normal grants。
- WebSocket/SSE/daemon lifecycle begins to enter `request`。
- Secret-bearing headers/env/config would flow from guest memory without explicit SecretRef/grant design。
- Regex support becomes opaque or hard to review。
- Parallel active `00001KVMFFYVX` work creates unavoidable `Cargo.lock` / `package.nix` conflict; report rather than broad-rebasing alone。

Validation plan:
- `cargo fmt --check`
- Focused manifest plugin tests for request permission parsing/resolution and naming removal。
- Focused pod plugin tests for grant allow/deny, requested-but-ungranted, granted-but-unrequested, loopback/local allow/deny, WebSocket/upgrade rejection, request/response bounds/credential-header safety。
- Focused yoi plugin CLI tests for request permission/grant/broad/missing diagnostics。
- Docs/template grep ensuring active `host_api.https` / `grants.https` / `PluginHttps*` naming is removed except historical Ticket text or explicitly justified migration notes。
- `cargo check` for affected crates, `git diff --check`, `yoi ticket doctor`。

Reviewer focus:
- Breaking-change intent is real: no stale active docs/templates/CLI inspection still teaching `host_api.https`。
- Authorization must be two-stage: manifest request + grant。
- Local/private network must not become ambient。
- Old public HTTPS use case must still work under request naming。
- External content and hidden context constraints remain intact。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T07:17:18Z from: queued to: inprogress reason: human_authorized_unblocked_parallel_implementation field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete acceptance criteria and no recorded blockers; active Workspace web control plane implementation is semantically separate enough for parallel implementation. Orchestrator accepts this Plugin host API refactor into implementation.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T07:18:48Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVMG8FTW-plugin-request-host-api`
- Created branch:
  - `impl/00001KVMG8FTW-plugin-request-host-api`
- Base commit:
  - `4cd4a06e ticket: route plugin request capabilities`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVMG8FTW`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVMG8FTW-plugin-request-host-api`

Parallelization note:
- `00001KVMFFYVX` remains in progress in a separate child worktree. Coder was instructed to avoid unnecessary Cargo.lock/package.nix churn and report if unavoidable conflicts appear。

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff. Orchestrator will not merge/close until reviewer approval and validation evidence are available。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T07:48:07Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVMG8FTW`.

Implementation commit:
- `962b7699 plugin: replace https host api with request grants`

Changed areas:
- `crates/manifest/src/plugin.rs`
  - Replaced active host API/grant naming with `request`。
  - Added manifest-declared `[[request]]` URL targets with `scheme`, `host`, optional `port`, `methods`, and `path_prefixes`。
  - Added request grant parsing/labels/broad diagnostics and tests that legacy HTTPS-facing names are not accepted。
- `crates/pod/src/feature/plugin.rs`
  - Renamed runtime path/imports to request-oriented naming。
  - Runtime now requires both manifest request target declaration and enabled request grant before network I/O。
  - Checks scheme/host/port/path/method, rejects embedded credentials, credential-like headers, WebSocket URLs/upgrades, and SSE/event-stream requests。
  - Allows loopback/local/private targets only when explicitly declared and granted。
  - Adds static inspection entries for requested/granted/missing/grant-only/broad request targets。
- `crates/yoi/src/plugin_cli.rs`
  - Shows configured request grants and request target eligibility diagnostics, including broad/missing/grant-only states。
- `resources/plugin/wit/*`
  - Updated WIT import/interface from HTTPS host API to `yoi:host/request@1.0.0`。
- `docs/development/plugin-development.md`
  - Rewrote host API docs for `request`, including manifest targets, enablement grants, loopback policy, broad target diagnostics, and WebSocket/SSE exclusion。
- `docs/design/plugin-component-model.md`
  - Updated design wording/import references。
- `crates/pod/src/pod.rs`
  - Updated test fixture initialization for the new manifest request target field。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p manifest request`: passed
- `cargo test -p pod feature::plugin::tests --lib`: passed
- `cargo test -p yoi plugin_cli::tests`: passed
- `cargo check -p manifest -p pod -p yoi`: passed
- `git diff --check`: passed
- Active naming grep for `host_api.https|grants.https|PluginHttps|yoi:host/https|yoi:https` under `crates docs resources`: no matches
- `target/debug/yoi ticket doctor`: passed

Coder dependency/package report:
- No `Cargo.lock`, `package.nix`, or flake changes。
- No dependency/package churn。

Known deferrals:
- Regex URL target matching was not added。
- WebSocket/SSE/persistent connection support remains explicitly outside `host_api.request`。

Orchestrator evidence checked:
- Implementation worktree is clean on `impl/00001KVMG8FTW-plugin-request-host-api`。
- HEAD is `962b7699`。
- Diff `4cd4a06e..HEAD` touches 9 expected manifest/pod/plugin-doc/WIT files, about 768 insertions / 448 deletions。
- Active stale naming grep under `crates docs resources` returned no matches。
- `git diff --check 4cd4a06e..HEAD` produced no diagnostics。

Next action:
- Route to read-only Reviewer Pod for acceptance review against the Ticket and IntentPacket。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-21T07:48:53Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVMG8FTW-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVMG8FTW-plugin-request-host-api`
- Branch: `impl/00001KVMG8FTW-plugin-request-host-api`
- Baseline: `4cd4a06e`
- Implementation commit: `962b7699 plugin: replace https host api with request grants`
- Diff: `4cd4a06e..962b7699`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Active API/docs/WIT/CLI naming uses request, not stale https names。
- No compatibility alias remains for old `host_api.https` / `grants.https` unless explicitly justified。
- Runtime authorization requires both manifest request target and enablement grant before network I/O。
- Grant-only and missing-grant cases fail closed / diagnose clearly。
- Local/private/loopback targets require explicit declaration and grant。
- WebSocket/SSE/persistent stream behavior is rejected or explicitly unsupported by `request`。
- Broad/arbitrary URL grants are visibly distinguished。
- Existing public HTTPS use case still works through request naming。

Orchestrator will wait for reviewer verdict before integration。

---
