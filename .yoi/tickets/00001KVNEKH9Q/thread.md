<!-- event: create author: "yoi ticket" at: 2026-06-21T16:00:49Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-21T16:01:33Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T16:01:33Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T16:09:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T16:10:32Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket body は Workspace backend に local Host / Worker read API を追加し、Web UI skeleton に表示する範囲まで具体化されている。
- Related Ticket `00001KVMFFYVX` は Workspace web control plane bootstrap で、既に `closed` / integrated。This Ticket はその read-only API / SPA skeleton の自然な follow-up。
- Relation metadata は `related` のみで blocker relation はない。
- Current queued Ticket はこの Ticket のみ。
- Orchestrator worktree is clean on `orchestration` at `7abc3c77` before routing side effects; target worktree / branch is not present。
- Visible Pods に対象 Ticket の child Pod は存在しない。

Evidence checked:
- Ticket body / thread / artifacts via `TicketShow` and direct `item.md` read。
- `TicketRelationQuery(00001KVNEKH9Q)`: one non-blocking `related` relation to closed `00001KVMFFYVX`。
- `TicketOrchestrationPlanQuery(00001KVNEKH9Q)`: no records。
- `TicketList(state=queued)`: this Ticket is the only queued Ticket。
- Orchestrator git state / worktree list / branch list checked from `/home/hare/Projects/yoi/.worktree/orchestration` only。
- Bounded code map:
  - `crates/workspace-server/src/{server.rs,store.rs,records.rs}` contains current read-only workspace/ticket/objective/runs/runners API skeleton。
  - `web/workspace/src/routes/+page.svelte` currently renders workspace/tickets/objectives/runs/runners sections through Deno/SvelteKit frontend。
  - `crates/pod-store/src/lib.rs` contains current Pod metadata authority (`{data_dir}/pods/<pod_name>/metadata.json`) and list/read helpers。
  - `manifest::paths::data_dir()` resolves local data dir.

IntentPacket:

Intent:
- Extend the Workspace web control plane so it can display the local execution environment as Host / Worker domain objects, with local Pods exposed only as implementation details。

Binding decisions / invariants:
- API naming should prioritize Host / Worker; Pod remains implementation detail under `implementation.kind = "local_pod"` / `pod_name` fields。
- This is read-only local bridge over existing Pod metadata/state; do not mutate Pod metadata, session logs, Tickets, Objectives, or runtime state。
- Do not expose secrets, prompt contents, session transcript contents, tool result contents, or hidden metadata。
- Missing/unreadable local Pod data dir must degrade to empty worker list + bounded diagnostic or host capability unavailable, not a process-fatal error。
- Existing `.yoi` Ticket / Objective canonical workflows remain unchanged。
- This is not remote runner / cloud runner registration protocol, scheduling, start/stop/attach/notify, or full run-worker correlation。
- Existing `/api/runners` placeholder may be removed or migrated to host/worker naming; breaking API change is acceptable。
- Frontend remains static SPA; do not introduce SSR/business authority。

Requirements / acceptance criteria:
- `GET /api/hosts` returns backend-local Host as bounded JSON with stable-ish `host_id`, label, kind/status, observed/last-seen timestamp, and bounded capability summary。
- `GET /api/workers` or `GET /api/hosts/{host_id}/workers` returns Worker list from local Pod metadata/state。
- Worker response uses `worker_id`, `host_id`, `label`/`pod_name`, optional role/profile if known, `workspace_root`, state/status, `implementation: { kind: "local_pod", pod_name: ... }`, and bounded diagnostics。
- Local Pod metadata absence/unreadability yields safe empty/diagnostic response。
- `/api/runners` placeholder is removed or migrated to Host/Worker naming and tests/frontend are updated。
- Web UI displays Host / Worker list。
- Focused tests cover host list, worker list, missing local Pod data dir, and redaction/bounds。

Implementation latitude:
- Reuse `pod-store` crate for metadata listing if practical; otherwise use a narrow read-only adapter with the same authority/path semantics。
- Host id can be deterministic local id derived from workspace/data-dir/machine label if no existing host id exists; keep it stable-ish and non-secret。
- Worker status can be coarse from metadata active flag / socket hint if available; avoid live socket probing unless bounded and read-only。
- Runners placeholder can be removed or preserved only as compatibility alias if clearly documented; Ticket allows breaking migration to host naming。
- Frontend UI can be simple table/cards; prioritize data shape and redaction over visual polish。

Escalate if:
- Pod metadata schema lacks enough information to derive bounded worker fields without reading session transcripts。
- Need to read session JSONL contents to satisfy requested fields。
- Host id generation would require storing new durable host metadata in `.yoi`。
- Existing frontend/backend API shape requires a broad redesign beyond local bridge。
- Deno/frontend tooling or package/Nix changes become unrelatedly unstable。

Validation plan:
- `cargo fmt --check`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi-workspace-server`
- frontend: `cd web/workspace && deno task check && deno task build`
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link` if package/source-filter/frontend/dependency handling changed; otherwise strongly preferred because Workspace backend/frontend package surface is touched。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T16:10:40Z from: queued to: inprogress reason: human_authorized_unblocked_host_worker_api_implementation field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete acceptance criteria, related bootstrap work is complete, and no blockers are recorded, so Orchestrator accepts implementation.

---
