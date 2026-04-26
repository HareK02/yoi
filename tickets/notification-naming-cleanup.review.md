# Review: Notification 用語のリネーム

## 前提・要件の確認

### outbound (Pod → Client) — Alert 系
- `Event::Notification(Notification)` → `Event::Alert(Alert)`: `crates/protocol/src/lib.rs:151`, `crates/protocol/src/lib.rs:178-185` で完了。
- `NotificationLevel/Source` → `AlertLevel/AlertSource`: `crates/protocol/src/lib.rs:187-201` で完了。serde の `rename_all = "snake_case"` も維持されており wire 名は `warn` / `error` / `pod` / `worker` / `compactor` / `agents_md` のまま (元と一致)。
- `notifier.rs` → `alerter.rs`、`Notifier` → `Alerter`、`notify()` → `alert()`: `crates/pod/src/ipc/alerter.rs:26-87` で完了。`subscribe_with_snapshot` の戻り値も `(Vec<Alert>, …)` (l.78)。
- `Block::Notification` → `Block::Alert`: `crates/tui/src/block.rs:24-28`。レンダリング側 (`crates/tui/src/ui.rs:316-335`) も match arm が更新されており、style (`MessageKind::NoticeWarn/Error` と prefix `[notice]` / `[notice error]`) は据置 — 範囲外要件を尊重している。
- イベント名のテスト: `event_alert_format` (`crates/protocol/src/lib.rs:464-479`) で `event=alert` の wire 形を確認。

### inbound (External → Pod) — Notify 据置 + 衛生整理
- `Method::Notify` 据置: `crates/protocol/src/lib.rs:18` 健在、`method_notify_json_roundtrip` (l.343) も保持。
- `notification_buffer.rs` → `notify_buffer.rs`、`NotificationBuffer/PendingNotification` → `NotifyBuffer/PendingNotify`: `crates/pod/src/ipc/notify_buffer.rs:20-67`。
- `format_notification` → `format_notify`: 同 l.74。
- `notify_wrapper` プロンプト名と `[Notification]` ラベル据置: `crates/pod/src/ipc/notify_buffer.rs:78,119` および `crates/pod/src/prompt/catalog.rs:447,484` の test asserts、`crates/pod/tests/controller_test.rs:413` の test も `[Notification]` を期待 — 範囲外要件と整合。

### protocol wire 互換
- 旧 `notification` event 名を吸収する `#[serde(rename)]` 等は入っていない (`crates/protocol/src/lib.rs:151` の `Alert(Alert)` は `rename_all = "snake_case"` 経由で wire 上 `event=alert` に変わる)。チケットの「wire 互換は意図的に切る」方針通り。

### 完了条件
- ビルド通過 (cargo build): pod / protocol / tui で warning は既存の `end_scope` のみ、本変更に起因しない。
- テスト: `cargo test -p pod -p protocol -p tui` で 205 件 PASS (ユニット+統合+doc 含む。0 fail / 0 ignored)。
- grep: `Notifier|NotificationLevel|NotificationSource|NotificationBuffer|PendingNotification|notification_buffer|push_notification|attach_notifier|notifier` がコード上で 0 ヒット。残る `Notification` 一致は LLM 向け wrapper ラベル `[Notification]` (据置) と inbound 概念を指す散在コメントのみ。

## アーキテクチャ・スコープ
- レイヤ境界尊重: 変更は protocol / pod (ipc, controller, pod 本体) / tui に限定。llm-worker は触れていない。
- 命名方針: `Alerter` / `alert()` の対 `Notify` (動詞) という非対称設計はチケット背景と一致しており、用語衝突を解消する最小限の手当てになっている。
- 不必要な変更の有無: outbound 一式の rename と inbound 側の衛生 (`NotifyBuffer` / `PendingNotify` / `format_notify`) は要件に明記されている範囲のみ。`pending_notifications` フィールド名 → `pending_notifies` や `push_notification` → `push_notify` まで踏み込んだのは、フィールド型自体が `NotifyBuffer` / `PendingNotify` に変わっていることを踏まえると naming consistency 上妥当。`run_for_notification` メソッド名は `Method::Notify` 由来の "notify-driven run" を表すので残しても問題ない (今回触っていない)。
- ファイル rename: ticket 通り `notifier.rs` → `alerter.rs`、`notification_buffer.rs` → `notify_buffer.rs` の 2 つだけで、過剰な移動なし。`mv` を使った旨 (history 保全) も妥当。

## 指摘事項

### Non-blocking / Follow-up
- `crates/pod/src/controller.rs:49` の doc comment が `Thin wrapper over `Alerter::notify`` のまま (実体は `Alerter::alert`)。1 行修正で解消可能。

### Nits
- `crates/protocol/src/lib.rs:39` の "via the notification buffer" や `crates/pod/src/controller.rs:255,258,346,474,480,657` の "notification buffer" 等の散在コメントは、文脈上 inbound (Notify) の話なので意味は通る。ただし型名が `NotifyBuffer` に揃ったので "notify buffer" に揃えると読み手のノイズが減る。ticket の「衛生整理」を保守的に解釈すれば任意項目。
- `crates/pod/src/pod.rs:91,330,363,365,372,581,585`、`crates/pod/src/ipc/interceptor.rs:4,43,46`、`crates/pod/src/ipc/event.rs:10`、`crates/pod/src/prompt/agents_md.rs:23` 等の doc comment の "notification" も同様 — inbound 側は "notify" 動詞・"alert" 名詞 (outbound) の使い分けが定義されたので、コメントもこの軸で揃え直すと表面の用語衝突がさらに減る。本リネームの影響範囲外と見るなら据置で問題ない。

## 判断
Approve with follow-up — 要件は完全に満たされ、ビルド/テストもクリーン。`controller.rs:49` の doc comment 修正は trivial なので、別チケット化せず本作業の最後に直しておくのが望ましい。コメント表面の用語ノイズは Nits で残るが、ticket の rename スコープ内で必要な分は果たされている。
