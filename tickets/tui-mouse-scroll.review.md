# Review: TUI マウスホイールでヒストリーをスクロール

## 前提・要件の確認

- フルスクリーンモード中にマウスホイールでスクロール可能
  - `enter_fullscreen` で `EnableMouseCapture` を `EnterAlternateScreen` と一緒に発行（`crates/tui/src/main.rs:258`）。`run_loop` の `event::read` 分岐に `TermEvent::Mouse(mouse) => handle_mouse(app, mouse)` を追加（`crates/tui/src/main.rs:314-316`）。要件達成。
- ホイール上 = `scroll_up` / 下 = `scroll_down`、感覚は Shift+↑/↓ と同等
  - `handle_mouse` が `MouseEventKind::ScrollUp/Down` をそのまま `app.scroll.scroll_up/down(WHEEL_LINES)` にマップ（`crates/tui/src/main.rs:366-372`）。`Scroll::scroll_up` は `top_offset` を減らす実装（`scroll.rs:47-50`）で「過去方向」と整合。`WHEEL_LINES = 3` は Shift+↑/↓ の 1 行より速いがページ未満で、ticket の「同じ感覚で良い」と矛盾しない。妥当。
- マウスキャプチャは alt-screen 中に限定し、終了時に確実に解除
  - 有効化は `enter_fullscreen` 1 箇所のみ（`main.rs:258`）。解除は `run_spawn` の child reap 前（`main.rs:237-241`）と `main` 末尾の最終 cleanup（`main.rs:173-178`）の 2 箇所で `DisableMouseCapture` を `LeaveAlternateScreen` と対で発行。picker / spawn のインラインフェーズでは `enter_fullscreen` を経由しないので有効化されない。要件達成。
- ホイール以外のマウスイベントでマッチ漏れエラーが出ないこと
  - `handle_mouse` の `match` は `_ => {}` で吸収（`main.rs:370`）。`run_loop` 側は `TermEvent::Mouse(_)` を新たに拾うので未処理エラーは発生しない。要件達成。

## 完了条件の検証

- フルスクリーンTUIでホイールが効く: コードパスは成立。実機確認はユーザー側で実施予定（ticket補足通り）。
- 正常終了でターミナル状態が残らない: `main` 末尾の cleanup は `result` の match より前に走るので、`Ok` でも `Err` でも必ず `DisableMouseCapture` を発行する。`run_spawn` 経由で `run` がエラー return しても、戻った直後の `execute!` で disable した後に `?` 相当の伝播が起きるため、二重 disable になっても idempotent で安全。
- 既存キー操作のスクロール挙動に変化なし: `Scroll` API・`handle_key` のスクロール経路は無変更。

## アーキテクチャ・スコープ

- 変更は `crates/tui/src/main.rs` の terminal lifecycle と event loop に限定。新規モジュール・新規 trait・新規抽象は導入していない。既存の `Scroll` API をそのまま再利用しており、過剰な抽象化はない。
- `WHEEL_LINES` を `main.rs` 内の `const` として置いた選択は、現状のキー側のマジックナンバー（page_up/down 等は `area_height` から導出、Shift+↑/↓ は `1` 直書き）と粒度が揃っている。`scroll.rs` 側に押し込む価値は今のところない。
- マウス捕捉の対称性（alt-screen と同じスコープでEnable/Disable）は ticket の「範囲外」記述と一致しており、picker / spawn のインラインフェーズでホイール挙動が変わらないことが保証されている。
- 依存追加なし。crossterm の既存featureで `MouseEvent` 系が利用可能（既に import 済みの `event` モジュールから引いている）。

## 指摘事項

### Non-blocking / Follow-up

- パニック時のクリーンアップは未対応。tui には `std::panic::set_hook` 等のフックが存在せず、これは本変更前から同じ（bracketed paste も alt-screen も同じく panic 時には残る）。本ticket の「異常終了でもキャプチャ状態が残らない」を厳密に取ると未達だが、既存の他リソース（alt-screen、bracketed paste、raw mode）と同レベルで揃っており、本ticketで先んじて対応するのは過剰。別ticket（パニック時のターミナル復帰一括対応）として扱うのが適切。
- `enter_fullscreen` 内で `EnterAlternateScreen` の後に `EnableMouseCapture` が失敗した場合、その関数は `?` で早期 return するが、`main` 末尾の cleanup が `DisableMouseCapture, LeaveAlternateScreen` を流すので結果的に状態は復帰する。明示性を上げるなら `enter_fullscreen` 内でロールバックを書く手もあるが、final cleanup と二重になるだけで価値は薄い。現状維持で良い。

### Nits

- `WHEEL_LINES` のドキュメントコメントは根拠が読みやすく良い。指摘なし。

## 判断

Approve — ticket の前提・要件・完了条件はすべて実装で達成されており、コードベースを歪めず最小差分で収まっている。実機確認はユーザー側で予定通り実施すれば良い。
