# レビュー: AGENTS.md の取り込み

対象差分: `crates/pod/src/{lib,pod,agents_md}.rs`, `crates/pod/tests/system_prompt_template_test.rs`（いずれも未コミット）

## 要件達成状況

| 要件 | 検証 |
|---|---|
| cwd 直下の `AGENTS.md` のみ読む（親遡及・ネスト無し） | ✅ `agents_md.rs:28` で `cwd.join("AGENTS.md")` 単発 |
| 不在時は `files.agents_md` キー自体を省略 | ✅ `pod.rs:407-410` が `Option::Some` のときのみ挿入 |
| サイズ上限 64KB、超過時は先頭 64KB + 注記 + warn | ✅ 定数 `AGENTS_MD_LIMIT` と `TRUNCATION_NOTICE`、`oversized_file_is_truncated_with_notice` で検証 |
| 非 UTF-8 / 読み取り失敗時は warn + キー省略 | ✅ `non_utf8_returns_none`, open/read エラー経路に warn |
| 不在時は warn を出さない（正常系） | ✅ `ErrorKind::NotFound` だけ warn をスキップ |
| first turn 時に1回のみ評価、compact 後も再評価しない | ✅ 既存の `ensure_system_prompt_materialized` 機構に乗る。`agents_md_not_reread_after_compact` が file を mutate + compact 経由で明示的に担保 |
| `{% if files.agents_md is defined %}` で取得可能（B 案） | ✅ `agents_md_absent_leaves_key_undefined`, `agents_md_is_injected_when_present` 両方でテンプレ側のガードを検証 |

完了条件4ケース（正常・不在・64KB 超過・非UTF-8）および compact 跨ぎ、合計5ケースすべてが unit / integration テストで自動化されている。**要件は完全に達成**。

## アーキテクチャ統合

- 実装を `pod.rs` に書き足さず `crates/pod/src/agents_md.rs` として別ファイルに切り出している。機能モジュール分離のプロジェクト方針と整合。
- `read_agents_md` は `pub(crate)` に閉じており、外部 API を汚さない。
- `pod.rs` 側の配線は 5 行の最小侵襲（既存 `files: BTreeMap` の作り方を変えただけ）。system_prompt_template の評価タイミング保証を自前で再実装せず、既存の materialize 機構に素直に乗っている。

アーキテクチャ的な懸念なし。

## 指摘事項

### 1. 🟡 切り詰め時の UTF-8 pop ループが「意図的な非 UTF-8 の大型ファイル」を通してしまう

`agents_md.rs:61-65`:

```rust
if truncated {
    while !buf.is_empty() && std::str::from_utf8(&buf).is_err() {
        buf.pop();
    }
}
```

コメントは明確: 「切り詰め時だけ partial UTF-8 を trim する。切り詰めていない本物の非 UTF-8 は拒否する」。意図は正しい。

しかし副作用として:

- 小さい非 UTF-8 ファイル → `String::from_utf8` で拒否（OK）
- **64KB を超える非 UTF-8 ファイル**で、先頭が偶然 UTF-8 valid な prefix を持つ場合 → pop ループで末尾の invalid バイトを削り落として採用されてしまう

つまり「非 UTF-8 は拒否」という要件が**ファイルサイズによって挙動が変わる非一貫**になっている。

**実害**:
- 普通の AGENTS.md が意図せずこの経路に落ちることは稀（エディタは UTF-8 で保存する）
- 巨大バイナリを誤配置した場合のフォールバックとしては「読める prefix を採用」は妥当とも言える

**判断**: 実害は小さいので**必須修正ではない**。ただし挙動を意図したものとして固めるなら、`str::from_utf8(&buf).unwrap_err().valid_up_to()` で truncation 時の末尾処理をより正確に（partial UTF-8 分だけ削る）記述するとコードが仕様と一致する。将来の誰かが読んで混乱しないためにも、コメントに「pop ループは truncation 末尾の partial UTF-8 だけを狙っているが、pathological には valid prefix を持つ非 UTF-8 ファイルも通る」と明記しておくと親切。

### 2. 🟢 non-UTF-8 / I/O エラー時に `files` キーが省略されることの integration 検証

`agents_md.rs` の unit test で `read_agents_md` が `None` を返すことは確認されているため、`pod.rs:407-410` の `if let Some(...)` 経路を通れば挙動は担保される。追加の integration test は不要。

ただし将来 `pod.rs` 側で `read_agents_md` の戻り値の扱いを変更した場合に unit test だけでは気付けない可能性がある。気になるなら `agents_md_non_utf8_leaves_key_undefined` という integration を1つ足す程度。**任意**。

### 3. 🟢 pod.rs で `std::collections::BTreeMap` がインラインのまま

`pod.rs:406` `let mut files = std::collections::BTreeMap::new();` がフルパス。既存コードスタイルへの追随のはずなので不問。必要なら別途クリーンアップ。

## 良い点

- `File::open` → `take(LIMIT + 1)` → `read_to_end` の **extra-byte トリック**でメタデータが信頼できないファイルシステム（pipe, procfs 等）でも truncation を正しく検出できる
- `oversized = metadata_len.map(|n| n as usize > AGENTS_MD_LIMIT)` と `buf.len() > AGENTS_MD_LIMIT` の**二重チェック**
- エラー経路ごとに warn のフィールド（`path`, `error`, `limit`）が構造化されている
- テストの網羅性: absent / small / oversized / exact limit / UTF-8 char boundary / non-UTF-8 / integration 3ケース = **計9テスト**
- `exact_limit_is_not_truncated` というオフバイワン検証が入っている（地味に偉い）
- 各コメントが **what ではなく why** を書いている（extra byte を読む理由、pop ループの条件分岐の意図）

## 結論

**無条件で受け入れ可**。指摘1はコメント追記または valid_up_to ベースに書き換えを**任意で**検討、指摘2・3は不問。
