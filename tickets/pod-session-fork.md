# Pod: 任意ターンからの Fork（複数ターン巻き戻し）

## 背景

`tickets/pod-empty-turn-rollback.md` は「直近 Submit が AI 応答ゼロのまま中断された」極めて狭いケースだけを自動で巻き戻す簡易フォーム。それを超える「3 ターン前から別の方針でやり直したい」「ある分岐は捨てて別ルートを試したい」といった **複数ターン巻き戻し** は、過去ターン境界からの Fork として実装する。

session_store には既に primitive が揃っている:

- `session::fork(state)` — 現状から新 session_id へ分岐（`crates/session-store/src/session.rs:400`）
- `session::fork_at(source_id, at_hash)` — 既存セッションログ上の任意 entry hash から分岐（同 :424）
- `SessionOrigin { session_id, at_hash }` — `SessionStart.forked_from` に出自を記録

未着手なのは Pod / protocol / クライアントへの露出と、ターン境界 ↔ entry hash の対応付け。

## 要件

- Pod に「現セッションから Fork して新セッションへ切り替える」操作を追加。Fork 起点はターン境界で指定する:
  - protocol に新 Method（仮: `Method::Fork { from: ForkPoint }`）を追加
  - `ForkPoint` は最低限「ターン番号」「entry hash」のいずれかで起点を指す
  - ターン番号 → entry hash の解決は Pod / session_store 側で行う（`save_turn_end` のログ entry を境界として使うのが自然）
- Fork 後の Pod 状態:
  - 新 session_id を active に切り替え、worker.history を fork 起点までの内容で再構築
  - 元セッションは破壊されない（後から switch back 可能な前提を残す）
  - 走行中（Running / Paused）状態での Fork は拒否し、Idle 限定
- Fork ツリーが追跡可能であること:
  - `forked_from` chain が session_store のログから辿れる（既存挙動の確認込み）
  - クライアントから「このセッションの祖先 / 子孫」を引ける API（最低限、`SessionOrigin` を読める形）
- pod_cli / TUI からの呼び出しインターフェースの設計:
  - 本チケットで protocol 上の Method は確定させる
  - pod_cli の引数仕様もここに含める（最低限 `pod fork --turn N` 程度）
  - TUI 側の UX（ターンを選択して fork する操作）は別チケット
- セッション切り替え後の `runtime_dir` の扱いを decide:
  - 1 つの runtime に対して active session が切り替わる形
  - セッションごとに別 runtime を持つ形
  - のどちらが今の構成と整合するか調査の上で決定

## 完了条件

- `Method::Fork` で過去ターン起点の新セッションが作成され、そこから `Method::Run` で続行できる
- 元セッションが fork 後も独立に存在し、別 Pod プロセスから resume できる（破壊されていない）
- Fork ツリーが session_store のログから機械的に辿れる
- pod_cli から fork → 新セッションでの run まで通せる
- `pod-empty-turn-rollback` の自動巻き戻しと共存（自動巻き戻しは本機能を使わずに従来通りの直接 truncate 方式で良い、と決めるならその根拠も明記）

## 範囲外

- Fork ツリーの可視化 UI（TUI / GUI）— 別チケット
- TUI 上での「ターンを選んで fork」UX — 別チケット
- 異なる Fork 間でのマージ
- 過去ターンの物理削除型リベース（fork は常に非破壊）
- 自動 GC / 古い fork の整理

## 依存 / 関連

- `tickets/pod-empty-turn-rollback.md`（直近 Submit のみの自動巻き戻し。本チケットは汎用 fork）
- session_store の既存 `fork` / `fork_at` を流用
