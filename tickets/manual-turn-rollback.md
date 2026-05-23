# Pod/TUI: 手動 rollback 導線

## 背景

`pod-empty-turn-rollback` / `tui-empty-turn-restore` により、AI 側出力が 0 の interrupted turn については Pod 側で自動 rollback し、TUI 側で入力を復元できるようになった。

一方で、rollback substrate は直前 Run の状態復元に使える形で入り始めているが、ユーザーが明示的に rollback を要求する導線はまだない。誤送信、モデル選択ミス、途中で方針を変えた場合などに、ユーザーが手動で直前状態へ戻す手段が必要になる可能性がある。

詳細な UX / rollback 対象範囲 / safety policy は未決定のため、本チケットでは要求を保持し、実装方針は着手時に確定する。

## 要件メモ

- ユーザーが明示的に rollback を要求できる導線を用意する。
  - TUI system command / keybinding / tool / protocol Method のどこに置くかは未決定。
  - 最初は TUI から直前 turn を rollback する導線が候補。
- rollback 対象範囲を決める。
  - 直前 submit のみか。
  - assistant output がある turn を許可するか。
  - tool call / tool result が含まれる turn を許可するか。
  - 複数 turn rollback は `pod-session-fork` との関係を確認する。
- safety policy を決める。
  - user-visible assistant output を消す場合は確認を要求するか。
  - tool side effect が既に発生した turn を rollback できるのか、履歴から消すのではなく fork に誘導するのか。
  - rollback が history/context 永続化原則を壊さないようにする。
- TUI 側の表示を決める。
  - rollback 成功 / 失敗の通知。
  - 消された blocks の扱い。
  - rollback された input を composer に戻すか、history/backup に置くか。
- protocol signal を整理する。
  - 既存 `RunResult::RolledBack` を再利用できるか。
  - 手動 rollback は RunEnd ではなく専用 Event / Method が必要か。

## 完了条件（詳細未確定）

- 手動操作で rollback を要求できる。
- rollback 成功時、Pod の session log / SegmentLogSink mirror / TUI 表示が整合する。
- rollback 失敗時、理由がユーザーに見える。
- tool side effect や assistant output を含む turn の扱いが仕様として明示されている。
- tests がある。
- `cargo fmt --check`
- `cargo check --workspace`
- 関連 crate の tests。

## 範囲外

- 複数ターン rollback / 過去地点からの本格的なやり直し（`pod-session-fork` と調整）
- rollback 履歴スタック
- tool side effect の undo
- fork tree 可視化

## 関連

- `tickets/pod-session-fork.md`
- 完了済み: `pod-empty-turn-rollback`
- 完了済み: `tui-empty-turn-restore`
