# TUI: 巻き戻された入力の編集領域への復元

## 背景

`tickets/pod-empty-turn-rollback.md` で Pod 側に「Submit 後 AI 応答ゼロで Pause/Cancel した場合に Submit 前の状態へ自動巻き戻す」仕組みを入れる。Pod は巻き戻しの有無を `Event::RunEnd`（または専用イベント）で TUI に通知する。

TUI 側は現状 `submit_input` 時点で `input.clear()` を呼んでおり、Submit 後はテキストが失われる。Pod が巻き戻したのに TUI 側だけ「空入力欄 + ターンヘッダーが画面に残った状態」では UX が破綻する。Pod の巻き戻しに合わせて、TUI も Submit 前と同等の見た目に戻す。

## 要件

- Pod から巻き戻し通知を受け取った場合:
  - 直近 Submit でレンダリングしたブロック（`Block::TurnHeader`, `Block::UserMessage`, それに付随する `[File: ...]` 等の attachment ブロック、`Block::TurnStats` 等の Run 由来ブロック全般）を画面から取り除く
  - 入力欄に当該 Submit のテキスト（typed segments）を復元し、カーソル位置はテキスト末尾
  - `running` / `paused` / `assistant_streaming` / `current_tool` / `run_*_tokens` 等のターン状態は Idle 相当にリセット
- 復元は 1 ターン分のみ（複数巻き戻しは Pod 側でも対象外）
- 入力欄に未送信の文字（Run 中にユーザーが書き始めたもの、tui-input-queue が入ったらキュー内容）が既にある場合の振る舞いを決める:
  - 既入力を上書きしない方針が素直（巻き戻し復元分は別バッファに置いて「↑キーで呼び出し」等にする手もある）
  - tui-input-queue とのバッティングはチケット着手時に再確認
- 巻き戻された旨をユーザーに 1 行のヒント（status line / トースト等）で出す。サイレントに戻すと「画面が消えて何が起きたか分からない」事故になりやすい

## 完了条件

- TUI で Submit → AI 応答が始まる前に Pause/Cancel すると、画面が Submit 前と一致し、入力欄に元のテキストが戻っている
- AI が 1 トークンでも応答した後の Pause/Cancel では従来通り（巻き戻さず Paused / Idle）
- 巻き戻しが起きたことがユーザーから視認できる

## 範囲外

- 複数ターン巻き戻し（→ `tickets/pod-session-fork.md` で Fork として実装する計画）
- 巻き戻し履歴のスタック化（直前 1 件のみ復元）
- pod_cli / 他クライアントの同等 UX（本チケットは TUI 限定）

## 依存

- `tickets/pod-empty-turn-rollback.md` の Pod 側実装 + 巻き戻し通知イベントの確定
