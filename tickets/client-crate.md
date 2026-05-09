# Client crate の切り出し

## 背景

protocol を喋る socket クライアントは現在 `crates/tui/src/client.rs` に閉じている。今後の二つの方向で、TUI 内に閉じていることが障害になる:

- **GUI MVP** (`tickets/native-gui-mvp.md`): GUI バイナリも同じ protocol を喋る。チケット側の検討事項にも「socket client を別 crate に切り出して共有するか、GUI crate 内に閉じて持つか」が挙げられている (`native-gui-mvp.md:67`)。
- **E2E ハーネス** (`tickets/e2e-harness.md`): TUI バイナリを PTY で叩くのは脆く、GUI バイナリは MVP 中。E2E から protocol を直接喋る入口として client crate が要る。

TUI 内に置いたまま GUI と E2E から再利用しようとすると、TUI のレンダリング都合と client の責務が混ざる。先に切り出しておく。

## 方針

- `crates/client/` を新設し、socket への接続・`Method` 送信・`Event` 受信・graceful shutdown までの低レベル機能を移す。
- TUI / GUI / E2E は当 crate を依存先として呼び出す。Pod の subprocess 起動も client crate 側で扱うのが自然か、別 crate / 上位呼び出し側に残すかは設計で詰める（GUI が subprocess を直接 spawn する流儀との整合）。
- 移行は機能等価で、TUI に regression を起こさないこと。

## 検討事項

- crate 名: `client` で良いか、より具体的な名前にするか（`pod-client` 等。`feedback_crate_naming.md` の方針に従いプレフィックスは付けない）。
- subprocess spawn 責務: client crate に含めるか、呼び出し側に残すか。GUI MVP では「GUI から直接 pod を spawn」する流儀なので、spawn と connect を分離して両方公開しておくのが妥当そう。
- 公開 API の境界: 生 socket / `JsonLineReader` までを露出するか、もう一段抽象化したリクエスト/サブスクライブ API にするか。
- 非同期ランタイム: tokio 前提で良いか（GUI の GPUI executor との統合は GUI 側で吸収する）。
- error 型: TUI の `client.rs` 内で持っている error をそのまま move するか、再設計するか。

## 要件

- `crates/client/` が新設され、現 `crates/tui/src/client.rs` 相当の機能を提供する。
- TUI は新 crate に依存して動作し、既存テスト・既存挙動が通る。
- API は GUI / E2E から呼べる粒度で公開されている（最低限: 接続、`Method` 送信、`Event` ストリーム購読、shutdown）。
- pod subprocess を spawn する経路をどこに置くかが決まり、必要なら本 crate からも呼べる。

## 完了条件

- 上記要件が満たされる。
- TUI が新 crate を使って従来通り動く（`cargo test -p tui` / TUI 手動起動で regression 無し）。
- E2E ハーネス（`tickets/e2e-harness.md`）が本 crate に依存して protocol を喋れる状態になる。

## 範囲外

- GUI バイナリ実装そのもの（`tickets/native-gui-mvp.md`）。
- protocol の拡張・互換破壊。
- TUI のリファクタリングを切り出し以上にやること（責務移動だけに留める）。
- daemon 層の導入。

## 依存 / 関連

- `tickets/native-gui-mvp.md`
- `tickets/e2e-harness.md`
- `crates/tui/src/client.rs`
- `crates/protocol/`

## Review
- 状態: Approve
- レビュー詳細: [./client-crate.review.md](./client-crate.review.md)
- 日付: 2026-05-09
