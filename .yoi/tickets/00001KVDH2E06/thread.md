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
