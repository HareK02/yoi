---
title: "E2E テスト戦略"
state: "active"
created_at: "2026-06-09T07:09:26Z"
updated_at: "2026-06-09T07:09:26Z"
linked_tickets: ["20260527-000002-001"]
---

## Goal

Yoi の実プロセス・実 socket・実 provider 境界をまたぐ振る舞いを、通常の crate 内 unit / integration test だけに頼らず検証できる E2E テスト戦略を確立する。

最初の到達点は、実 `pod` / product binary を spawn し、protocol 経由で最小シナリオを実行し、graceful shutdown まで確認できる opt-in E2E harness を持つこと。その上で、permission、resume/fork、spawned Pod、provider stream、TUI/Panel などの重要境界を段階的に増やせる状態にする。

## Motivation / background

現状のテストは crate 内の in-process coverage が厚い一方で、以下の性質は単体テストだけでは十分に確認しづらい。

- 実プロセス spawn と runtime dir / socket / env の相互作用。
- Pod controller / protocol client / session store / metadata / restore の統合挙動。
- provider endpoint、streaming、auth/token、tool call、continuation、retry の実接続に近い振る舞い。
- permission deny、scope、manifest/profile 解決、child Pod delegation など、複数 crate と実 runtime state をまたぐ policy。
- TUI / Panel が前提にする Pod lifecycle や Ticket orchestration の外形。

E2E は常時実行の軽いテストではなく、dogfooding 中に「この機能は実 runtime でも壊れていない」と確認するための opt-in 検証基盤として必要。

## Strategy / design direction

- E2E は通常の `cargo test --workspace` からは外し、明示 feature / 専用 package / 独立 job で opt-in 実行する。
- まずはワークスペース直下の専用 E2E package / harness として設計し、個別 crate の unit test に押し込めない。
- protocol を喋る側は TUI の PTY 操作ではなく、typed client / protocol client を使う方向を優先する。
- provider 依存は最初から全部を対象にしない。
  - 最小 harness では canned / fixture / mock HTTP server を使う。
  - provider 差分は代表 provider から段階的に増やす。
- env / runtime dir / socket path は test ごとに隔離し、並列実行方針を明示する。
  - 必要なら最初は `--test-threads=1` 相当で安全側に倒す。
  - 将来的には per-test runtime dir と typed launch config で並列性を上げる。
- E2E は「全シナリオを大量に持つ」より、重要な runtime seam ごとに少数の高価値 scenario を置く。
- 失敗時 diagnostics は、secret を出さずに process phase、socket path、session id、log path、provider fixture id を辿れる形にする。
- E2E harness 自体が flaky にならないよう、network / time / external auth への依存は明示 opt-in に分ける。

## Success criteria / exit conditions

- `cargo test --workspace` では E2E が走らず、通常開発の feedback loop を重くしない。
- 明示コマンドで E2E harness を実行できる。
  - 例: `cargo test -p e2e --features e2e` または後続で決める同等コマンド。
- 最小 scenario が実 `pod` / product binary を spawn し、protocol 経由で 1 turn 実行し、graceful shutdown まで通る。
- E2E 実行は専用 runtime/data dir を使い、通常の user/workspace state を汚さない。
- fixture / mock provider の設計があり、少なくとも 1 provider 相当の canned response を実 HTTP 経由で返せる。
- failure diagnostics から、spawn 失敗・socket 接続失敗・provider fixture 失敗・protocol 失敗・shutdown 失敗を区別できる。
- 後続 Ticket が permission / resume / fork / spawned Pod / provider streaming / Panel などを追加できる harness boundary がある。

## Decision context

- linked Ticket `20260527-000002-001` は、E2E harness の最初の concrete implementation Ticket として扱う。
- この Objective は E2E 全体の中長期方針・判断軸を保持する。個別 scenario の実装や harness の細部は concrete Ticket に分ける。
- TUI を直接 PTY で叩く方針は初期 harness では避け、protocol/client 経由を優先する。
- provider 全対応は初期 scope にしない。fixture / mock HTTP server を基礎にし、代表 provider から段階的に広げる。
- E2E は flakiness と実行コストが高いため、既定 CI / 既定 workspace test には入れず、opt-in 検証として始める。
- Objective context は判断材料であり、実装 authority は各 Ticket の body/thread/artifacts と明示的な Ticket relation / OrchestrationPlan に置く。
