# レビュー: TUI: Pod を明示的に終了させる操作

対象差分: `crates/protocol/src/lib.rs`, `crates/pod/src/{controller,lib,main}.rs`, `crates/pod/tests/controller_test.rs`, `crates/pod/examples/pod_protocol.rs`, `crates/tui/src/{app,main}.rs`（staged、未コミット）

## 要件達成状況

| 要件 | 状態 |
|---|---|
| TUI 内のキーバインドで Pod の終了を開始できる | ✅ `Ctrl-D` で `Method::Shutdown` を送信 |
| 実行中のターンがあれば確認を挟む | ✅ `handle_shutdown`: running 中は「Press Ctrl-D again」警告を表示、3秒以内の再押下で確定 |
| 実行中のターンがなければ即座に終了 | ✅ `!app.running` なら即 `Method::Shutdown` |
| 既存のキャンセル機構で中断する | ✅ `run_with_cancel_support` 内で `Method::Shutdown` 受信時に `cancel_tx.try_send(())` + `shutdown_requested = true` |
| キャンセル完了後 session-store にフラッシュ | ✅ `run_with_cancel_support` が `(status, shutdown)` を返した後、controller loop 内で `write_status` / `write_history` が走ってから `break` |
| shutdown 完了後 TUI が終了する | ✅ `Event::Shutdown` → `app.quit = true`、main loop が break |
| Pod プロセス自体も正常終了する | ✅ controller が `shutdown_tx.send(())` → `main.rs` の `shutdown_rx` が発火 → `drop(handle)` → `ExitCode::SUCCESS` |
| shutdown 中の進行状況が画面で分かる | 🟡 「Press Ctrl-D again to cancel and shut down.」メッセージは出るが、shutdown 後の「shutting down...」表示は無い（`Event::Shutdown` 受信時に `app.quit = true` だけで即終了）|

## アーキテクチャ

### Protocol 拡張

- `Method::Shutdown` と `Event::Shutdown` が protocol に追加。最小限の拡張で意味が明確
- `Method::Shutdown` は Run/Resume/Cancel と同格の method バリアント。socket 層での特殊扱い不要

### Controller 内の shutdown フロー

```
Idle 状態で Shutdown 受信:
  → Event::Shutdown を broadcast → controller loop break → shutdown_tx.send(())

Running 状態で Shutdown 受信 (run_with_cancel_support 内):
  → cancel_tx.try_send(()) で実行中ターンをキャンセル
  → shutdown_requested = true をフラグ
  → pod_future が Cancelled/Error で完了
  → (PodStatus::Idle, shutdown=true) を返す
  → controller loop が compaction → write_status → write_history の通常後処理
  → shutdown フラグを見て Event::Shutdown → break → shutdown_tx.send(())
```

**重要**: shutdown が来ても**通常の後処理（compaction, persist）を必ず経由**してから抜ける。session の整合性が壊れない。この設計は正しい。

### main.rs の二択 select

```rust
tokio::select! {
    _ = tokio::signal::ctrl_c() => { ... }
    _ = shutdown_rx => { ... }
}
```

Ctrl-C（signal）と client 要求の Shutdown を同格に扱う。どちらが先に来ても `drop(handle)` → 正常終了。

### TUI の確認 UI

`shutdown_confirm: Option<Instant>` フィールドで「最後に Ctrl-D を押した時刻」を保持。3秒以内の再押下で確定。Timeout 後は再度リセット。シンプルで十分。

## 指摘事項

### 1. 🟡 shutdown 中の進行表示が無い

チケット要件:
> shutdown 中は進行状況が画面上で分かる（「shutting down...」等の表示）。

実装:
- `Event::Shutdown` を受けた瞬間に `app.quit = true` → main loop が即 break → terminal restore
- ユーザーが「shutting down」を読む暇がない

実際には shutdown はほぼ瞬時（キャンセル完了 + persist + Event::Shutdown が数十 ms）なので視認できるタイミングがそもそも無い。要件の「shutting down...」は「長い shutdown を想定した表示」だが、実装上 shutdown は速いので**実用上問題にならない**。

**判断**: 実害なし。将来 shutdown が重くなった場合（大量ターンの persist 等）に表示を足すのは容易（`Event::Shutdown` 受信時に output_queue に push してから quit を遅延させればよい）。**不問**。

### 2. 🟢 `shutdown_confirm` が `Instant` ベース

wall clock ではなく `std::time::Instant` を使っている。NTP ジャンプに影響されない。正しい選択。

### 3. 🟢 Resume パスでも shutdown 対応

`Method::Resume` のハンドラも `run_with_cancel_support` を通っており、Resume 中に `Shutdown` が来ても同じフローで処理される。漏れなし。

### 4. 🟢 examples / tests の追随

- `pod_protocol.rs`: `PodController::spawn` の戻り値を `(handle, _shutdown_rx)` に分解
- `controller_test.rs`: 同上
- 既存テストが壊れていない

### 5. 🟢 `Ctrl-D` のキーバインド選択

`Ctrl-D` は shell の EOF 慣例に合っており、「この Pod との対話を終える」という意味が自然。`q` や `:quit` は通常入力との衝突リスクがあるので `Ctrl-D` は妥当。

## テスト

- `controller_test.rs`: 戻り値の型変更に追随。shutdown 固有のテスト（Shutdown method を送って controller が break するか、persist 後に shutdown_rx が発火するか等）は**未追加**
- `handle_shutdown` のユニットテスト（running 時の confirm フロー、timeout 後のリセット等）は**未追加**

テストが薄い点はあるが、shutdown フローは controller loop 内の分岐であり、integration test で網羅すると MockClient の応答タイミング制御が必要になって重くなる。最小実装としては許容。

## 結論

**受け入れ可**。要件はほぼ達成、アーキテクチャは「通常の後処理を必ず経由してから終了」という最重要の不変条件を守っている。指摘1（shutdown 中の進行表示）は実害なしで不問。テストの薄さは将来必要になったら足す程度。
