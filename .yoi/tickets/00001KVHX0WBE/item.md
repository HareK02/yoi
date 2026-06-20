---
title: 'Dashboard / Console 呼称導入と TUI モジュール境界整理'
state: 'inprogress'
created_at: '2026-06-20T06:55:49Z'
updated_at: '2026-06-20T09:33:16Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['ux-naming', 'module-boundary', 'public-cli', 'test-coverage']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T08:30:58Z'
---

## Background

TUI まわりの user-facing surface 名と code/module 境界を明確にする。

- **Dashboard**: `yoi panel`。workspace-level の Ticket / Pod / Companion / Orchestrator / Intake 状態と action を扱う surface。
- **Console**: 通常の単一 Pod に接続して会話する chat/client surface。
- **TUI**: Dashboard / Console / picker / setup UI などを実装する terminal UI crate/layer の総称。

事前調査では `crates/tui/src/multi_pod.rs` は約 9452 lines で、実態は multi-Pod list というより workspace Dashboard 本体だった。主な責務分布は次の通り。

```text
    1-  307    entry/runtime loop
  308-  451    background reload/notice handles
  452-  792    intake launch/handoff helpers
  793- 1160    diagnostics/e2e dashboard structs
 1161- 2422    MultiPodApp state + key/mouse/composer methods
 2423- 3377    snapshot/lifecycle/load
 3378- 4986    companion/orchestrator/ticket actions/queue handoff
 4987- 6258    row classification/layout/render
 6259- 9452    tests
```

関連ファイル規模の目安:

```text
  9452 crates/tui/src/multi_pod.rs
  2812 crates/tui/src/workspace_panel.rs
  2346 crates/tui/src/single_pod.rs
  3689 crates/tui/src/app.rs
  1851 crates/tui/src/ui.rs
  1209 crates/tui/src/pod_list.rs
   556 crates/tui/src/role_session_registry.rs
```

`multi_pod.rs` は unit test / async test も多く、挙動維持の根拠としてテストを活かしながらまとめて整理する。

## Requirements

- `Dashboard` / `Console` / `TUI` の呼称を導入し、help / docs / comments / test names / internal naming を可能な範囲で揃える。
- `Dashboard` は `yoi panel` の user-facing surface 名とする。
- `Console` は単一 Pod 接続チャットクライアントの user-facing surface 名とする。
- `TUI` は terminal UI implementation layer の総称として扱い、Dashboard / Console の代替 mode 名として乱用しない。
- `yoi panel` command 名は維持する。
- `yoi dashboard` などの不要な互換 alias は追加しない。
- Dashboard の起動 entrypoint を Console / `single_pod` 側から分離する。
- Dashboard から Pod を開いて Console に入る bridge は残すが、narrow API として責務を明確にする。
- `crates/tui/src/multi_pod.rs` を Dashboard module 境界へ寄せる。
- `multi_pod.rs` の巨大化を解消するため、少なくとも次の責務を分離・整理する。
  - Dashboard entry/runtime loop
  - Dashboard app state / action dispatch
  - Dashboard snapshot/lifecycle loading
  - Companion / Orchestrator / Intake / Ticket action glue
  - Dashboard render/layout/row classification
  - Dashboard e2e diagnostics structs
  - Dashboard tests
- `workspace_panel.rs` は Dashboard view model builder としての責務を確認し、必要なら名前・配置・参照を Dashboard 語彙に寄せる。
- `single_pod.rs` は Console 側の起動・接続・spawn/resume・chat loop を主責務にする。
- 挙動変更は目的にしない。主眼は naming / module boundary / maintainability refactor。

## Acceptance criteria

- `yoi panel` が Dashboard として説明されている。
- 単一 Pod UI が Console として説明されている。
- `TUI` が Dashboard / Console の総称 implementation layer として整理されている。
- `LaunchMode::Panel` が `single_pod::run_panel` のような Console module entrypoint に直接流れない。
- Dashboard entrypoint が Dashboard module 側にある。
- Console module は単一 Pod chat/connect/spawn/resume の責務を主に持つ。
- Dashboard から Console を開く bridge の境界が読み取れる。
- `multi_pod.rs` 相当の責務が Dashboard module 配下へ移され、巨大単一ファイル状態が改善されている。
- render/list/layout、action/lifecycle、diagnostics/tests などの境界が reviewer に分かる形になっている。
- 既存の workspace panel / action model の設計と矛盾しない。
- `yoi panel` の Ticket-centric workspace cockpit という意味を保つ。
- `yoi panel` が scheduler/backend であるかのような実装・説明になっていない。
- 既存テストを維持・更新し、refactor による挙動退行が検出できる状態にする。
- reviewer が diff から、単なる rename ではなく Dashboard / Console の責務境界が改善されたことを確認できる。

## Binding decisions / invariants

- **Dashboard = `yoi panel`**。
- **Console = single-Pod chat/client surface**。
- **TUI = terminal UI implementation layer / crate-level umbrella**。
- `panel` / Dashboard は単一 Pod Console の名前には使わない。
- Console は Dashboard の subordinate mode ではなく、別の user-facing surface として扱う。
- Dashboard は Console の一部ではない。
- Dashboard は workspace-level cockpit/action surface であり、scheduler/backend ではない。
- `yoi panel` command 名は維持する。
- 不要な compatibility alias は追加しない。
- テストが維持できる範囲では、過度に分割を避ける必要はない。大きな file move / module split を許容する。

## Implementation latitude

- 最終的な module 構成は実装者判断でよいが、次の方向を推奨する。

```text
crates/tui/src/
  console/
    mod.rs              # user-facing Pod Console launch API
    app_loop.rs         # current single_pod run loop 相当
    nested.rs           # Dashboard から Pod Console を開く bridge
    terminal.rs         # Console fullscreen/mouse helpers
  dashboard/
    mod.rs              # user-facing Dashboard launch API
    app.rs              # Dashboard app state / selection / key handling
    runtime.rs          # current multi_pod::run loop 相当
    snapshot.rs         # Dashboard snapshot loading
    lifecycle.rs        # companion/orchestrator ensure/observe
    actions.rs          # Ticket/Intake/Companion/Orchestrator actions
    render.rs           # layout/draw/list rendering
    e2e.rs              # dashboard-specific e2e structs/events
    tests.rs or tests/
```

- 互換性や reviewability のため、type 名の rename は段階的でもよい。ただし module / docs / entrypoint は Dashboard / Console 語彙へ寄せる。
- `MultiPodApp` などの既存名は必要なら一時的に残せるが、最終的な方向性が Dashboard であることをコード上から読めるようにする。
- tests は同一 commit 内で移動・更新してよい。test coverage 維持を優先する。
- exact string test がある場合は、新しい Dashboard / Console 呼称に合わせて期待値を更新する。

## Readiness

- readiness: implementation_ready
- risk_flags: [ux-naming, module-boundary, public-cli, test-coverage]

Risk flags は stop gate ではなく reviewer focus として扱う。主な確認点は public CLI/help の不要な互換増加、Dashboard / Console 境界、既存 panel authority/action model の保持、テスト維持。

## Escalation conditions

- `Dashboard` / `Console` の二分では説明しきれない第三の user-facing surface が見つかった場合は確認する。
- `yoi panel` command 名を変える必要が出た場合は確認する。
- `yoi dashboard` などの alias 追加が必要だと判断した場合は確認する。
- module split の過程で Ticket state transition authority、Pod lifecycle、Orchestrator handoff の挙動変更が必要になった場合は確認する。
- テスト維持では覆えない user-visible behavior change が必要になった場合は確認する。

## Validation

- `cargo test -p tui`
- `cargo test -p yoi`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- 必要に応じて CLI help / docs / resources grep で `Dashboard` / `Console` / `TUI` / `panel` / `multi_pod` の残存表現を確認する。

## Related work

- `00001KTCSRS61` Workspace orchestration panel design
- `00001KTCSRS62` Workspace panel action model
- `00001KSKBPYER` Multi-Pod view UI
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/single_pod.rs`
- `crates/tui/src/workspace_panel.rs`
- `crates/tui/src/lib.rs`
- `yoi panel`
