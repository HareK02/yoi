<!-- event: create author: tickets.sh at: 2026-06-01T02:11:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T23:01:38Z -->

## Decision

Updated based on user direction:

- keep this as the existing `tui-composer-history-persistence` ticket rather than creating a duplicate;
- default user-data shape should be like `~/.yoi/<path-to-pwd>/composer-history.json` using a path-safe/stable workspace key;
- do not create composer history under workspace `./.yoi/`;
- bound persisted recall history to about 30 entries per workspace instead of the older 100-entry note;
- keep typed `Segment` storage, non-destructive recall semantics, and no Pod/session transcript mutation.


---

<!-- event: intake_summary author: intake at: 2026-06-08T13:14:53Z -->

## Intake summary

既存 Ticket を確認し、同目的の open Ticket は他にないことを確認した。要件は、TUI composer の Up/Down recall 履歴を user data dir 配下に workspace-scoped で永続化し、workspace `./.yoi/`、Pod protocol、transcript、Worker/session history、model context には影響させないこと。保存対象は typed `Segment` vector、non-blank、連続重複抑止、workspace ごと最大 30 件程度。破損ファイルは startup を致命的に壊さず fallback する。受け入れ基準と validation は Ticket 本文に十分記録済みで、Orchestrator が routing 可能なため ready とする。

---

<!-- event: state_changed author: intake at: 2026-06-08T13:14:53Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake 確認により、既存 Ticket は実装 routing 可能な状態です。未解決の blocking open question はありません。主な reviewer focus は user-data persistence 境界、private/secret 混入前提の非露出、workspace 分離、既存 TUI recall semantics の回帰防止です。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T13:19:06Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-08T13:19:31Z -->

## Decision

Routing decision: defer_parallel_tui_work

This Ticket is queued and implementation-ready, but I am intentionally not accepting it to `inprogress` yet.

Reason:
- There is already an active queued-route TUI implementation in progress: `tui-chat-markdown-table-rendering`, with Coder Pod `coder-tui-chat-markdown-table-rendering` running in `.worktree/tui-chat-markdown-table-rendering`.
- `tui-composer-history-persistence` also touches TUI composer/input behavior and likely adjacent tests/helpers.
- Running both TUI changes concurrently would raise conflict/review risk without a clear need; this Ticket can follow immediately after the table-rendering Ticket reaches merge-ready or lands.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder for it yet.

Next action:
- Re-read this Ticket after `tui-chat-markdown-table-rendering` finishes review/merge routing, then accept it from queued if still unblocked.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T23:38:02Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after re-reading the Ticket and current workspace state. The previously active TUI table-rendering work has been reviewed, merged, closed, and cleaned up, so the earlier parallel-TUI-work deferral no longer applies.

---

<!-- event: decision author: orchestrator at: 2026-06-08T23:38:02Z -->

## Decision

Routing decision: implementation_ready

Reason:
- This Ticket has clear storage, privacy, workspace-separation, and recall-semantics requirements.
- The earlier deferral behind `tui-chat-markdown-table-rendering` is resolved: that Ticket has been merged, closed, and cleaned up.
- Main workspace is clean and there are no active implementation worktrees, so implementation side effects can proceed safely.

IntentPacket:

Intent:
- Persist TUI composer input recall history per workspace in user data, while preserving the existing TUI-local/non-destructive recall semantics.

Binding decisions / invariants:
- Store composer history under the existing user data dir, not under workspace `./.yoi/`.
- Workspace histories must be separated by a stable/path-safe workspace key and include metadata sufficient to identify the source workspace/display name.
- Store typed `Segment` vectors, not lossy flattened strings, so structured input recall remains possible.
- Persist only non-blank entries, suppress consecutive duplicates, and bound stored history to about 30 entries per workspace.
- Recalled input must not affect conversation state until the user explicitly submits it.
- Do not mutate Pod protocol, worker history, transcript/session logs, model context, or stored message history for recall operations.
- Treat history content as potentially private/secret: avoid diagnostics/logs/snapshots/tickets/model context exposure of saved entries.
- Corrupt history files must not make TUI startup fatal; fall back to empty history with a bounded warning if needed.
- Preserve existing multiline cursor navigation, Up/Down browse, draft restore, and edit-on-recall behavior.

Implementation latitude:
- Coder may choose the exact on-disk user-data path/key encoding if it is stable, path-safe, data-dir aware, and workspace-scoped.
- Coder may add a small persistence module near existing TUI composer history code or shared user-data helpers if present.
- Coder should prefer focused unit tests with temporary data dirs/workspaces for persistence, separation, bounding, duplicate suppression, corrupt-file fallback, and no workspace-file creation.
- If interactive confirmation is still useful, include exact manual steps in the implementation report, but do not rely on manual-only validation if focused tests are practical.

Escalate if:
- The implementation requires changing Pod protocol/history/session serialization.
- The implementation would store history under workspace `.yoi/`.
- Existing composer history representation cannot preserve typed `Segment` vectors without a larger input model redesign.
- Secret/private content would be emitted into logs/diagnostics/tests/tickets/model context.

Validation:
- Focused TUI composer history persistence tests for same-workspace recall after reload, workspace separation, max-size bound, consecutive duplicate suppression, corrupt-file fallback, and no workspace `.yoi` output.
- Existing TUI composer history/navigation tests if present.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo check --workspace`.
- `nix build .#yoi` before final completion because TUI behavior/packaging changes.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T23:38:48Z -->

## Implementation report

Implementation routing started after the previously deferred TUI table-rendering work was merged and cleaned up.

Worktree/branch:
- Worktree: `.worktree/tui-composer-history-persistence`
- Branch: `tui-composer-history-persistence`
- Base/routing commit: `ff5f985 ticket: route composer history persistence`

Spawned sibling Coder Pod:
- `coder-tui-composer-history-persistence`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Routing context:
- The earlier `defer_parallel_tui_work` decision is resolved because `tui-chat-markdown-table-rendering` has been merged, closed, and cleaned up.
- This Ticket is now the active TUI implementation work.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T23:55:25Z -->

## Implementation report

Coder implementation completed and is ready for sibling review.

Coder Pod:
- `coder-tui-composer-history-persistence`
- Commit: `64b7ff7c7c6f93c85e0c091ed5526e1e382ea023 tui: persist composer history`
- Worktree status before review: clean branch `tui-composer-history-persistence`
- Stopped after collecting output to reclaim delegated worktree scope.

Implementation summary:
- Adds user-data-backed TUI composer recall history persistence under `<data_dir>/composer-history/workspaces/<workspace-label>-<fnv64-key>/history.json`.
- Stores workspace metadata plus typed `Segment` vectors; does not flatten input to strings.
- Bounds history to 30 entries, skips blank inputs, and suppresses consecutive duplicates.
- Loads history on single-Pod TUI startup and saves on submit while keeping recall non-destructive/TUI-local.
- Corrupt history files fall back to empty history with a bounded actionbar warning that does not include saved entry contents.

Changed files:
- `crates/tui/src/composer_history.rs`
- `crates/tui/src/app.rs`
- `crates/tui/src/lib.rs`
- `crates/tui/src/single_pod.rs`

Coder validation reported passed:
- `cargo test -p tui composer_history`
- `cargo test -p tui input_history`
- `cargo test -p tui completion_flow_tests`
- `cargo fmt --check`
- `git diff --check`
- `cargo check --workspace`
- `nix build .#yoi`

Coder note:
- Full `cargo test -p tui` still fails at pre-existing `multi_composer_target_switch_preserves_typed_text`; `multi_pod.rs` was not modified.

---
