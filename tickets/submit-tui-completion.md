# サブミット入力: TUI 補完 + 型付き atom 化

## 背景

`tickets/submit-segment-protocol.md` で protocol が `Vec<Segment>` を運べるようになった前提で、TUI 側に「`@` / `#` / `/` を打鍵中に候補を出し、確定したら typed atom (= 送出時の `Segment`) に昇格させる」UX を載せる。

`@` / `#` / `/` は TUI の入力アフォーダンスであって protocol contract ではない（GUI などは同じ intent をボタンや picker で表す）。本チケットは TUI 限定の UX に閉じる。

## 要件

### token 検出と昇格

入力中に prefix を検出して候補を浮かせる:

- `@<部分パス>` — workspace 内のファイル / ディレクトリ候補
- `#<部分slug>` — Knowledge slug
- `/<部分slug>` — Workflow slug + client-side コマンド（`/clear` など）

トリガー条件:

- prefix 記号の直前が「行頭 or 空白 or paste 等の indivisible atom」のときのみ候補ポップアップを開く
- 開いた後に prefix 直後がスペースになった時点（例: `@ `, `/ `）で候補を閉じ、通常テキストに戻す

確定（Tab / Enter 等、入力 UX の詳細は実装で）で対象範囲を `Atom::FileRef` / `Atom::KnowledgeRef` / `Atom::WorkflowInvoke` の indivisible atom に置き換える。挙動は既存の `Atom::Paste` と同等（cursor は中に入れない、Backspace で塊ごと削除）。submit 時に対応する `Segment` 変種に変換して送る。

### 候補列挙のための protocol query

補完用に Pod へ問い合わせる軽量経路を追加。`Method::GetHistory` と同じパターンを踏襲する: 専用 `Method` variant を受信した IPC server 層 (`crates/pod/src/ipc/server.rs`) で Controller を介さず直接処理し、対応する `Event` 変種を同ソケットに write back する（broadcast には流さない）。

扱う query:

- ファイル候補（Pod scope 内、prefix マッチ）— 本チケットで実装
- Knowledge / Workflow slug 候補（kind 指定 + prefix マッチ）— wire の枠だけ用意し、resolver 未登録時は空応答。実体はそれぞれのチケット側

### Pod 側ファイル resolver（auto-read 切り出し）

現状 `crates/pod/src/compact/worker.rs` の `mark_read_required` ツールと `crates/pod/src/pod.rs` の再読ロジックに散らばっている auto-read 機構を、「Pod から見たファイルシステム操作」を担う独立モジュール（または新規クレート）に集約する:

- 既存の auto-read（`ScopedFs` 経由の読み込み + budget 管理）をこのモジュールに移動
- 補完候補の prefix マッチ列挙を同モジュールに新設

このモジュールは Pod の Interceptor / Hook 経路から呼び出される。compact からの利用も新モジュール経由に切り替える。memory / workflow チケットの resolver はここには含めない。

### 表示

確定後の atom は paste と同じ「indivisible chip」スタイルで描画する。`@` / `#` / `/` ごとに色を変える程度の差異化を入れる。`Block::UserMessage` 側でも同一スタイルで再描画する（`Event::UserMessage` が typed segment で来る前提）。

## 範囲外

- Knowledge / Workflow slug の Pod 側 resolver 実装（それぞれ memory / workflow チケット側で実装。本チケットでは wire の枠と空応答のみ）
- 候補スコアリング、fuzzy search、preview 等の高度な補完体験
- リッチクライアント（GUI / web）の同等 UX

## 完了条件

- `@` / `#` / `/` を打鍵すると候補が出て、確定で chip 化される
- chip 化された atom が対応する `Segment` として Pod に送出され、`Event::UserMessage` で戻ってきた typed segment が同じ見た目で再描画される
- 候補列挙の query / response が wire を通る
- 既存ビルド・テストを壊さない

## 依存

- `tickets/submit-segment-protocol.md`

## 参照

- `crates/tui/src/input.rs`（`Atom` 体系の拡張）
- `crates/tui/src/app.rs`（`submit_input`、`Block::UserMessage` 描画）
- `crates/pod/src/ipc/server.rs`（`GetHistory` パターン: Method 受信 → 同ソケットに Event を直接 write back）
- `crates/pod/src/compact/worker.rs` / `crates/pod/src/pod.rs`（切り出し対象の auto-read 機構）
- `docs/plan/memory.md` §retrieval 経路（slug 補完対象）
- `docs/plan/workflow.md` §呼び出しと依存

## Review

- 状態: Approve
- レビュー詳細: [./submit-tui-completion.review.md](./submit-tui-completion.review.md)
- 日付: 2026-04-30
