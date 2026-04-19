# Review: anthropic-prompt-cache

実装は未コミット（staged + unstaged）。`cargo build` clean、`cargo test --workspace` 467 / 0 fail。

## 総評

チケット要件を忠実に反映。3 層 breakpoint（Prune 位置 / 最後のターン末 / 新規 Head）を `BTreeSet` で管理し重複除去、Anthropic scheme のみに局所化、OpenAI / Gemini は透過的に無視。エッジケース（empty / out-of-range / overlap / shorthand 保持）まで網羅。軽微な指摘のみで blocker なし。

## 完了条件の対応

| 要件 | 状態 | 根拠 |
|---|---|---|
| Prune 位置に `cache_control` | ✅ | `compute_breakpoints` anchor 分岐、`three_breakpoints_when_compact_plus_prior_turn` |
| 最後のターン末に `cache_control` | ✅ | `last_user - 1` 計算、同テスト |
| 新規 Head に `cache_control` | ✅ | `items.len() - 1`、`two_breakpoints_without_compaction` |
| 3 箇所重複の除去 | ✅ | `BTreeSet` で自動重複削除、`overlap_collapses_*` / `single_breakpoint_*` |
| 実セッションで cache_read 非 0 | ⚠️ | 結合で要確認（自動テスト範囲外、実運用で観測） |
| 既存テスト全 pass | ✅ | workspace 467 / 0 fail |
| 新規ユニットテスト | ✅ | 10 件（シリアライズ形状検証含む） |

## 良い点

1. **層構造が素直に分離**: `Request::cache_anchor: Option<usize>` を追加、`Worker::set_cache_anchor` で透過的に伝播、Anthropic scheme のみが実際の breakpoint 配置を知る。OpenAI / Gemini は透過的に無視して回帰なし。

2. **`Pod::compact` / `Pod::from_state` の両方で anchor を復旧**: `from_state` は `history[0]` が `Role::System` かどうかで compact の痕跡を検出して anchor を張る。resume 時のキャッシュ継続性が確保されている。

3. **`force_parts` で breakpoint 位置だけ array 形式に落とす**: 通常は text shorthand を保ちつつ、breakpoint を付ける位置だけ array 化。リクエストサイズを最小化する配慮（`single_text_message_uses_text_shorthand_without_breakpoint` で検証）。

4. **シリアライズ形状の検証**: `serialized_json_shape_matches_anthropic_spec` で `{"type":"ephemeral"}` が正しい階層に出ることを明示的にテスト。Anthropic spec からずれたら即発見できる。

5. **エッジケース網羅**: 空 items / out-of-range anchor / 1 item のみ / tool_result が head になるケース / Parts 強制化 / tool 定義に cache_control が漏れないこと、が全部テスト。

6. **冪等な anchor 範囲チェック**: `request.cache_anchor = self.cache_anchor.filter(|&anchor| anchor < context.len())`。prune が先頭をトリムしても安全。

## 指摘と判断

### 軽微

#### 1. `anchor=0` に `compact` が暗黙依存

```rust
// pod.rs :872
worker.set_history(new_history);
worker.set_cache_anchor(Some(0));
```

`compact` が history[0] に summary を置く前提で `Some(0)` をハードコード。将来 compact のレイアウトを変えたら breaker になる。コメントで invariant を明記してあるので read 可能だが、`compact_state` 側に「summary の位置」を返す API を置いて受け渡す方が健全。

**判断**: 対応は任意。現状は compact の設計上 history[0] が summary であることが不変、from_state 側の検出ロジックも Role::System を見ているので、レイアウトを変えるなら両方を直すことになる。今回は放置で OK。

#### 2. `Request::cache_anchor` が raw フィールド代入

既存の `Request` builder は fluent（`Request::new().user(...).item(...)`）だが、`cache_anchor` のみ `request.cache_anchor = Some(0)` の直代入。テスト内で何度も使われていて少し浮く。`fn cache_anchor(self, anchor: Option<usize>) -> Self` を足して fluent を揃える方が一貫性が良い。

**判断**: スタイルの範囲。機能に影響なし、後で揃えても良い。本チケットでは見送り可。

#### 3. 「直近 user msg の 1 つ前」が tool_call になりうるケース

前ターンが interrupted で最後が `tool_call`（tool_result 未応答）だった場合、turn_end が tool_call を指す。Anthropic 的には tool_use にも cache_control を付けられるので wire 上は OK。意味論的に「ターン末」と呼ぶには微妙だが、キャッシュ位置としての機能は成立（その prefix が安定ならキャッシュが効く）ので実害なし。

**判断**: 仕様上許容。注記不要。

#### 4. Tools array 自体にキャッシュマーカーを付けない選択

ticket で「tools 別 TTL 管理は将来」と明記されており、今は意図的に off。tools は message-level breakpoint の prefix に含まれるので、実質的に system + tools + history のまとまりで cache される。問題なし。

**判断**: ticket 通り。

## 完了に向けた作業

- 必須修正なし
- 指摘 1〜2 は style の範囲、本チケットでは対応不要
- 実セッションで cache_read_tokens が 2 コール目以降に非 0 になるかは実運用で確認（LLM への実アクセスが必要、CI の範囲外）

**完了 OK**。
