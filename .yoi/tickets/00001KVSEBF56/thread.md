<!-- event: create author: "yoi ticket" at: 2026-06-23T05:13:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-23T05:14:09Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-23T05:14:09Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-23T05:40:01Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-23T05:41:02Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket body has concrete requirements for protocol DTO feature-splitting, generated TypeScript types, generated-output drift check, and Workspace web integration stance。
- No blocking relations / orchestration plan records exist。
- Current queued Ticket is this Ticket only。
- Orchestrator worktree is clean on `orchestration` at `615c0250`; target worktree / branch is not present。
- Existing code map confirms `crates/protocol` currently has `tokio` dependency and `stream.rs` enabled unconditionally, while frontend has local hand-written types for Workspace entities but no generated protocol types yet。

IntentPacket:

Intent:
- Make `crates/protocol` the Rust authority for Pod wire DTOs and generate Workspace web TypeScript types from those DTOs instead of hand-writing protocol mirror types。

Binding decisions / invariants:
- DTO authority remains Rust `crates/protocol`; do not create a separate web-only protocol authority。
- Browser must not connect directly to Pod Unix sockets。
- Backend proxy may be design-recorded or minimally scaffolded, but must preserve Worker identity resolution and method allow/block boundary; do not implement broad Worker operation UI in this Ticket。
- `stream.rs` / JSONL tokio transport should be optional behind a default `stream` feature; DTO-only crate should compile without tokio for wasm/no-default-features。
- Generated TypeScript must reflect serde wire shape for tagged enums / rename conventions as closely as practical。
- Generated artifacts should be committed only if they are intended import artifacts and have a drift check。
- Avoid broad protocol redesign or permission-model invention。

Requirements / acceptance criteria:
- `cargo check -p protocol --target wasm32-unknown-unknown --no-default-features` passes, or if target installation is unavailable in environment, the code is structured correctly and validation limitation is documented。
- Protocol root types at least include `Method`, `Event`, `Segment`, and related DTOs needed by Worker operation UI / stream display。
- Workspace web can import generated protocol TypeScript types from a generated directory/file。
- Generated-output drift check exists, e.g. cargo test/generator check that fails if committed generated TS is stale。
- `cargo test -p protocol` passes。
- Deno check/build passes。
- `git diff --check` and `nix build .#yoi --no-link` pass if dependencies/package hash changed。
- Backend proxy stance is represented in implementation or design notes without direct browser-to-Pod socket exposure。

Implementation latitude:
- Use `ts-rs` if it fits; otherwise choose a small generator that preserves serde-tagged shape and is reviewable。
- Put generated types under `web/workspace/src/lib/generated/protocol.ts` or similarly clear generated path。
- Generator can be a test in `protocol`, a small binary/example, or another deterministic Cargo path; keep it simple and reproducible。
- If adding `ts-rs` feature annotations is invasive, cover a useful initial subset and document unsupported types clearly, but keep `Method`, `Event`, and `Segment` included。
- For `uuid::Uuid`, `PathBuf`, `serde_json::Value`, choose clear TS representations (`string`, `unknown`/JSON value, etc.) and document/test them。

Escalate if:
- `ts-rs` cannot represent current serde-tagged enums without unacceptable manual mirrors。
- Making protocol DTOs wasm/no-default-features compatible requires broad type redesign beyond optional `stream` feature。
- Generated file would include secret/project-local content or nondeterministic ordering。
- Backend proxy implementation scope expands into full Worker operation UI/session WebSocket implementation。

Validation plan:
- `cargo fmt --check`
- `cargo test -p protocol`
- `cargo check -p protocol --target wasm32-unknown-unknown --no-default-features`
- Type generation drift check command/test。
- `cd web/workspace && deno task check && deno task build`
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link` if Cargo.lock/package.nix/source behavior changes。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-23T05:41:13Z from: queued to: inprogress reason: human_authorized_unblocked_protocol_types_generation field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete acceptance criteria and no recorded blockers, so Orchestrator accepts implementation.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T05:42:14Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVSEBF56-protocol-typescript-types`
- Created branch:
  - `impl/00001KVSEBF56-protocol-typescript-types`
- Base commit:
  - `746c51c7 ticket: accept protocol ts generation`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVSEBF56`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVSEBF56-protocol-typescript-types`

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T06:09:42Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVSEBF56`.

Implementation commit:
- `a13fb693 protocol: generate workspace TypeScript types`

Changed areas:
- `crates/protocol`
  - Made `stream` optional behind default feature `stream`。
  - Added optional `typescript` feature using `ts-rs`。
  - Added `cfg_attr(feature = "typescript", derive(ts_rs::TS))` to protocol DTOs。
  - Added deterministic generator module and example:
    - `crates/protocol/src/typescript.rs`
    - `crates/protocol/examples/generate_typescript.rs`
  - Added drift check:
    - `cargo test -p protocol --features typescript generated_protocol_types_are_current`
- `web/workspace`
  - Added generated TypeScript artifact:
    - `web/workspace/src/lib/generated/protocol.ts`
  - Re-exported generated root protocol types from workspace sidebar types:
    - `PodProtocolMethod`
    - `PodProtocolEvent`
    - `PodProtocolSegment`
- `crates/workspace-server`
  - Updated extension-point notes to explicitly preserve no direct browser-to-Pod socket exposure and require future Workspace-server proxy enforcement of Worker identity / method allow-block boundaries。
- Packaging/dependencies:
  - Updated `Cargo.lock`。
  - Updated `package.nix` `cargoHash`。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p protocol`: passed
- `cargo test -p protocol --features typescript generated_protocol_types_are_current`: passed
- `cargo test -p protocol --features typescript`: passed
- `cargo check -p protocol --target wasm32-unknown-unknown --no-default-features`: passed
- `cd web/workspace && deno task check && deno task build`: passed
- `cargo run -p yoi -- ticket doctor`: passed
- `git diff --check`: passed
- `nix build .#yoi --no-link`: passed

Generated artifact cleanup:
- Only intended generated TypeScript import artifact is tracked。
- Orchestrator removed ignored `web/workspace/node_modules/` before Reviewer routing; no ignored frontend artifacts remain under `web/workspace`。

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVSEBF56-protocol-typescript-types` at `a13fb693`。
- Diff `746c51c7..HEAD` touches expected protocol/frontend/workspace-server/package files only。
- `git diff --check 746c51c7..HEAD`: passed。

Next action:
- Route to read-only Reviewer Pod for acceptance review。

---
