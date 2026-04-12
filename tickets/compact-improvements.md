# Compact の改善

## 背景

`Pod::compact()` とその周辺機構は実装済み。
要約品質、保護単位、compact 後のコンテキスト構築に改善が必要。

## 前提チケット

- [token-counter.md](token-counter.md) — LlmClient に Tokenizer 導入。retained_tokens / auto-read budget がこれに依存
- [tool-output-referenced-files.md](tool-output-referenced-files.md) — ToolOutput にファイル追跡フィールド追加。デフォルトリファレンスがこれに依存

---

## 要件

### R1: 一貫した振る舞い
- システムプロンプトは不変
- compact 前後でユーザーが違和感を覚えない
- 「何を知っていて何を忘れたか」が自然であること

### R2: 直近の記憶の確実性
- 直近 N トークン分の会話をそのまま保持（Prune 済み = summary only の状態で計測）
- **トークン数ベース** で保護量を決める（ターン単位ではない）
  - 自走エージェントは1ターン内で多数のリクエストを回す
  - ターン単位だと保護量がターン長に依存してしまう

### R3: Auto-Read + リファレンス
- compact 後の最初のターンで、タスク遂行に必要なファイルが既に読まれている
- 2段階: **Read**（全文/範囲をコンテキストに注入）と **Reference**（「読んだことがある」とだけ伝える）
- compact worker が「続行に必要なファイル」を判断して指定する

### R4: マルチタスク対応
- セッション中に一貫した課題に取り組んでいないものとする
- **完了タスク**: 簡潔に。注意点・発覚した事実だけ
- **進行中タスク**: サマリ + 現状 + 次のステップを十分に

---

## 閾値の修正（重要）

現状の実装は閾値の大小関係が意図と逆になっている。修正する。

### 正しい方針

- **post-run (タスク区切り) = 早めの閾値**: タスクの区切りで先を見越して compact
- **mid-turn (pre_llm_request) = 遅めの閾値**: ターン中は最終防衛ラインとして、遅くなっても止まらないよう

```
manifest.compact_threshold  →  post_run_threshold  (基本ライン, 早め)
                               turn_threshold = post_run_threshold * 9 / 8  (safety net, 遅め)
```

### 影響箇所

- `crates/pod/src/compact_state.rs`
  - フィールド名と初期化を入れ替え: `manifest compact_threshold` は `post_run_threshold` に代入
  - `turn_threshold` は `post_run_threshold * 9 / 8` として導出
  - テストの `assert_eq!(state.post_run_threshold, 90_000)` を逆転（`turn_threshold = 90_000`, `post_run_threshold = 80_000` が正）
- `crates/pod/src/compact_interceptor.rs` — そのまま（`exceeds_turn` を呼ぶだけ）
- `crates/pod/src/pod.rs:371` の `exceeds_post_run` 判定 — そのまま
- `docs/compaction.md` — 「ターン間は早めの閾値」の記述を逆に修正

---

## compact 後の history 構造

全て system message（`Item::Message { role: System }`)として注入。

```
[system prompt]                              ← 不変 (R1)
[system: 構造化要約]                          ← R4: compact worker の出力
[system: auto-read ファイル群]                ← R3: read_required の結果
[system: リファレンス一覧]                     ← R3: reference の結果
[直近 N トークン分の生の会話]                  ← R2: pruned 状態で保持
```

system message で統一する理由:
- LLM に「システムから提供された前提情報」として認識させる
- fake ユーザーメッセージや fake ToolCall を作らない
- 要約もファイルも同じ role で自然に並ぶ

### auto-read の system message 例

```
[Auto-read file: src/main.rs:42-142]
fn main() {
    let config = Config::load();
    ...
}
```

### リファレンスの system message 例

```
[Referenced files — read before compaction, contents not included]
- src/config.rs (read during task setup)
- tests/integration_test.rs (read during test implementation)
Use read_file to access current contents if needed.
```

---

## R2: トークンベースの保護

現状の `retained_turns` を `retained_tokens` に変更。

```
history (全て pruned 済み = summary only):
  [...古い部分...] [...直近 N トークン分...]
       ↓                    ↓
    要約対象            そのまま新 history に載せる
```

- Prune 済みの history に対して `LlmClient::tokenizer()` でトークン数を推定
- 末尾から逆順に数えて N トークン分の位置で切る
- ターン境界は無視。アイテム単位で切る

```toml
[compaction]
compact_threshold = 80000
retained_tokens = 8000     # ← retained_turns から変更
```

token-counter チケットが前提。

---

## R3: Auto-Read + リファレンス

### デフォルトリファレンスの抽出

Pod は `ToolOutput.referenced_files` を `HookInterceptor::post_tool_call` で観察し、
LRU 的な履歴バッファに積む（→ tool-output-referenced-files チケット）。
Compact 時は先頭 5 件を compact worker のデフォルトリファレンスとして渡す。

### compact worker のツール

```
read_file(path, offset?, limit?)          — ファイルを読んで判断するため
mark_read_required(path, offset?, limit?) — auto-read 対象（内容をコンテキストに載せる）
add_reference(path)                       — リファレンス追加（内容は載せない）
write_summary(text)                       — 構造化要約を出力/上書き（上書き可）
```

`write_summary` は**上書き可**。マルチターンで「下書き → 追加 read → 書き直し」の順序が自然に動く。
最終的に直近の呼び出しが採用される。ガードは「一度も呼ばれていない」時のみ。

### フロー

1. Pod が referenced_files バッファから先頭 5 件を抽出（デフォルトリファレンス）
2. compact worker のプロンプトに含める:

   ```
   以下のファイルがリファレンスとして指定されています。
   全て読んで、タスク続行に必要なものを mark_read_required で指定してください。
   リファレンスを追加したい場合は add_reference で追加できます。
   ```

3. compact worker が read_file で全ファイルを読み、判断:
   - 必要なファイル → `mark_read_required(path, offset?, limit?)`
   - 不要だがコンテキストとして有用 → リファレンスのまま残す
   - 追加のリファレンス → `add_reference(path)`
4. `write_summary` で構造化要約を出力（最後のが採用される）
5. ターン終了時に summary が一度も書かれていない or read_required が空（かつファイル操作履歴がある場合）→ 追加プロンプトで促す

### Auto-Read の Budget 管理

compact worker が `mark_read_required` を無制限に呼ぶとコンテキストが膨張する。
共有 budget で制御:

```toml
[compaction]
auto_read_budget = 8000   # 合計トークン上限
```

- `mark_read_required` のツール結果で残量を返す:
  `"Marked. Budget: 4200/8000 tokens remaining"`
- 50% 以下になったら次のツール結果に system reminder を append:
  `"Budget half consumed. Consider calling write_summary soon."`
- 100% 超過で Err:
  `"Error: auto-read budget exhausted (8000 tokens). Remove an existing mark or use add_reference instead."`
- compact worker が判断して自分で調整できる（Err は即中断ではない）

token-counter チケットが前提（budget の計測に `estimate_text` が要る）。

### compact worker の暴走抑止

Turn/request 数ではなく、compact worker の累計入力トークンで上限を設ける:

```toml
[compaction]
compact_worker_max_input_tokens = 50000
```

超えたら compact worker を強制終了。`CompactState::record_compact_failure()` 経由で
サーキットブレーカーの自然な経路に乗る。

---

## R4: 要約の内容と品質

### 出力方法

compact worker が `write_summary(text)` ツールで出力する（上書き可）。
最後のテキスト出力ではなくツールにする理由:
- マルチターンで read_file → 判断 → 要約の順序が自由
- 要約を書いた後にさらにリファレンスを追加できる
- 「要約を書いていない」のガードが mark_read_required と同じパターンで検出可能

### 含めるべき内容

コードスニペットは auto-read に任せる。要約に求めるのは:
1. **何を、なぜやったか** — 意思決定の記録。具体的な型名・関数名で言及
2. **ユーザーの指示・フィードバックの原文** — ニュアンス保持。重要なもののみ
3. **発生した問題と解決策** — 同じ轍を踏まない
4. **今どこにいて次に何をするか** — compact 前後の一貫性 (R1)

含めないもの:
- コードの全文（auto-read が担う）
- 変更の diff（git がある）
- 中間のやりとりの詳細（最終結論だけ）

### フォーマット（5セクション、1000-2000 トークン目安）

```
## Completed Tasks
### (タスク名)
- 完了した作業（具体的な型名・ファイル名で）
- 注意点 / 発覚した事実

### (タスク名)
- ...

## Active Task
### (タスク名)
- 目標
- 現状（何が済んで何が未着手か）
- 次のステップ

## Key Decisions
- (判断内容) — (理由)
- ...

## User Directives
- 「（ユーザー発言の原文）」 — 重要な指示・フィードバックのみ
- ...

## Current Work
（直前に何をしていたか。2-3行）
```

各セクションの目安量:
- Completed Tasks: 各タスク 2-3 行 × タスク数
- Active Task: 5-10 行
- Key Decisions: 各 1-2 行
- User Directives: 重要な発言のみ原文引用
- Current Work: 2-3 行

### 要約の入力

pruned history から:
- ToolResult は summary のみ（content 除去）
- ToolCall は名前のみ（arguments 除去）
- Reasoning は除去

---

## 挙動の未決定事項

### Yield のタイミング精度

現状 `pre_llm_request`（リクエストの切れ目）でのみチェック。
1ターン内でツール呼び出しが多く途中でコンテキストが膨らむケースは次のリクエストまで待つ。

検討: `post_tool_call` でもチェックする？

### 閾値の比率

- `post_run_threshold` = マニフェストの `compact_threshold`
- `turn_threshold` = `post_run_threshold * 9 / 8`（≈ 112.5%）

9/8 の根拠はない（安全マージン）。要調整。

### Prune と Compact の相互作用

Prune はリクエストコンテキストのみ操作、`last_input_tokens` は前回の LLM レスポンスの値。
Prune の効果は閾値判断に反映されない。保守的（compact しすぎる方向）で実害は小さい。

### compact 中のクライアント通知

Protocol チケットの `CompactStart`/`CompactDone` で対応。

### 復元時の挙動

`Outcome::Yielded` で記録されたセッションは `last_run_interrupted = true` で復元。
compact 後の新セッションが存在する場合、どちらを restore するかは呼び出し側の責任。
`compacted_from` で辿れる。

---

## 実装順序

0. **[前提] token-counter** — LlmClient に Tokenizer
0. **[前提] tool-output-referenced-files** — ToolOutput + Pod の LRU バッファ
1. **閾値逆転の修正** — `compact_state.rs` のフィールド入れ替え、テスト修正、docs 更新
2. **要約入力の削減** — `build_summary_prompt` から content/arguments/reasoning を除去
3. **retained_tokens 化** — retained_turns → retained_tokens に変更。マニフェスト設定追加
4. **compact worker のツール化** — read_file + mark_read_required + add_reference + write_summary (上書き可)
5. **Auto-Read + リファレンス** — デフォルト5ファイル抽出 (referenced_files バッファから)、compact worker による選定、system message での注入
6. **Auto-Read Budget** — `mark_read_required` のトークン会計、残量通知、超過エラー
7. **compact worker の累計入力トークン制限** — `compact_worker_max_input_tokens`
8. **要約フォーマット** — タスク分類の要約プロンプト調整
9. **ガード** — write_summary 未呼び出し or mark_read_required 空時の追加プロンプト
