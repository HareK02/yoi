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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T16:11:47Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVNEKH9Q-workspace-host-workers`
- Created branch:
  - `impl/00001KVNEKH9Q-workspace-host-workers`
- Base commit:
  - `e1f02ffc ticket: accept workspace host workers`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVNEKH9Q`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVNEKH9Q-workspace-host-workers`

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T16:31:48Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVNEKH9Q`.

Implementation commit:
- `58143ead feat: expose workspace hosts and workers`

Changed areas observed by Orchestrator:
- `crates/workspace-server/src/hosts.rs` added:
  - Host / Worker API domain models and local Pod metadata bridge。
  - Safe degradation for missing/unreadable Pod metadata directory via bounded diagnostics。
  - Redaction/bounding tests。
- `crates/workspace-server/src/server.rs` updated:
  - Host / Worker API routes added。
  - `/api/runners` placeholder removed/migrated away from runner naming。
  - Tests updated for host/worker API and static/API separation。
- `crates/workspace-server/src/store.rs` updated:
  - Runners placeholder removed。
- `web/workspace/src/routes/+page.svelte` updated:
  - UI displays Host / Worker list instead of runners placeholder。
- Package/dependency updates:
  - `crates/workspace-server/Cargo.toml`
  - `Cargo.lock`
  - `package.nix`

Orchestrator evidence checked:
- Implementation worktree clean on `impl/00001KVNEKH9Q-workspace-host-workers`。
- HEAD is `58143ead`。
- Diff `e1f02ffc..HEAD` touches 8 expected backend/frontend/package files, about 913 insertions / 75 deletions。
- `git diff --check e1f02ffc..HEAD` produced no diagnostics。
- Validation had produced ignored frontend artifacts in child worktree:
  - `web/workspace/node_modules/`
  - `web/workspace/.svelte-kit/`
  - `web/workspace/build/`
- Orchestrator removed those ignored artifacts before reviewer routing; tracked worktree remains clean。

Next action:
- Route to read-only Reviewer Pod for acceptance review against the Ticket。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-21T16:32:22Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVNEKH9Q-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVNEKH9Q-workspace-host-workers`
- Branch: `impl/00001KVNEKH9Q-workspace-host-workers`
- Baseline: `e1f02ffc`
- Implementation commit: `58143ead feat: expose workspace hosts and workers`
- Diff: `e1f02ffc..58143ead`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Host / Worker API domain shape and redaction/bounding。
- Safe read-only local Pod metadata bridge。
- Missing/unreadable Pod metadata dir degradation。
- `/api/runners` placeholder migration/removal and frontend/test updates。
- Static SPA displays Host / Worker list without backend authority changes。
- Package/Nix/frontend generated artifact hygiene。

Orchestrator will wait for reviewer verdict before integration。

---

<!-- event: review author: yoi-reviewer-00001KVNEKH9Q-r1 at: 2026-06-21T16:38:05Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket authority:
  - `.yoi/tickets/00001KVNEKH9Q/item.md`
  - `.yoi/tickets/00001KVNEKH9Q/thread.md`
- Diff/changed areas `e1f02ffc..58143ead`:
  - `crates/workspace-server/src/hosts.rs`
  - `crates/workspace-server/src/server.rs`
  - `crates/workspace-server/src/store.rs`
  - `crates/workspace-server/src/lib.rs`
  - `crates/workspace-server/src/main.rs`
  - `crates/workspace-server/Cargo.toml`
  - `Cargo.lock`
  - `package.nix`
  - `web/workspace/src/routes/+page.svelte`
  - `web/workspace/svelte.config.js`
- Supporting inspection:
  - `crates/pod-store/src/lib.rs`
  - `runners` references and session/prompt/tool-result exposure grep。

Blocking issues:
- None。

Acceptance verification:
- API domain naming is Host / Worker:
  - Routes added for `/api/hosts`, `/api/workers`, and `/api/hosts/{host_id}/workers`。
  - `/api/runners` placeholder is removed and test-covered as `404`。
- Local Pods are implementation detail:
  - Worker model exposes domain fields (`worker_id`, `host_id`, `label`, `state`, `status`, etc.)。
  - Pod-specific detail is limited to `pod_name` and `implementation: { kind: "local_pod", pod_name: ... }`。
- Local Pod bridge is read-only:
  - Reads local Pod `metadata.json` under configured local data dir。
  - No mutation of Pod runtime metadata, session logs, Tickets, or Objectives observed。
- Redaction / sensitive data:
  - No session JSONL/transcript/tool-result/prompt content reads introduced。
  - `resolved_manifest_snapshot` is not serialized wholesale; role/profile extraction is bounded/sanitized。
  - Tests include a snapshot containing `secret_token` / `system_prompt` and assert they are not returned。
- Missing/unreadable metadata degradation:
  - Missing Pod metadata root returns empty workers plus bounded diagnostics。
  - Host capability degrades to unavailable instead of failing the server。
- Frontend:
  - Static Svelte page fetches/displays Hosts and Workers。
  - No SSR/business authority addition observed。
- Existing Ticket/Objective workflows remain preserved; `ticket doctor` passes。

Non-blocking concerns:
- None。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check e1f02ffc..HEAD`: passed
- `cargo test -p yoi-workspace-server`: passed (`6 passed`)
- `cargo check -p yoi-workspace-server`: passed
- `cd web/workspace && deno task check && deno task build`: passed (`svelte-check found 0 errors and 0 warnings`; static build completed)
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed

Generated artifacts:
- Frontend validation produced ignored generated artifacts only:
  - `web/workspace/.svelte-kit/`
  - `web/workspace/build/`
  - `web/workspace/node_modules/`
- Reviewer did not clean them due review-only scope; they are ignored and not tracked。

Worktree status:
- Tracked/untracked status is clean on `impl/00001KVNEKH9Q-workspace-host-workers`。
- Ignored generated frontend artifacts remain as listed above。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T16:38:57Z -->

## Implementation report

Reviewer r1 approvalを受けて、実装ブランチを Orchestrator worktree の `orchestration` branch に統合した。

統合:
- Merge commit: `c884d517 merge: workspace host workers`
- Merged branch: `impl/00001KVNEKH9Q-workspace-host-workers`
- Implementation commit:
  - `58143ead feat: expose workspace hosts and workers`

Orchestrator worktree validation:
- `cargo fmt --check`: passed
- `git diff --check HEAD^1..HEAD`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `cd web/workspace && rm -rf node_modules .svelte-kit build && deno task check`: passed (`svelte-check found 0 errors and 0 warnings`)
- `cd web/workspace && deno task build`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed

Validation generated ignored frontend artifacts in Orchestrator worktree:
- `web/workspace/node_modules/`
- `web/workspace/.svelte-kit/`
- `web/workspace/build/`

These were removed after validation. Final Orchestrator worktree status after validation cleanup is clean on `orchestration` at `c884d517`。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T16:39:02Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Reviewer approval、Orchestrator worktree への統合、workspace-server tests/check、Deno check/build、Ticket doctor、Nix build が完了したため `done` に遷移する。

---

<!-- event: state_changed author: hare at: 2026-06-21T16:39:15Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-21T16:39:15Z status: closed -->

## 完了

Workspace backend に local Host / Worker read API を追加し、Web UI に Host / Worker list を表示する変更を統合した。

主な成果:
- Workspace backend に `/api/hosts`, `/api/workers`, `/api/hosts/{host_id}/workers` を追加。
- `/api/runners` placeholder を削除 / Host-Worker naming に移行。
- Local Pod metadata を read-only bridge として Worker domain object に投影。
- Pod は primary domain ではなく `implementation: { kind: "local_pod", pod_name: ... }` として表現。
- Worker response includes bounded `worker_id`, `host_id`, label/pod name, role/profile when known, `workspace_root`, state/status, implementation detail, and diagnostics。
- Missing/unreadable Pod metadata root は process/server failure ではなく empty workers + bounded diagnostics / unavailable capability に degrade。
- Session transcript / tool result / prompt contents / raw session JSONL は読まない。
- Web UI static SPA に Host / Worker list を追加。
- Existing Ticket / Objective canonical workflows remain unchanged。

統合・検証:
- Merge commit: `c884d517 merge: workspace host workers`
- Implementation commit: `58143ead feat: expose workspace hosts and workers`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p yoi-workspace-server`, `cargo check -p yoi-workspace-server`, `deno task check`, `deno task build`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Remote/cloud runner registration, scheduling, start/stop/attach/notify operations, and full run-worker correlation were not implemented。
- Frontend remains static SPA and does not own backend business/lifecycle authority。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T16:39:43Z -->

## Implementation report

Post-close cleanup completed。

- Stopped child Pods and reclaimed scope:
  - `yoi-coder-00001KVNEKH9Q`
  - `yoi-reviewer-00001KVNEKH9Q-r1`
- Removed ignored frontend validation artifacts from child worktree before worktree removal:
  - `web/workspace/node_modules/`
  - `web/workspace/.svelte-kit/`
  - `web/workspace/build/`
- Removed implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVNEKH9Q-workspace-host-workers`
- Deleted implementation branch:
  - `impl/00001KVNEKH9Q-workspace-host-workers`
- Orchestrator worktree remains clean on `orchestration` at `a4ed5fb0`。

Root/original workspace was not used for merge/validation/cleanup operations。

Note for related active work:
- `00001KVNG9B9Z` sidebar UI work was branched before this merge and may need to integrate the Host/Worker UI/API changes from `c884d517` during review/merge。

---
