---
title: 'Orchestrator Ticket event Companion notify の peer registration / diagnostics を修正する'
state: 'done'
created_at: '2026-06-18T14:33:09Z'
updated_at: '2026-06-18T14:33:50Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['orchestrator', 'companion', 'peer-notify', 'ticket-event', 'auto-run-false', 'diagnostics']
---

## Background

`00001KTTW04W2` で Orchestrator role の明示 Ticket lifecycle event を workspace Companion に `Notify { auto_run: false }` で送る hook は実装済みだった。しかし live 状態では Orchestrator metadata 側に workspace Companion peer が無いと、`send_weak_notify_to_live_peer` が silent no-op になり、Companion に通知が届かない。

現在の運用では workspace Companion `yoi` と `yoi-orchestrator` が同じ workspace の role Pod として存在していても、peer metadata が片方向または欠落することがある。この場合、通知 hook は実行されても delivery 前提を満たせず、ユーザーには何も見えない。

この Ticket では peer 境界を緩めず、workspace Companion / Orchestrator の reciprocal peer relationship を通知前に保証し、delivery skip reason を bounded diagnostic として残す。

## Requirements

- Orchestrator Ticket event Companion notify は `auto_run: false` を維持する。
- Companion missing / stopped / unreachable では spawn / restore しない。
- Peer visibility check は維持する。
  - arbitrary Pod name へ notify できるようにはしない。
- Orchestrator startup / hook install 時に、既存 workspace Companion metadata があれば reciprocal peer を ensure する。
- Ticket event hook 実行時にも、既存 workspace Companion metadata があれば reciprocal peer を ensure する。
  - 既存 running state で片方向 peer / missing peer があっても回復できるようにする。
- `send_weak_notify_to_live_peer` は bool ではなく delivery reason を返す。
  - delivered
  - missing metadata
  - not visible
  - visible but not peer
  - not live / unreachable
  - send failed
- Delivery skip / failure reason は bounded tracing diagnostic として確認できる。
- Missing Companion は debug/no-op に留める。
- Ticket event hook は passive Ticket read/list/show では発火しない既存条件を維持する。

## Implementation summary

- `PodDiscovery::ensure_existing_peer` を追加し、peer metadata が存在する場合だけ reciprocal peer registration を行う。
- `register_peer` は `ensure_existing_peer` を使う形に整理し、missing peer は従来どおり `MissingPod` error を返す。
- `WeakNotifyDelivery` を追加し、weak notify delivery result を reason 付きで返すようにした。
- Orchestrator Ticket event hook install 時に existing Companion peer を ensure する。
- Ticket event hook call 時にも existing Companion peer を ensure してから weak notify する。
- Delivery skipped / send failed を `warn!`、missing Companion / ensured peer を `debug!` で記録する。
- Tests を追加・更新し、peer が事前登録されていない既存 Companion metadata でも hook が reciprocal peer を作り、`Notify { auto_run: false }` を届けることを確認した。

## Acceptance criteria

- Existing workspace Companion metadata がある場合、Orchestrator Ticket event notify 前に reciprocal peer が ensure される。
- Orchestrator Ticket event hook は peer 未登録の既存 Companion に `Notify { auto_run: false }` を届けられる。
- `send_weak_notify_to_live_peer` は delivered / skipped reason を区別して返す。
- Spawned-child visibility など peer ではない Pod には weak notify しない。
- Missing Companion では spawn / restore せず no-op diagnostic に留める。
- Passive Ticket tool call では通知しない既存挙動を維持する。

## Validation

- `cargo test -p pod discovery::tests::register_peer_persists_reciprocal_metadata --no-default-features`
- `cargo test -p pod weak_notify --no-default-features`
- `cargo test -p pod ticket_event_notify --no-default-features`
- `cargo check -p pod -p tui --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi --no-link`

## Related work

- `00001KTTW04W2` — Orchestrator進捗をAutoKickなしでCompanionへ通知する。
