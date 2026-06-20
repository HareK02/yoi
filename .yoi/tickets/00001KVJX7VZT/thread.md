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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T16:31:44Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVJX7VZT-cli-resume-subcommand`
- Created branch:
  - `impl/00001KVJX7VZT-cli-resume-subcommand`
- Base commit:
  - `a63b40f4 ticket: accept resume cli routing`
- Confirmed tracked Ticket project records are visible in the child worktree.
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check.
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVJX7VZT`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVJX7VZT-cli-resume-subcommand`

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff. Orchestrator will not merge/close until reviewer approval and validation evidence are available。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T16:46:22Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVJX7VZT`.

Implementation commit:
- `861c351a feat: add explicit resume command`

Changed files:
- `crates/yoi/src/main.rs`
- `crates/tui/src/lib.rs`
- `crates/tui/src/console/mod.rs`
- `crates/tui/src/picker.rs`

Coder summary:
- Added explicit `yoi resume` subcommand。
- Added `yoi resume --workspace <PATH>` workspace-scoped picker behavior。
- Added `yoi resume --all` host/data-dir-wide picker behavior。
- Removed legacy top-level `-r` / `--resume` resume mode。
- Removed bare positional Pod-name inference (`yoi agent` now errors as unknown command)。
- Preserved explicit `--pod <NAME>` routing。
- Updated help to remove `[POD_NAME]` and `-r, --resume`, and document `yoi resume [--workspace <PATH>] [--all]`。
- Reused `PodList::from_workspace_sources(...)` for workspace-scoped resume filtering。
- Added parser and picker/workspace tests for new behavior。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p yoi parse_`: passed (`32 passed`)
- `cargo test -p tui picker_`: passed (`7 passed`)
- `cargo test -p tui workspace`: passed (`37 passed`)
- `cargo check -p yoi -p tui`: passed
- `git diff --check`: passed
- `target/debug/yoi ticket doctor`: passed (`doctor: ok`)
- CLI smoke:
  - `target/debug/yoi --help`: passed
  - `target/debug/yoi resume --help`: passed
  - `target/debug/yoi agent`: nonzero with `unknown command \`agent\``
  - `target/debug/yoi -r`: nonzero with `unknown argument: -r`
  - `target/debug/yoi --pod cli-smoke --socket /tmp/yoi-cli-smoke.sock`: reached TUI launch path and failed raw-mode due non-TTY environment as expected; parser tests cover explicit `--pod` routing。

Orchestrator evidence checked:
- Implementation worktree is clean on `impl/00001KVJX7VZT-cli-resume-subcommand`。
- HEAD is `861c351a`。
- Diff `a63b40f4..HEAD` touches 4 expected files with 282 insertions / 142 deletions。
- `git diff --check a63b40f4..HEAD` produced no diagnostics。

Next action:
- Route to read-only Reviewer Pod for acceptance review against the Ticket and IntentPacket。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-20T16:46:52Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVJX7VZT-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVJX7VZT-cli-resume-subcommand`
- Branch: `impl/00001KVJX7VZT-cli-resume-subcommand`
- Baseline: `a63b40f4`
- Implementation commit: `861c351a feat: add explicit resume command`
- Diff: `a63b40f4..861c351a`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- No remaining `-r` / `--resume` compatibility alias。
- No remaining top-level bare Pod-name inference。
- Default `yoi resume` is workspace-scoped and `resume --all` is required for host-wide listing。
- Workspace filtering uses persisted Pod metadata `workspace_root` semantics。
- Explicit `--pod <NAME>` and existing subcommands remain parseable。
- Help/errors are discoverable and no longer advertise `[POD_NAME]` or legacy resume flag。

Orchestrator will wait for reviewer verdict before integration。

---

<!-- event: review author: yoi-reviewer-00001KVJX7VZT-r1 at: 2026-06-20T16:50:49Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket authority:
  - `.yoi/tickets/00001KVJX7VZT/item.md`
  - `.yoi/tickets/00001KVJX7VZT/thread.md`
- Implementation diff `a63b40f4..861c351a`:
  - `crates/yoi/src/main.rs`
  - `crates/tui/src/lib.rs`
  - `crates/tui/src/console/mod.rs`
  - `crates/tui/src/picker.rs`
- Related inspected file for stale user-facing guidance:
  - `crates/tui/src/dashboard/mod.rs`
- Related workspace filtering authority:
  - `crates/tui/src/pod_list.rs`

Core implementation assessment:
- `resume` is parsed as an explicit subcommand。
- Legacy `-r` / `--resume` no longer parse as resume。
- Bare top-level words are no longer inferred as Pod names。
- `--pod <NAME>` remains the explicit direct path。
- Default resume uses `PickerOptions::workspace(...)` / `PodList::from_workspace_sources(...)`。
- `resume --all` uses host/data-dir-wide sources。

Blocking issue:

1. Stale user-facing guidance still tells users to run removed `yoi -r`。

Evidence:

```rust
// crates/tui/src/dashboard/mod.rs:91-94
Self::NoPods => write!(
    f,
    "no Tickets or Pods found — create a Ticket with `yoi ticket create` or restore a Pod with `yoi -r`"
),
```

This is misleading because `yoi -r` intentionally fails:

```text
$ target/debug/yoi -r
yoi: unknown argument: -r
try `yoi --help` for usage.
exit=1
```

This conflicts with the Ticket’s explicit CLI replacement intent and the reviewer focus that parser/help/user-facing errors should be discoverable and not misleading. Update the guidance to `yoi resume`, or if this Dashboard no-pods case intends host/data-dir-wide discovery, use explicit `yoi resume --all` wording。

Non-blocking concern:
- `PickerError::NoPods` still says `no pods found — start a fresh pod with `yoi` and try again`。
- For default workspace-scoped `yoi resume`, this can be ambiguous if Pods exist outside the current workspace。Consider a scope-aware message such as `no pods found in this workspace; use `yoi resume --all` to list all host/data-dir Pods` either in this fix or as follow-up。This is not blocking by itself。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check a63b40f4..HEAD`: passed
- `cargo test -p yoi parse_`: passed, 32 tests
- `cargo test -p tui picker_`: passed, 7 tests
- `cargo test -p tui workspace`: passed, 37 tests
- `cargo check -p yoi -p tui`: passed
- `target/debug/yoi ticket doctor`: passed (`doctor: ok`)
- CLI smoke:
  - `target/debug/yoi --help`: exit 0; no `[POD_NAME]`, no `-r, --resume`, includes `yoi resume [--workspace <PATH>] [--all]`
  - `target/debug/yoi resume --help`: exit 0; documents `--workspace` and `--all`
  - `target/debug/yoi agent`: exit 1; `unknown command 'agent'`
  - `target/debug/yoi -r`: exit 1; `unknown argument: -r`
  - `target/debug/yoi --resume`: exit 1; `unknown argument: --resume`
  - `target/debug/yoi memory other`: exit 1; usage error, not Pod-name inference
  - `target/debug/yoi resume --all --workspace /tmp/ws`: exit 1; mutually exclusive error
- Stale-alias search:
  - `rg 'yoi -r|--resume|\[POD_NAME\]' . --glob '!target/**' --glob '!Cargo.lock'`
  - Found blocking active product-code stale guidance at `crates/tui/src/dashboard/mod.rs:93`; historical docs/report mentions were not treated as product guidance。

Worktree status:
- Clean on `impl/00001KVJX7VZT-cli-resume-subcommand`。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T16:54:23Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVJX7VZT`.

New fix commit:
- `d25ca6ff fix: update resume guidance`

Branch commits now:
- `861c351a feat: add explicit resume command`
- `d25ca6ff fix: update resume guidance`

Changed files in r1 fix:
- `crates/tui/src/dashboard/mod.rs`
- `crates/tui/src/picker.rs`

Fix summary:
- Updated Dashboard no-pods guidance from removed `yoi -r` to `yoi resume`。
- Searched active product guidance for `yoi -r`, `--resume`, and `[POD_NAME]` excluding historical docs/reports and Ticket records; no remaining active stale guidance found。
- Addressed Reviewer non-blocking concern:
  - Workspace-scoped resume empty result now says: `no pods found in this workspace — use yoi resume --all to list all host/data-dir Pods`
  - Host-wide `--all` empty result keeps the fresh Pod hint。
  - Added picker tests for these messages。

Coder validation reported:
- `cargo fmt --check`: passed
- `git diff --check`: passed
- `cargo test -p tui picker_`: passed
- `cargo test -p yoi parse_`: passed
- `cargo test -p tui workspace`: passed
- `cargo check -p yoi -p tui`: passed
- `target/debug/yoi ticket doctor`: passed
- `cargo build -p yoi`: passed
- CLI smoke:
  - `target/debug/yoi --help`: passed
  - `target/debug/yoi resume --help`: passed
  - `target/debug/yoi agent`: nonzero with `unknown command \`agent\``
  - `target/debug/yoi -r`: nonzero with `unknown argument: -r`
  - `target/debug/yoi --resume`: nonzero with `unknown argument: --resume`

Orchestrator evidence checked:
- Implementation worktree clean on `impl/00001KVJX7VZT-cli-resume-subcommand`。
- HEAD is `d25ca6ff`。
- r1-fix diff `861c351a..HEAD` touches only `crates/tui/src/dashboard/mod.rs` and `crates/tui/src/picker.rs`。
- Active product-guidance grep under `crates` / `resources` for `yoi -r|--resume|[POD_NAME]` returned only the parser test expectation for `unknown argument: --resume`, not active user guidance。
- `git diff --check a63b40f4..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r2 verification and final acceptance review。

---
