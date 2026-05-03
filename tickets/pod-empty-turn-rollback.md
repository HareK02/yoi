# Pod: 空応答ターンの自動巻き戻し

## 背景

`Method::Run` でユーザーが Submit したあと、AI 応答が一切 history に積まれないまま Pause / Cancel されると、現状の Pod は以下の状態を確定させる:

- `worker.history` に user message と attachment 由来の system message が残る
- `pod.user_segments` に当該入力が push 済み
- `session_store` 側にも `save_user_input` の delta が commit されており、続いて `save_delta`（実体は空 or attachments のみ）/`save_turn_end`/`save_run_completed(interrupted=true)` が積まれて head_hash が前進する

結果として「ユーザー発話だけがあり、AI 応答ゼロ」のターンが履歴に commit され、次の Run はその上に積み増される。Submit を取り消して書き直す・別の話に切り替える、といった操作のたびにノイズが残ってしまう。

人間操作の TUI ではこのケースが頻発するので、Submit 前の状態に丸ごと巻き戻す仕組みがほしい。

## 要件

- Pod controller のターン処理で、`Method::Run` 起点のターンが以下を **両方** 満たす場合は Submit 直前の状態に巻き戻す:
  - 終了が Pause / Cancel 由来である（`Method::Pause` / `Method::Cancel` を受けて `WorkerError::Cancelled` で抜けた経路）
  - そのターンで `worker.history` に LLM 応答に由来する entry（assistant message / tool_use / tool_result）が **一つも** append されていない
- 巻き戻しは Submit 起点で生まれた変更を全て取り消す:
  - `worker.history` を Submit 前の長さに truncate
  - `pod.user_segments` から push した分を pop
  - `pending_attachments` を破棄
  - `session_store` の head_hash を Submit 直前まで戻し、`save_user_input` / `save_delta` / `save_turn_end` / `save_run_completed` の commit を相殺
  - `runtime_dir.write_history` / `write_status` で永続化済みの `history.json` / `status` を同期
- 巻き戻し成立時の最終 status は **Idle**（Resume すべき AI ターンは存在しないため Paused にしない）
- 巻き戻しの有無はクライアントが判別できるようイベントで伝える（`Event::RunEnd` の variant 拡張、または専用イベント）。これにより TUI 側で「巻き戻されたので入力欄に戻した」等のフィードバックが組める。
- 対象は `Method::Run` 起点のターンのみ。Notify 起点の自動 Run（`run_for_notification`）と `Method::Resume` は対象外。
- Mid-run compaction 後の resume で LLM 応答ゼロになるケース（`do_compact_and_resume` 経路）は、Submit 前の history snapshot が依然有効である限り同様に巻き戻せる設計とする。

## 完了条件

- TUI / pod_cli いずれの経路でも、`Method::Run` → AI 応答ゼロで Pause / Cancel すると Pod の in-memory 状態（`worker.history`, `user_segments`, `pending_attachments`, status）が Submit 前と一致する
- 同条件で session_store / `history.json` / `status` の永続側も Submit 前と一致する
- AI 応答が 1 件でも積まれていたターンは従来通り、巻き戻さずに Paused / Idle で確定する
- クライアントが受け取るイベントから巻き戻しの有無が分かる
- ユニットテストで「assistant 応答ゼロでの Cancel」「assistant 応答ゼロでの Pause」「tool_use 1 回のあとの Cancel（巻き戻さない）」「Notify 起点の Cancel（対象外）」の 4 ケースを最低限カバーする

## 範囲外

- 複数ターンに遡る undo（→ `tickets/pod-session-fork.md` で Fork として実装する計画）
- ユーザーの明示操作で「このターンを巻き戻す」を選ばせる UI（自動条件のみ）
- TUI 側で「巻き戻された入力を編集領域に復元する」等の UX（→ `tickets/tui-empty-turn-restore.md`）
