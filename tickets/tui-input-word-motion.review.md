# Review: TUI 入力欄の単語単位カーソル移動・削除

## 前提・要件の確認

- **`Ctrl+Left` で1単語ぶん後ろに移動**: `crates/tui/src/main.rs:422-425` で `KeyCode::Left if ctrl` を `app.move_cursor_word_left()` に分岐。`crates/tui/src/input.rs:171-182` の `move_word_left` が Sep スキップ → 同一 `AtomClass` の連続をスキップする実装。要件を満たす。
- **`Ctrl+Right` で1単語ぶん前に移動**: `main.rs:430-433` と `input.rs:185-198` の対称実装。要件を満たす。
- **文字種ベースの境界判定**:
  - ASCII 英数字 + `_` → `WordKind::Ascii` (`input.rs:68-69`)
  - ひらがな U+3040..U+309F → `Hiragana` (`input.rs:73`)
  - カタカナ U+30A0..U+30FF, U+31F0..U+31FF → `Katakana` (`input.rs:74`)
  - 漢字 U+3400..U+4DBF, U+4E00..U+9FFF, U+F900..U+FAFF, U+20000..U+2FFFF → `Han` (`input.rs:75-77`)
  - その他 `is_alphanumeric()` true → `Other` (`input.rs:78`)
  - 残りは `Sep` (`input.rs:79`)
  チケットの範囲表とビット単位で一致。
- **同種連続=1単語、種別切替で境界**: `move_word_left/right` の二段ループで実装 (Sep を食い、その先の Word/Paste 1ブロック分を `kind` で揃えて食う)。`script_switch_is_a_word_boundary` テスト (`input.rs:702-720`) が直接検証。
- **Paste は不可分**: `Atom::Paste` を `AtomClass::Paste` として独立したクラスにしているため、隣接 Atom と同種にならず常に1ブロック扱い。`paste_counts_as_one_word` (`input.rs:655-681`) と `delete_word_treats_paste_as_one_unit` (`input.rs:794-812`) で検証。
- **既存 `Ctrl+Home/End`, `Ctrl+[`, `Ctrl+]` と非衝突**: それらは `main.rs:354-392` の先頭ブロック (履歴スクロール) で先に処理される。新規 `Ctrl+Left/Right`/`Ctrl+Backspace` は別の `match` ブロックで処理され、コードフロー上衝突しない。
- **既存 `Left/Right`/`Backspace` の1文字挙動を維持**: `main.rs:426-429`, `434-437`, `414-417` で従来分岐を残している。
- **`Ctrl+Backspace` で1単語ぶん手前を削除、境界判定は `Ctrl+Left` と同じ**: `delete_word_before` (`input.rs:153-158`) は `move_word_left` を呼んで境界を出してから `drain` するだけなので、ロジックが完全に共有される。
- **テスト**: word-motion テスト 17 件すべて pass、`cargo test -p tui` 26 件全 pass。チケットで要求された「空・連続スペース・`\n` をまたぐ・Paste をまたぐ・ひらがな/カタカナ/漢字/ASCII の混在」を網羅。

## アーキテクチャ・スコープ

- 既存の `InputBuffer` のメソッドファミリー (`move_left`, `move_right`, `move_home`, ...) と並べて `move_word_left` / `move_word_right` / `delete_word_before` を追加しただけで、レイヤー越境や新クレートはなし。`app.rs:410-415, 401-403` の薄いフォワーダーも既存の慣習に揃っている。
- `AtomClass` / `WordKind` は `input.rs` 内の private enum で完結しており、外部に余計な API 露出がない。
- `delete_word_before` を `move_word_left` 経由で実装しているのは「境界判定ロジックの一元化」というチケット要求 (`要件: 境界判定は Ctrl+Left と同じ（同じロジックを共有）`) に最もストレートに応える形で、`drain(start..end)` と組み合わせて副作用も自然に正しい状態に落ちる (移動後 cursor がそのまま新しい挿入点)。シンプルで適切。
- スコープ超過なし。チケットで「範囲外」と書かれた `Ctrl+Delete` / `Alt+d` / `Alt+Left/Right` には手を出していない。

## 指摘事項

### Non-blocking / Follow-up

- **半角カナ (U+FF65..U+FF9F) と全角英数字 (U+FF21..U+FF3A 等) の扱い** — 現状 `is_alphanumeric()` の挙動に依存して fall-through する。半角カナ (`ｱｲｳ` 等) は Unicode 的に `is_alphanumeric()` が **false** なので `Sep` 扱いになり、空白同様にスキップされる。チケット本文に明記されていないので blocking ではないが、日本語テキストの実用上は単語境界としてふるまってほしい範囲。次のチケットで「半角カナ・全角英数の `WordKind` 拡張」を別出しするか、チケットの範囲外節に「半角カナは現状 Sep 扱い」と一言追記するか、どちらかを推奨。
- **Han の追加面** — CJK Extension B–F (U+20000..U+2FFFF) は範囲に含まれているが、Extension G/H/I (U+30000..U+3134F あたり) は外れている。実用上ほぼ遭遇しないので follow-up 扱いで十分。
- **チケット完了条件の表現** — 「単語境界の判定にユニットテストが付いている」という一文だけだと `Ctrl+Backspace` 側のテスト要件が暗黙。完了条件を読むだけだと曖昧なので、今後の同種チケットでは削除側のケースも明記すると良い (今回の実装は十分だが、ドキュメント整合性の観点)。

### Nits

- `WordKind::Other` の判定がバイト単位より下流の `is_alphanumeric()` フォールバックなのは妥当だが、コメント (`input.rs:78`) に「アクセント付きラテン、キリル、ハングル等」と挙げると意図が読みやすい (チケット要件文と同じ表現)。

## 判断

**Approve** — チケットの要件・完了条件・範囲外境界すべてに整合した実装で、テストも要求項目を網羅し全体ビルドも維持されている。半角カナ/全角英数は Non-blocking の follow-up 候補として残るのみ。
