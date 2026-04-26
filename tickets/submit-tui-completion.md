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

確定（Tab / Enter 等、入力 UX の詳細は実装で）で対象範囲を `Atom::FileRef` / `Atom::KnowledgeRef` / `Atom::WorkflowInvoke` の indivisible atom に置き換える。挙動は既存の `Atom::Paste` と同等（cursor は中に入れない、Backspace で塊ごと削除）。submit 時に対応する `Segment` 変種に変換して送る。

### 候補列挙のための protocol query

補完用に Pod へ問い合わせる軽量経路を追加:

- ファイル候補（scope 内、prefix マッチ）
- Knowledge / Workflow slug 候補（kind 指定 + prefix マッチ）

`Event` ストリームに載せる性質ではないため、request/response 形式を新設する（具体形式は実装で判断、既存 `Method` の枠に増やすか別経路を作るかも実装側で決める）。

### 表示

確定後の atom は paste と同じ「indivisible chip」スタイルで描画する。`@` / `#` / `/` ごとに色を変える程度の差異化を入れる。`Block::UserMessage` 側でも同一スタイルで再描画する（`Event::UserMessage` が typed segment で来る前提）。

### client-side `/<slug>` の dispatch

`/clear` のような client 完結コマンドは Pod に送らず TUI 内で処理する。TUI 内に簡易な dispatch 表を持ち、未知の `/<slug>` は `Segment::WorkflowInvoke` として送る。初期 dispatch 表は `/clear` 程度で良く、拡張は別途。

## 範囲外

- Pod 側の resolver 実装（memory / workflow チケット）
- 候補スコアリング、fuzzy search、preview 等の高度な補完体験
- リッチクライアント（GUI / web）の同等 UX

## 完了条件

- `@` / `#` / `/` を打鍵すると候補が出て、確定で chip 化される
- chip 化された atom が対応する `Segment` として Pod に送出され、`Event::UserMessage` で戻ってきた typed segment が同じ見た目で再描画される
- 候補列挙の query / response が wire を通る
- `/clear` が client-side で処理され、Pod には届かない
- 既存ビルド・テストを壊さない

## 依存

- `tickets/submit-segment-protocol.md`

## 参照

- `crates/tui/src/input.rs`（`Atom` 体系の拡張）
- `crates/tui/src/app.rs`（`submit_input`、`Block::UserMessage` 描画）
- `docs/plan/memory.md` §retrieval 経路（slug 補完対象）
- `docs/plan/workflow.md` §呼び出しと依存
