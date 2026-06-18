<!-- event: create author: ticket-intake at: 2026-06-18T14:09:59Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-18T14:10:12Z -->

## Intake summary

ユーザー承認済み。`yoi panel` の通常 Pod 表示を現在の runtime workspace に属する Pod だけに限定する concrete work item。workspace 外 Pod は通常一覧/action target から除外し、workspace 判定不能な legacy/corrupt metadata は通常表示せず bounded diagnostic のみ許容する。受け入れ条件・binding invariants・validation は Ticket body に記録済み。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-18T14:10:12Z from: planning to: ready reason: user_approved_intake_ready field: state -->

## State changed

Ticket intake が完了しました。実装起動は Orchestrator routing / queue flow に委ねます。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-18T14:47:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-18T14:50:44Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body には、workspace-scoped Pod visibility の intent、binding invariants、acceptance criteria、implementation latitude、validation、escalation conditions が揃っている。
- Risk domain は Panel / Pod metadata / workspace boundary / runtime observation だが、Ticket では authority を runtime workspace root / persisted Pod metadata に固定しており、Pod name prefix / cwd heuristic を禁止している。bounded context check 後も implementation 前に必要な追加の human decision は見つからなかった。
- `depends_on` / incoming `blocks` の blocker はない。OrchestrationPlan には既存 blocker/conflict record はなく、今回 accepted plan を記録済み。
- 現在 inprogress Ticket は 0 件で、visible child Pod もない。Orchestrator worktree は clean、同名 branch/worktree は存在しない。

Evidence checked:
- Ticket `00001KVDH2E06` body / thread / artifacts。
- `TicketRelationQuery(00001KVDH2E06)`: relation 0 件、blocking acceptance blocker なし。
- `TicketOrchestrationPlanQuery(00001KVDH2E06)`: 既存 record なし。今回 `accepted_plan` を記録済み。
- `TicketList`: queued は本 Ticket 1 件、inprogress は 0 件。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`: clean。
- Existing worktree/branch: `00001KVDH2E06` / panel-current-workspace-pods matching branch/worktree なし。
- Visible Pods: self `yoi-orchestrator` のみ。
- Current code map:
  - `crates/tui/src/pod_list.rs`: stored/live Pod metadata を `PodListEntry` に merge して action/diagnostics を作る。
  - `crates/tui/src/workspace_panel.rs`: `build_workspace_panel*` と `pod_rows(pods)` が Pod rows を Panel に入れている。
  - `crates/pod-store/src/lib.rs`: durable `PodMetadata` は `resolved_manifest_snapshot` を持ち、runtime workspace 判定の authority candidate になる。
  - Current `pod_rows` は `pods.entries.iter().map(pod_row)` で、workspace filter が見当たらない。

IntentPacket:

Intent:
- `yoi panel` の通常 Pod 表示と Pod action target を、現在の runtime workspace に属する Pod に限定する。
- 別 workspace の Pod、workspace 判定不能な legacy/corrupt metadata を通常 Pod rows に混ぜない。
- Current workspace の Companion / Orchestrator / Ticket role Pod は、cwd が dedicated orchestration worktree や implementation worktree でも、runtime workspace が現在 workspace なら表示対象に残す。

Binding decisions / invariants:
- Authority は runtime workspace root / persisted Pod metadata / resolved runtime workspace identity。Pod name prefix や process cwd だけで判定しない。
- `role_workspace_root` / `original_workspace_root` / `implementation_worktree_root` / `merge_target_workspace_root` を混同しない。
- Workspace 外 Pod は通常一覧に出して warning するのではなく通常一覧から除外する。
- 別 workspace Pod に対する attach/open/action path を Panel から提供しない。
- Corrupt / unknown workspace metadata は current workspace とみなさない。必要なら bounded diagnostic に留める。
- 既存 Pod metadata の破壊的 migration はこの Ticket の範囲外。
- Panel Ticket rows / queue actions / Orchestrator lifecycle semantics は維持する。

Requirements / acceptance criteria:
- workspace A で `yoi panel` を開いたとき workspace B の Pod が通常 Pod list に表示されない。
- Current workspace の workspace Orchestrator / Companion / Ticket role Pod は表示・操作できる。
- cwd が `.worktree/...` 配下でも runtime workspace が current workspace なら隠されない。
- Unknown/corrupt workspace metadata は通常 current-workspace Pod row として誤表示されない。
- Attach/open は表示されている current workspace Pod に対して従来通り機能する。
- Focused tests または E2E/fixture tests で multiple workspace Pod metadata の filter を確認する。

Implementation latitude:
- workspace root canonicalization / path comparison の具体実装は Coder が既存 pattern に合わせて選んでよい。
- Filtering location は `PodList` 側または `WorkspacePanelViewModel` construction 側のどちらでもよいが、normal Panel rows/action target に外部 workspace Pod が混ざらないこと。
- 判定不能 Pod の diagnostic 表示は既存 Panel diagnostics pattern に合わせてよい。
- E2E が重すぎる場合は unit/fixture coverage を優先してよい。ただし user-visible Panel behavior の確認手段は残す。

Escalate if:
- Existing metadata / resolved manifest snapshot に runtime workspace root が十分に保存されておらず、schema/storage 追加や destructive migration が必要。
- Legacy Pod を隠すことで restore/attach 導線を維持できない。
- canonicalization で symlink / moved checkout / worktree treatment の product decision が必要。
- no-Ticket fallback が all-Pod dashboard であるべきか current-workspace dashboard であるべきか、既存設計と衝突する。
- 別 workspace Pod を診断用に通常 action row とは別に見せる必要が出る。

Validation:
- Focused tests for Panel ViewModel / Pod list filtering:
  - current workspace Pod visible;
  - other workspace Pod hidden;
  - current workspace Orchestrator/role Pod visible;
  - cwd/worktree difference alone does not hide current workspace Pod;
  - unknown/corrupt workspace metadata is not treated as current workspace。
- Existing Panel/TUI tests continue to pass。
- If practical: Panel E2E fixture with multiple workspace Pod metadata records。
- `cargo fmt --check`
- `git diff --check`
- relevant `cargo test` / `cargo check`

Critical risks / reviewer focus:
- workspace filter が cwd/name heuristic になっていないか。
- metadata 判定不能 Pod を current workspace と誤認していないか。
- dedicated orchestration / implementation worktree cwd の current workspace Pod を誤って隠していないか。
- attach/open/action target が hidden external Pod に残っていないか。
- no-Ticket fallback と Ticket-enabled Panel の両方で current workspace Pod discovery が維持されているか。

Next action:
- `queued -> inprogress` を記録し、Ticket records を Orchestrator worktree に commit してから、専用 implementation worktree を作成し Coder Pod を narrow write scope で起動する。root/original workspace は操作しない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-18T14:50:54Z from: queued to: inprogress reason: orchestrator_acceptance_current_workspace_pod_filter field: state -->

## State changed

Ticket body/thread, relation metadata, orchestration plan records, Orchestrator worktree, visible Pods, existing branch/worktree, and bounded code context were checked. No blocking relation, conflict, dirty-state blocker, or missing planning decision was found. Risk flags are captured as invariants/reviewer focus rather than stop gates. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T14:51:50Z -->

## Implementation report

Implementation start note:

`queued -> inprogress` acceptance、accepted plan、routing decision / IntentPacket を記録し、Orchestrator worktree で commit した後に、専用 implementation worktree と Coder Pod を起動した。

Worktree:
- `/home/hare/Projects/yoi/.worktree/00001KVDH2E06-panel-current-workspace-pods`
- branch: `impl/00001KVDH2E06-panel-current-workspace-pods`

Coder Pod:
- `yoi-coder-00001KVDH2E06`

Scope / boundaries:
- child runtime workspace root は read scope。
- implementation worktree は write scope。
- root/original workspace と Orchestrator worktree へは書き込まないよう指示済み。
- `.yoi/memory`、runtime/log/lock/local/secret-like `.yoi` paths は編集しないよう指示済み。

Next:
- Coder は current-workspace Pod filtering、focused tests、validation、implementation commit を行い、実装報告を返す予定。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T15:15:57Z -->

## Implementation report

Coder implementation report received:

Commit:
- `3b634d66` — `tui: filter panel pods by workspace`

Summary:
- `PodMetadata` に optional persisted `workspace_root` を追加し、新規 metadata write 時に Pod runtime workspace を保存するようにした。
- persisted metadata に基づく workspace-aware Pod list construction/filtering を追加。Pod name prefix や cwd heuristic では判定しない。
- `yoi panel` の normal Pod rows / action targets を current runtime workspace metadata で filter するようにした。
- legacy / unknown / corrupt workspace metadata は normal Panel Pod rows では fail-closed にした。
- current-workspace Companion / Orchestrator / Ticket role Pod は、stored runtime workspace が current workspace と一致すれば worktree/cwd 差分に関係なく表示対象に残る。

Files changed:
- `crates/pod-store/src/lib.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/discovery.rs`
- `crates/pod/src/ticket_event_notify.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/pod_list.rs`
- `crates/tui/src/workspace_panel.rs`

Focused validation reported by Coder:
- `cargo test -p tui workspace_panel -- --nocapture` — passed; 23 tests
- `cargo test -p tui pod_list -- --nocapture` — passed; 19 tests
- `cargo test -p pod-store` — passed; 6 tests
- `cargo test -p pod pod_metadata -- --nocapture` — passed targeted Pod metadata tests and filtered integration tests
- `cargo check -p tui -p pod -p pod-store` — passed
- `cargo fmt --check` — passed
- `git diff --check` — passed

Full-suite note:
- Coder also attempted `cargo test -p tui`; it failed with 3 tests outside the focused scope:
  - `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`
  - `spawn::tests::profile_choices_include_builtin_and_project_default_marker`
  - `spawn::tests::profile_choices_use_project_registry_default`
- Orchestrator re-ran these failing tests in both the implementation worktree and the current Orchestrator worktree before this implementation was merged. The same failures reproduce on the Orchestrator branch, so they are currently treated as pre-existing validation debt rather than blockers for this Ticket, unless review finds the implementation worsens their semantics.

Orchestrator evidence checked:
- Pod output を確認。
- implementation worktree は clean。
- branch `impl/00001KVDH2E06-panel-current-workspace-pods` HEAD は `3b634d66`。
- diff stat from accepted base `e2e76d3b..3b634d66`: 7 files changed, 292 insertions, 14 deletions。
- Focused failing-test comparison:
  - `cargo test -p tui multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace -- --nocapture` fails identically on implementation worktree and Orchestrator worktree.
  - `cargo test -p tui spawn::tests::profile_choices -- --nocapture` fails identically on implementation worktree and Orchestrator worktree.

Routing classification: review_needed

Next action:
- recorded intent / invariants / acceptance criteria に照らして、read-only Reviewer Pod で外部レビューする。
- 特に workspace authority が runtime workspace metadata であり cwd/name heuristic ではないこと、unknown/corrupt metadata の fail-closed、current workspace role Pod visibility、hidden external Pod に action target が残らないこと、optional `workspace_root` metadata の compatibility を確認する。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-18T15:16:45Z -->

## Plan

Review dispatch:

Reviewer Pod `yoi-reviewer-00001KVDH2E06` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVDH2E06-panel-current-workspace-pods`
- branch: `impl/00001KVDH2E06-panel-current-workspace-pods`
- commit: `3b634d66`

Reviewer focus:
- recorded intent / invariants / acceptance criteria に対する適合性。
- workspace ownership authority が persisted runtime workspace metadata / resolved identity に基づき、Pod name prefix / cwd heuristic ではないこと。
- external workspace Pod が normal rows と normal action targets から除外されること。
- current workspace Companion / Orchestrator / Ticket role Pod が cwd/worktree difference によって誤って隠されないこと。
- unknown/corrupt/legacy metadata が fail-closed で current workspace と誤認されないこと。
- optional `workspace_root` metadata が non-destructive / compatible であること。
- focused tests が current visible / external hidden / role visible / worktree cwd difference / live-only legacy corrupt hidden / Panel row-action filtering を十分に覆っていること。
- `cargo test -p tui` の 3 failure は Orchestrator branch でも同様に再現するため pre-existing debt と見てよいか、それともこの branch が semantic に悪化させているか。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree へ merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVDH2E06 at: 2026-06-18T15:24:39Z status: request_changes -->

## Review: request changes

Review result: request_changes

対象:
- Ticket: `00001KVDH2E06`
- branch: `impl/00001KVDH2E06-panel-current-workspace-pods`
- commit: `3b634d66` (`tui: filter panel pods by workspace`)

Blocker:
- runtime metadata 用の `workspace_root` は計算されているが、normal metadata write-through path で永続化される前に捨てられている。

Evidence:
- `crates/pod/src/pod.rs:4320-4327` では `PodMetadata::new(...).with_workspace_root(workspace_root.to_path_buf())` を構築している。
- しかし `pod_metadata_writer_for_store` (`crates/pod/src/pod.rs:68-80`) は `store.set_active(&metadata.pod_name, metadata.active, metadata.resolved_manifest_snapshot)` だけを呼び、`metadata.workspace_root` を渡していない。
- `PodMetadataStore::set_active` / `update_by_name` (`crates/pod-store/src/lib.rs:154-178`) は absent record を `PodMetadata::new(pod_name, None)` で作り、`active` / `resolved_manifest_snapshot` だけを更新する。そのため、新規 metadata file は `workspace_root: None` のままになる。
- constructors/write-through paths、例: `write_pod_metadata_pending` はこの writer を使うため、normal runtime path では新しい workspace ownership field が永続化されないように見える。

Why this breaks acceptance:
- `crates/tui/src/pod_list.rs:68-95` は fail-closed 設計で、stored Pods は `info.workspace_root.is_some_and(...)` の場合だけ含め、live Pods も stored current-workspace record と name が一致した場合だけ含める。
- normal write path が current-workspace Pods を `workspace_root: None` のままにすると、新規 current-workspace Pods が unknown/legacy として扱われ、normal Panel rows/action targets から隠れる。
- これは current-workspace Companion / Orchestrator / Ticket role Pods を表示するという acceptance criteria を直接壊す。

Positive findings:
- filter logic の方向性は正しい。persisted `workspace_root` を使っており、Pod name / live cwd heuristic ではない。
- `workspace_root: Option<PathBuf>` に serde default / skip-serializing-if を付ける設計は old metadata に対して non-destructive に見える。
- tests は pure Panel/PodList filtering fixtures を覆っているが、runtime metadata with `workspace_root` が normal Pod runtime path で実際に永続化される persistence boundary を見逃している。

Requested changes:
- write-through path で `metadata.workspace_root` を永続化すること。
  - 例: `set_active` を拡張する、または active/snapshot/workspace ownership を同時に更新する store method を追加する。
  - unrelated fields は保持すること。
- normal Pod metadata write-through を構築/実行し、stored metadata に runtime workspace root が残ることを確認する targeted test を追加する。
- Filtering fixture だけではなく、`PodMetadata::with_workspace_root` が存在するだけでもなく、normal runtime/store boundary の永続化を検証すること。

Validation note:
- Reviewer は read-only inspection を実施。Coder の focused validation は確認した。
- `cargo test -p tui` の既知 3 failures について、この branch が原因/悪化とは見ていない。ただし上記 persistence blocker は独立して修正が必要。

---
