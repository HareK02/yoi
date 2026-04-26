# サブミット入力: protocol Segment 化

## 背景

現状の `Method::Run { input: String }` はユーザー入力を素の文字列で運んでいる。TUI の `Atom::Paste`（bracketed paste で受けた塊）は表示用には `[Clipboard #N | X chars, Y lines]` ラベルを持っているが、submit 時に `InputBuffer::submit_text` で flatten され、Pod や session log にはただの長い user message として伝わる。

memory / workflow 導入に伴い、submit には paste 以外にも以下を載せたくなる:

- `@<path>` のファイル参照 + auto_read
- `#<slug>` の Knowledge 参照（`docs/plan/memory.md`）
- `/<slug>` の Workflow 起動（`docs/plan/workflow.md`）

これを protocol 上で **String + marker パース**で扱うと、Pod が正規表現と escape 規則を抱え、code block 内の `#foo` を literal にするか等の ambiguity と escape 漏洩が常に付きまとう。さらにリッチクライアント（GUI / web / ネイティブ）はファイル添付や paste を file chip / clipboard card として描くため、protocol が String だと「chip → string marker → Pod で再 parse → 表示用に再構築」という 3 段 round-trip を全 client で再実装する形になる。

protocol を typed segment の列に上げると、Pod は parser を持たず resolve に集中でき、client は自分の表現単位のまま intent を送れる。最低限 `Segment::Text(String)` を fallback として置くので、CLI piping 等の dumb client は 1 segment を作るだけで従来通り動く（protocol の dictation が「Text を作れること」に閉じる）。

## 要件

### protocol

`Method::Run` の payload を `String` から `Vec<Segment>` に置き換える。Segment は最低限以下の variant を持つ:

- `Text` — 自由文字列。dumb client の fallback
- `Paste` — TUI 等の bracketed paste 由来の塊。`id` と本文を持つ
- `FileRef` — `@<path>` の auto_read 対象
- `KnowledgeRef` — `#<slug>` の Knowledge 参照
- `WorkflowInvoke` — `/<slug>` の Workflow 起動

`Event::UserMessage` の payload も同じ `Vec<Segment>` に揃え、複数 client で見たり log replay する際に typed のまま再構築できる状態にする。

未知 variant に対する forward compatibility（`#[serde(other)]` 相当の吸収）を入れる。

### Pod 側 resolve

Pod は `Vec<Segment>` を LLM context へ flatten する単一経路を持つ:

- `Text` → そのまま
- `Paste` → 本文を inline 展開（現状 TUI が flatten してきたのと同じ結果）。**session log / Event::UserMessage 上ではラベル化情報を保持**し、表示時に `[Clipboard #N | X chars]` を再構築できること
- `FileRef` → scope 内なら本文 read + inline、scope 外は明示エラー
- `KnowledgeRef` / `WorkflowInvoke` → resolver が登録されていれば展開、未登録はフォールバック（後述）

resolver の trait 化と memory / workflow 用 resolver 実装は別チケット。本チケットは **Text と Paste を typed のまま end-to-end で運び切る経路**で完結させる。`FileRef` 以降は variant 定義と「resolver 未登録時のフォールバック」だけ仕込んで実装は後続に委ねる。

### TUI 側

`InputBuffer::Atom::Paste` を submit 時に flatten せず、`Segment::Paste` として送出する。`Event::UserMessage` を typed segment で受けて `Block::UserMessage` を typed のまま描画（paste は引き続き magenta ラベル）。

text しか作れない client が引き続き存在しても良いことを protocol 仕様に明記する（`vec![Segment::Text(_)]` のみで動く）。

### unknown variant / 未登録 resolver の扱い（要決定）

新 variant を持つ client が古い Pod に投げる、または resolver 未登録の variant を Pod が受けるケースの挙動を本チケットで決める。候補:

- (a) 黙って drop — 静かに情報が落ちて user/LLM 双方が気付けない
- (b) `[unknown input: kind=foo]` 相当の placeholder を LLM context に差し込み、LLM が気づいて指摘できる
- (c) hard error で submit 拒否

(b) を仮の第一候補として実装方針を詰める。serde 側は unknown variant を吸収する形を初期から入れる。

## 範囲外

- `@` / `#` / `/` token の TUI parsing と補完 UI（`tickets/submit-tui-completion.md`）
- KnowledgeRef / WorkflowInvoke の resolver 実装（memory / workflow チケット）
- FileRef の引数拡張（行範囲、glob 等）

## 完了条件

- `Method::Run` / `Event::UserMessage` が `Vec<Segment>` で wire を通る
- TUI から paste した内容が `Segment::Paste` で送出され、Pod の LLM context では本文展開、Event 経由の再描画では `[Clipboard #N | ...]` が復元される
- `vec![Segment::Text(_)]` だけ送る client は従来通り動く
- 未登録 variant の扱いが決定済みルール通りに動く（決定は本チケット内で合意）
- forward compat: 未知 variant を含む payload を deserialize しても panic / parse error にならない
- 既存ビルド・テストを壊さない

## 参照

- `docs/plan/memory.md` §retrieval 経路 / §Knowledge の呼び出し制御
- `docs/plan/workflow.md` §呼び出しと依存
- `crates/protocol/src/lib.rs`（`Method::Run`, `Event::UserMessage`）
- `crates/tui/src/input.rs`（`Atom::Paste`, `submit_text`）
- `crates/tui/src/app.rs`（`submit_input`, `Block::UserMessage` 描画）
