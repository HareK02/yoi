<!-- event: create author: LocalTicketBackend at: 2026-06-20T16:18:52Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T16:29:26Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T16:30:48Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket body は `yoi resume` サブコマンド化、top-level bare Pod name 推測廃止、workspace-scoped resume default、明示 `resume --all`、`--pod <NAME>` 維持、help/parser/picker tests 更新まで具体化されている。
- Binding decisions / invariants と implementation latitude が分離されており、`-r` / `--resume` alias を残さない方針、host-wide enumeration を `resume --all` に限定する方針、legacy Pod metadata 判断時の escalation 条件も明示されている。
- Relation metadata と orchestration plan に blocker はない。
- Orchestrator worktree は clean で、対象 Ticket 用の既存 implementation worktree / branch はない。
- Visible Pods にこの Ticket の coder/reviewer は存在せず、現在 queued Ticket はこの Ticket のみ。

Evidence checked:
- Ticket body / thread / artifacts via `TicketShow` and direct `item.md` read。
- `TicketRelationQuery(00001KVJX7VZT)`: no relations / blockers。
- `TicketOrchestrationPlanQuery(00001KVJX7VZT)`: no records。
- `TicketList(state=queued)`: queued Ticket はこの Ticket のみ。
- `ListPods`: current visible Pods に対象 Ticket の child Pod はない。
- Orchestrator worktree git state: clean on `orchestration` at `8684344e`。
- Worktree list / branch list: 対象 Ticket 用 worktree / branch はない。
- Bounded code map check:
  - `crates/yoi/src/main.rs` に `LaunchMode::PodName` / `LaunchMode::Resume` / old help and parser tests がある。
  - `crates/tui/src/lib.rs` currently dispatches `LaunchMode::Resume` to `console::run_resume(runtime_command)`。
  - `crates/tui/src/console/mod.rs` resume path calls `picker::run()` without workspace filtering。
  - `crates/tui/src/pod_list.rs` has `PodList::from_workspace_sources(...)` and workspace metadata filtering tests。

IntentPacket:

Intent:
- CLI resume UX を explicit subcommand model に変更し、bare word Pod name inference を廃止する。
- Default resume picker は workspace-scoped、`resume --all` は human CLI explicit opt-in の host-wide listing とする。

Binding decisions / invariants:
- Top-level bare word を Pod 名として推測しない。
- Pod 名を直接開く導線は explicit `--pod <NAME>` のみ維持する。
- `-r` / `--resume` は互換 alias として残さず、unknown/deprecated error にする。
- `resume --all` なしで host-wide Pod list を表示しない。
- Workspace-scoped resume は Pod metadata の `workspace_root` を authority にし、Dashboard と同じ方向の semantics を使う。
- Human CLI の `resume --all` と LLM-facing Pod tool visibility/scope を混同しない。
- `yoi` 引数なし default Console 起動は変更しない。
- Existing explicit command paths (`yoi pod`, `yoi ticket`, `yoi plugin`, `yoi memory lint`, `yoi panel`, `--session`, `--profile`) を壊さない。

Requirements / acceptance criteria:
- `yoi resume` が resume picker を開く。
- `yoi resume --workspace <PATH>` が指定 workspace の Pod だけを表示する。
- `yoi resume --all` が host/data-dir wide Pod 一覧を表示する。
- `yoi -r` / `yoi --resume` は legacy resume mode にならない。
- `yoi <bare-word>` は `PodName` mode ではなく unknown command / usage error になる。
- `yoi --pod <NAME>` は明示 Pod open/attach/restore/create path として残る。
- `yoi --help` から `[POD_NAME]` と `-r, --resume` guidance を消し、`yoi resume [--workspace <PATH>] [--all]` を案内する。
- Parser tests と TUI/pod-list/picker tests が新挙動を固定する。

Implementation latitude:
- `LaunchMode::Resume` に options を持たせるか、別 options 型を作るかは Coder 判断でよい。
- picker API は options 化してよい。
- Workspace filtering は `PodList::from_workspace_sources(...)` reuse を優先し、必要なら小 helper extraction を許可する。
- Error/help wording は `--pod` と `resume` が発見しやすい範囲で Coder 判断可。
- CLI smoke は non-interactive に検証できる範囲でよい。

Escalate if:
- `workspace_root` metadata がない legacy Pod を workspace-scoped resume に含めるべきか実装中に判断不能になる。
- `-r` / `--resume` alias を残す必要が出る。
- `resume --all` が Pod scope / permission / LLM-visible tool visibility と混ざる設計になりそうになる。
- Parser change が existing explicit command paths を壊す。
- 要件達成に public CLI surface beyond this Ticket を追加する必要が出る。

Validation:
- `cargo fmt --check`
- Focused parser tests in `-p yoi`
- Focused TUI/pod-list/picker tests in `-p tui`
- `cargo check -p yoi -p tui`
- `git diff --check`
- `yoi ticket doctor`
- CLI smoke as practical: `target/debug/yoi --help`, `target/debug/yoi resume --help`, `target/debug/yoi unknown`, and parser coverage for `--pod`。

Current code map:
- Primary files: `crates/yoi/src/main.rs`, `crates/tui/src/lib.rs`, `crates/tui/src/console/mod.rs`, `crates/tui/src/picker.rs`, `crates/tui/src/pod_list.rs`。
- Related panel semantics reference: `crates/tui/src/workspace_panel.rs` / dashboard PodList filtering, but avoid broad dashboard churn。

Critical risks / reviewer focus:
- Backward compatibility intentionally changes: ensure old positional Pod name tests are removed/replaced, not silently preserved。
- Ensure `resume --all` is explicit and default resume cannot leak host-wide Pod list。
- Ensure `--pod <NAME>` remains the explicit direct Pod path。
- Ensure help text and parser error behavior make the new command model discoverable。
- Ensure workspace filtering depends on metadata `workspace_root`, not live socket heuristics alone。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T16:30:54Z from: queued to: inprogress reason: human_authorized_unblocked_implementation field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete acceptance criteria and no recorded blockers, so Orchestrator accepts implementation and will create a child implementation worktree before spawning sibling Coder/Reviewer roles.

---
