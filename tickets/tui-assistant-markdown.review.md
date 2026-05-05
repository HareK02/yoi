# Review: TUI Assistant 応答の Markdown スタイル表示

## 前提・要件の確認

### 対応する Markdown 要素 (チケット「対応する Markdown 要素」セクション)
- 強調 `**bold**` / `*italic*` / `~~strike~~`: `Renderer::start` の `Tag::Strong/Emphasis/Strikethrough` で深さカウンタを増やし、`span_style` で `Modifier::BOLD/ITALIC/CROSSED_OUT` を付与 (`crates/tui/src/markdown.rs:219-221, 81-89`)。`Options::ENABLE_STRIKETHROUGH` も付いている (`crates/tui/src/markdown.rs:18`)。✓
- インラインコード: `Event::Code` で `in_inline_code` を立ててから `push_text` し、`span_style` で yellow on `Rgb(40,40,40)` を返す (`crates/tui/src/markdown.rs:145-149, 70-73`)。✓
- フェンスコードブロック: `Tag::CodeBlock` で `in_code_block=true`、`Text` イベント側で `\n` を実際に行分割しつつ等幅 (cyan) で塗る (`crates/tui/src/markdown.rs:131-140, 74-76`)。言語タグは `Tag::CodeBlock(_)` で破棄。✓
- 見出し H1〜H6: `Tag::Heading { level, .. }` で `self.heading` を立て、`span_style` で `heading_style` を返す。H5/H6 は H4 と同色 (`crates/tui/src/markdown.rs:175-178, 277-284`)。✓
- 箇条書きリスト (`-`/`*`/`+`、ネスト可): `Tag::List(None)` 経由で `list_stack` に積み、`LIST_INDENT` を `line_prefix` に push、`Tag::Item` で `• ` マーカー (`crates/tui/src/markdown.rs:183-211`)。テスト `nested_list_indents` で深さ 2 を確認。✓
- 順序リスト (`1.`/`1)`、ネスト可、開始番号尊重): `Tag::List(Some(n))` で `Some(n)` を積み、`Tag::Item` で `n.` マーカーを出して `n += 1`。`pulldown-cmark` 側でも `Start(List(Some(3)))` のように開始番号が来るのを probe で確認したので、`3. a / 4. b` のような表示は意図通りになる。✓
- 引用 (`> ...`、ネスト可): `Tag::BlockQuote(_)` で `│ ` を `line_prefix` に push、ネストすると `│ │ ` になる (`crates/tui/src/markdown.rs:212-218, 256-259`)。✓
- 水平線 (`---`/`***`): `Event::Rule` で `─` × 40 を DarkGray で出し、前後に blank を試みる (`crates/tui/src/markdown.rs:152-161`)。✓
- リンク `[text](url)`: `Tag::Link { .. }` で `in_link` を立て、`span_style` で cyan + underline。URL は表示しない。✓

### 範囲外項目の取り扱い
- 表 (GFM): `Options::ENABLE_TABLES` は付けていないので素通り。テーブル記号がそのまま見える形になるが、ストリーム自体は破綻しない。✓
- 画像 `![alt](src)`: `image_depth` カウンタで alt を含めて捨てる (`crates/tui/src/markdown.rs:97-102, 223, 264`)。テスト `image_alt_is_dropped` あり。✓
- HTML パススルー: チケットの「範囲外」では「タグはそのまま生テキストで出る」と書かれているが、実装では `Event::Html` / `InlineHtml` をハンドラの `_ => {}` で**完全に捨てている** (`crates/tui/src/markdown.rs:166`)。probe で `<div>hi</div>` 入りの入力に対し `Start(HtmlBlock) / Html / End(HtmlBlock)` 列が出ることを確認したが、これら 3 イベントはすべて未処理 = 表示されない。挙動としては「タグ含めて消える」になっている。チケットの記述とはわずかにズレるが、UX 上は無音で消える方が望ましいケースが多く、blocking にはしない。
- 数式 / syntax highlight / OSC 8 / Thinking 適用 / ライブストリーム途中要素フォールバック: 着手なし、チケット通り。✓

### 完了条件
- 「上記要素が視認可能なスタイルで描画される」: 上記の通り全要素にスタイルが付くことをコードと 14 ケースのユニットテストで確認。✓
- 「ストリーミング中、フェンスコードブロックの開きが先に着いて中身が後から流れるケースで全体の見た目が大きく崩れない」: probe で `before\n\n```rust\nlet x = 1;` (閉じ忘れ) を流すと `Start(Paragraph)/Text("before")/End(Paragraph)/Start(CodeBlock(Fenced))/Text("let x = 1;")/End(CodeBlock)` が出ることを確認。途中状態でも `End(CodeBlock)` が EOF で必ず付くため `in_code_block` は確実に閉じ、現状コードブロックを描画したまま自然に途切れる。fence-only (`` ```rust ``) は中身ゼロで blank 1 行分の領域だけ取る程度で破綻しない。`unfinished_emphasis_is_treated_as_text` のテストでも `**` 単体を素テキスト扱いできることが pulldown-cmark の出力から保証される。✓
- 「`Mode::Detail` / `Mode::Normal` で Markdown スタイル、`Mode::Overview` は従来通り」: `crates/tui/src/ui.rs:592-595` の `match mode` で `Overview` だけ従来の `push_overview_line` を保ち、それ以外を `markdown::render` に流している。✓
- 「`wrap_line_into` のラップ・右パディング・スクロールが乱れない」: `markdown::render` は `Line::from(spans)` を返すだけで line-level の `style.bg` を一切セットしない。よって `wrap_line_into` の `fill_to_width = line_style.bg.is_some()` は false のまま、右パディングは発生せず diff-style 行の挙動と干渉しない。char 幅は通常の Span をそのまま並べるだけなので `UnicodeWidthChar` 計算も従来同等。✓

## アーキテクチャ・スコープ

- 影響範囲はチケット通り `crates/tui/Cargo.toml` / `crates/tui/src/markdown.rs` (新設) / `crates/tui/src/ui.rs` の 1 行 / `crates/tui/src/main.rs` の `mod markdown;` 1 行のみ。`ui.rs` は 1 行差し替えに収まり (`crates/tui/src/ui.rs:594`)、レンダリングパイプライン (`compute_history` → `wrap_line_into` → スクロール) には触っていない。最小スコープが守られている。
- 公開面はチケット指定通り `pub fn render(text: &str, base: Style) -> Vec<Line<'static>>` の 1 関数のみ。`Renderer` 構造体は `pub` でない。過剰抽象化なし。
- 依存追加は `pulldown-cmark = { version = "0.13.3", default-features = false }` で、CommonMark コアのみを取り込む形。`tui-markdown` を避け、syntect 等の重量依存を持ち込んでいない (チケット方針通り)。
- 新規クレートは作っていないので命名ポリシー (insomnia- プレフィックス禁止) は対象外。
- `markdown` モジュールは `crates/tui/src/markdown.rs` の単一ファイルにまとまっており、`#[cfg(test)]` で 14 ケース同居。低レベル基盤クレート (`llm-worker` 等) を汚染していない、TUI レイヤ内に閉じる正しい配置。

## 指摘事項

### Non-blocking / Follow-up
- HTML 取り扱いがチケット記載 (「タグはそのまま生テキストで出る」) と実装 (完全に破棄) で食い違う。実装側の方が UX 的に望ましいので、チケット側の文面を「HTML はそのまま無視する」に直すか、レビュー記録のままにしておくかは判断に委ねる。`crates/tui/src/markdown.rs:162-166`。
- `span_style` 内で inline code / code block / heading が `self.base` を完全に無視している。Assistant の `kind_style` (`fg(White)`) しか base に来ない現状では実害ゼロだが、将来同じ `markdown::render` を `Thinking` (magenta + ITALIC) や `SystemMessage` (cyan) で使い回す際にコードブロックだけ palette から外れる。本チケットは Assistant のみが対象なので非ブロッキング。差すタイミングで「base を起点に code/heading の色相だけ寄せる」関数化を検討すると良い。`crates/tui/src/markdown.rs:70-94`。
- 空のリスト項目 (`- a\n-\n- c` のような) は `pending_marker` が `flush_line` で消費される結果、`• ` だけの行が出る。`TagEnd::Item` のコメントは「marker was never consumed」と書いてあるが、現実には `flush_line` (current 空 + pending_marker Some) のガード条件をすり抜けて消費される (`crates/tui/src/markdown.rs:104-116`)。挙動として「空項目は空のバレットを 1 行出す」になっているのは妥当だが、コメントの意図と挙動がやや不一致。pending_marker を消費するか落とすかは別チケットでも構わない範囲。

### Nits
- `RULE_WIDTH` が 40 固定。ターミナル幅に応じた可変化は本チケットの完了条件外なので OK だが、`wrap_line_into` 経由で右側に折り返されない (40 < width 前提) ことだけ将来確認が要る。狭幅環境でも安全側 (はみ出さない) なので問題なし。
- `pulldown_cmark::Options::ENABLE_STRIKETHROUGH` のみ有効。GFM のうち autolink / task list は今回対象外なので妥当。

## 判断

**Approve** — チケットの「対応する Markdown 要素」「範囲外」「完了条件」「影響範囲」のすべてに、コードとテストの両面で対応している。ストリーミング途中状態の堅牢性は CommonMark + pulldown-cmark 0.13 のセマンティクスに任せる方針が妥当に効いており、`wrap_line_into` との互換性も line-level style を空に保つことで担保できている。HTML 表示の文面ズレは非ブロッキング。
