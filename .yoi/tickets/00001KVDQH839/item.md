---
title: 'Panel E2E に shell Enter 起動経路の dashboard readiness 計測を追加する'
state: 'closed'
created_at: '2026-06-18T16:02:56Z'
updated_at: '2026-06-19T05:44:09Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel', 'e2e', 'startup-latency', 'shell-launch', 'dashboard-content-ready']
---

## Background

既存の Panel dashboard readiness E2E は direct `Command::spawn(yoi panel ...)` から dashboard content ready までを測っていた。ユーザー目線の「Enter した瞬間から実コンテンツが表示されるまで」に近づけるため、isolated fixture は維持しつつ、shell 上で command line を投入して Enter された起動経路を部分的に模した E2E を追加する。

この Ticket は live workspace の遅延改善そのものではなく、E2E の起動経路を実利用に近づける追加計測である。

## Requirements

- PTY 上で `/bin/sh` を起動し、`exec <yoi> panel ...` を command line として送って Enter 相当から測定する。
- 測定開始点は command line を PTY に送る直前とする。
- dashboard readiness は既存の `dashboard_content_ready` snapshot matcher を使う。
- first frame だけ、単一 row だけでは通さない。
- Isolated fixture / isolated HOME / XDG dirs / runtime dirs は維持する。
- `YOI_POD_RUNTIME_COMMAND` は tested binary を明示して渡す。
- Direct spawn E2E は残し、shell-enter path は追加 coverage とする。

## Implementation summary

- `PanelHarness::spawn_via_shell_enter` を追加した。
  - `/bin/sh` を PTY 上で起動。
  - `exec '<binary>' '<args>'...` を送信。
  - command line送信直前の `Instant` を返す。
  - artifacts `run.json` に `launch_mode: shell_enter_exec` を記録。
- shell quote helper を追加した。
- `panel_dashboard_content_ready_from_shell_enter_path` E2E を追加した。
  - Enter相当から first frame / dashboard content ready を測定。
  - expected dashboard snapshot / source breakdown を検証。

## Validation

- `cargo test -p yoi-e2e --features e2e --test panel panel_dashboard_content_ready_from_shell_enter_path -- --nocapture`
  - observed: dashboard content ready 約 `220ms`, first frame 約 `20ms` in isolated fixture。
- `cargo test -p yoi-e2e --features e2e --test panel`
- `cargo check -p yoi-e2e -p yoi -p tui --features tui/e2e-test`
- `cargo fmt --check`
- `git diff --check`

## Non-goals

- Live workspace startup latency の改善。
- Interactive shell の command lookup / user typing latency の完全再現。
- User's actual shell rc/profile を読むこと。
- Panel architecture / lifecycle の変更。

## Related work

- `00001KVDETSN6` — Panel startup latency をユーザー目線の dashboard content ready 基準で計測・改善する。
