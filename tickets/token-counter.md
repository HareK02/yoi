# トークン会計 (Usage 履歴ベース)

## 背景

Compact / Prune の挙動改善に「**履歴上の任意位置のトークン数**」と
「**ある変更でどれだけトークンが浮くか**」を答えられる仕組みが要る。

ローカル近似（`len/4` や BPE テーブル）はモデル/言語/ツール overhead の
誤差が大きく、Anthropic に至ってはオフラインで正確に数える手段が無い。

一方、LLM レスポンスの `Usage` は **送信した history prefix に対して
プロバイダが実測したトークン数** であり、これが手元にある最も正確な情報源。

リクエスト毎にスナップショットを蓄積すれば、ターンより細かい粒度で
履歴の任意 suffix のトークン数を実測ベースで逆算できる。

## 動機

正確なトークン数（推定でも実測由来）が要る箇所:

- **Compact の retained_tokens 切り出し** — 末尾から N トークン残す cut 位置を決める
- **Prune の `min_savings` 判定** — 「この content を捨てたら何トークン浮くか」を見積もる
- **Compact worker の auto-read budget 判定** — `mark_read_required` の累計
- **UI 向けトークン表示**（将来）

ターン境界での切り出しでは粒度が粗すぎる。長く自走するエージェントは
1ターン内で多数のリクエストを回し、ターン長が大きくバラつくため。

## 前提

- [usage-history.md](usage-history.md) — session-store に `LogEntry::LlmUsage`
  を追加し、Worker からリクエスト送信時の prefix と実測値を組で記録する基盤。
  本チケットはその履歴を消費する側。

## 方針

ローカルなトークナイザは持たない。プロバイダ実測値の履歴 (`UsageRecord` 列)
と現在の history items から、ピュアな計算で答えを返す。

API は **session 概念を持つ型のメソッド** として生やす。両方を所有する
オーナーが呼ぶ形になるので引数はゼロ。具体的な配置先は実装時に確定する
（候補: `RestoredState` / Worker 内の history 所有者 / 薄い view 型）。

```rust
impl Session {
    /// 現在の history 全体の推定トークン数。
    /// 最後の measurement + その後追加された未測定分の按分。
    pub fn total_tokens(&self) -> TokenEstimate;

    /// 末尾から `retained_tokens` 以上を残すための分割位置 (history index)。
    /// `items[..cut.index]` が捨てる/要約される側、`items[cut.index..]` が残る側。
    pub fn split_for_retained(&self, retained_tokens: u64) -> SplitPoint;

    /// 指定範囲の items を drop した場合の推定節約トークン数。
    pub fn savings_for_drop(&self, range: Range<usize>) -> TokenEstimate;
}

pub struct TokenEstimate {
    pub tokens: u64,
    pub source: EstimateSource,
}

pub struct SplitPoint {
    pub index: usize,
    pub source: EstimateSource,
}

/// 推定の出どころ。呼び出し側が「概算である」ことを認識して扱えるよう明示する。
pub enum EstimateSource {
    /// measurement の境界とちょうど一致（実測そのもの）
    Measured,
    /// 連続する 2 measurement の間をバイト按分で計算
    Interpolated,
    /// 最後の measurement より新しい区間 → 最後の rate で外挿
    Extrapolated,
    /// measurement ゼロ件 → バイト数のみのフォールバック
    NoData,
}
```

呼び出し側:
```rust
let cut = session.split_for_retained(8_000);
let saved = session.savings_for_drop(0..cut.index);
if saved.tokens >= min_savings {
    // prune を実行
}
```

## 設計ポイント

- **状態を持たない**: 計算は所有 history と所有 measurements を見るだけの pure 関数。
  trait もインスタンスも tokenizer も要らない
- **概算であることを返り値で明示**: `EstimateSource` で呼び出し側が
  measurement 直上 / 按分 / 外挿 / 履歴無しを区別できる。課金判断には使えない
- **provider 非依存**: tiktoken やプロバイダ別実装は一切不要。
  実測値に provider/model/言語/ツール overhead が全て込み
- **キャッシュヒットの扱い**: `cache_read_input_tokens` は「コンテキスト占有量」
  には含めるが「実コスト」とは区別する。Compact/Prune の判定は占有量基準
  （= raw input_tokens、cache_read 込みの total）

## 計算アルゴリズム

`measurements: &[UsageRecord]` （`(history_len_at_send, input_tokens)` の昇順列）と
`history: &[Item]` から:

### `total_tokens`
1. measurement が無い → `NoData`、history のバイト数で粗い概算
2. 最新 measurement の `tokens` を起点に、`history.len() > last.history_len` の
   差分があれば最終 rate (tokens / total_bytes) で按分して足す → `Extrapolated`
3. ぴったり history が一致 → `Measured`

### `split_for_retained(retained)`
1. measurements を末尾から走査
2. `current_total - measurement.tokens >= retained` を満たす最小の measurement を見つける
3. 一致する measurement あり → `index = measurement.history_len`, `Measured`
4. 2つの measurement の間に境界がある → 区間内のアイテムをバイト按分して `Interpolated`
5. 最後の measurement より新しい区間で境界 → 最終 rate で外挿、`Extrapolated`
6. measurements 不足 → `NoData` でバイト按分フォールバック

### `savings_for_drop(range)`
1. range と measurement の境界の包含関係を見て、区間を完全に含む measurement の
   差分を取る → `Measured` or `Interpolated`
2. range が最後の measurement より後ろを含む → 外挿で `Extrapolated`
3. measurements 無し → バイト数のみ、`NoData`

## 実装対象

- session-store または最終的な配置先クレート:
  - `Session`（または既存の history+measurements 所有者）に
    `total_tokens` / `split_for_retained` / `savings_for_drop` を実装
  - `TokenEstimate` / `SplitPoint` / `EstimateSource` 型
  - 単体テスト: measurement 0/1/N 件、ぴったり境界、按分、外挿、prune 後の整合性
- `crates/llm-worker/src/prune.rs`:
  - `estimate_tokens` を削除し、`min_savings` 判定を `Session::savings_for_drop`
    呼び出しに置き換え（呼び出し側で渡す）
  - prune の API シグネチャ調整は最小限に

## 依存

- [usage-history.md](usage-history.md) — Usage を session-store に積む基盤

## ブロックする後続

- [compact-improvements.md](compact-improvements.md) — retained_tokens 化、
  auto-read budget、prune の min_savings 精度向上が依存
