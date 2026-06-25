<!-- event: create author: "yoi ticket" at: 2026-06-25T13:42:37Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T14:08:22Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T14:08:22Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T14:13:35Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T14:14:52Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- outgoing `depends_on` は `00001KVZD10ED` で、`00001KVZD10ED` は done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- incoming dependent `00001KVZBCQH4` は後続 worker-runtime creation であり、この Ticket の acceptance blocker ではない。
- Ticket body は `crates/pod` -> `crates/worker` crate/package/import rename、public execution-unit API rename、CLI/process surface update、non-goals、validation を具体的に列挙している。
- bounded context check で current active references to `crates/pod`, `pod::`, package `pod`, `yoi pod` process entrypoint, tests/docs/Nix を確認した。変更量は大きいが mechanical rename + bounded public surface alignment として進められる。
- CLI/process surface は risk だが、Ticket は backward compatibility alias 不要、dogfooding runtime/spawn path/scripts/tests 影響を明示処理することを要求している。設計未決定というより implementation slice 内の bounded choice と判断する。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。thread は create、planning->ready、ready->queued のみで未解決 blocker は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVZD10ED` is done; incoming dependent `00001KVZBCQH4`; routing 前 plan 0 件。accepted plan `orch-plan-20260625-141406-1` を記録済み。
- Code context: `git grep` で `crates/pod`, `pod::`, package/dependency `pod`, `yoi pod`, `Pod` public API refs を確認。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。queued Ticket はこの 1 件、inprogress Ticket は 0 件。

IntentPacket:

Intent:
- 現在の single execution-unit host crate `pod` を `worker` に rename し、実行単位としての `worker::Worker` と LLM turn engine `llm_engine::Engine` の命名境界を揃える。

Binding decisions / invariants:
- This is a rename / API terminology alignment Ticket, not a responsibility rewrite.
- `worker` crate remains the current single Worker host: input handling, llm-engine integration, event emission, session/transcript compatibility, tool registry, workflow integration, legacy socket compatibility.
- Do not implement `worker-runtime` crate or multi-worker Runtime API.
- Do not standalone-rename `pod-store` / `pod-registry` to `worker-store` / `worker-registry` in this Ticket.
- Do not change provider request/tool-call/history semantics.
- Do not create `pod` crate/import compatibility alias.
- Existing on-disk/socket/session compatibility may retain legacy `pod` strings only where explicitly legacy/internal and documented.

Requirements / acceptance criteria:
- `crates/worker` exists; `crates/pod` does not remain.
- Cargo package/import path are `worker`.
- Public execution-unit type is `worker::Worker`, not `pod::Pod`.
- Active repository references to `pod` crate / `pod::` import / `crates/pod` are gone except intentional legacy context.
- Dependent crates compile against `worker` crate.
- `llm_engine::Engine` vs `worker::Worker` boundary is clear in code/docs/comments.
- Existing process/socket/session compatibility path still works or is explicitly updated without old-name alias.
- `pod-store` / `pod-registry` are not renamed as standalone crates.
- Validation target includes `cargo test -p worker`, `cargo test -p yoi` or relevant CLI tests, `cargo check -p yoi`, `git diff --check`, `nix build .#yoi --no-link`.

Implementation latitude:
- Exact CLI command spelling may be updated according to Ticket requirement, but no backward alias should be added unless a hard blocker appears. If command migration threatens current runtime dogfooding assumptions, escalate.
- Internal legacy file/socket/session names may remain only when required for compatibility and must be clearly legacy/internal, not active API guidance.
- Type/module rename can be staged mechanically; prioritize compile/test correctness and grep evidence.

Escalate if:
- Rename requires broad runtime architecture rewrite or worker-runtime implementation.
- Current process launch/spawn mechanics cannot work without a compatibility `pod` command/alias.
- `pod-store` / `pod-registry` must be renamed for compile correctness.
- Session/socket/on-disk migration would be required beyond explicit legacy compatibility labels.
- Behavior changes unrelated to naming are needed.

Validation:
- `cargo test -p worker`
- `cargo test -p yoi` or focused process/CLI tests
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- grep evidence for stale active references.

Current code map:
- Primary: `crates/pod`, workspace `Cargo.toml`, dependent crates (`crates/yoi`, `crates/tui`, workspace-server/client as discovered), tests/docs/resources, `Cargo.lock`, `package.nix`.
- Avoid: `pod-store` / `pod-registry` standalone rename, worker-runtime implementation, root/original workspace operations.

Critical risks / reviewer focus:
- stale active `pod` crate/import/directory references.
- hidden compatibility alias.
- breaking runtime process launch / spawn command path.
- accidentally renaming persistence crates out of scope.
- behavior changes mixed into mechanical rename.
- confusion between `llm_engine::Engine` and `worker::Worker` responsibilities.

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVZG9BMS-worker-crate-rename` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T14:15:18Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、derived blockers は空。
- outgoing dependency `00001KVZD10ED` は done / merged / reviewed / validated 済み。
- accepted plan `orch-plan-20260625-141406-1` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVZG9BMS-worker-crate-rename` を作成し、multi-agent-workflow に接続する。

---
