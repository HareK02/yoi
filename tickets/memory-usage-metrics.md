# メモリ機構: 明示使用回数ログ

## 背景

memory / Knowledge の改善で必要なのは、「どの record が実際に context に読まれたか」を後から確認できる観測ログである。

検索結果に出たことや session あたりの浮上率は、session の目的や query の広さに強く依存し、record が読まれたことも意味しない。`n/Mtoken` のような線形スコアも、Knowledge の価値や使用実態を表す軸としては不安定である。

本チケットでは、判断ロジックではなく **明示使用回数の計測** を実装する。Knowledge 化、整理時の保護、`model_invokation` ON/OFF の判断は、このログを材料に consolidation / Doctor / prompt-eval 側で行う。

## 方針

- workspace 側に append-only な usage event log を残す
- staging や session-local state と独立させ、session データ喪失で統計が消えないようにする
- 主指標は record が明示的に context に読まれた回数に限定する
- 検索 hit / query result surface は主指標にしない。初期実装では記録しなくてよい
- session 浮上率や `n/Mtoken` は採用しない
- `model_invokation` 常駐注入は「使用回数」には含めず、resident exposure cost として別口で記録する

## 使用回数として数える event

### `use`

record が明示的に context に取り込まれたことを表す。

対象:

- `MemoryRead`
- `#<slug>` 完全一致参照
- `/workflow` で workflow 自体が呼び出された場合の workflow record
- 将来追加される明示的な Knowledge / Workflow read 経路

補足:

- `MemoryQuery` / `KnowledgeQuery` の検索結果に出ただけでは count しない
- LLM が検索後に自分で `MemoryRead` した場合は、その read を count する
- 同一 session 内での連続参照 coalescing は初期実装では必須にしない。必要になった時に report 側で補助的に扱う

### `resident_exposure`

`model_invokation: true` 等により常駐注入されたことを表す。これは使用回数ではなく cost 観測である。

対象:

- resident knowledge injection

用途:

- record token size / injection count / 推定 total exposure cost の把握
- 明示使用が少ない高コスト常駐 record の発見
- usefulness の分子には含めない

## event schema

具体形式は実装で決定するが、最低限以下を保持する。

```text
id: uuid
occurred_at
session_id
event: use | resident_exposure
source: MemoryRead | KnowledgeRef | WorkflowInvoke | ResidentInjection
records[]:
  kind
  slug
  file_bytes
  file_tokens_estimate
```

`input_tokens` や prompt occupancy は、取得できるなら補助メタデータとして残してよい。ただし集計・判定の主軸にしない。

## report

consolidation / Doctor / prompt-eval から読める集計 API を提供する。

record ごとに最低限以下を出す。

```text
kind
slug
use_count
last_used_at
source_breakdown
resident_exposure_count
estimated_tokens_per_injection
estimated_total_resident_exposure_tokens
```

この report は hard decision ではなく、判断材料である。

- Knowledge 化候補: `decision` / `request` のうち、明示使用回数が多いものを確認対象にする
- tidy protection: 明示使用された record は削除 / 大幅圧縮時に追加確認する
- resident 見直し: resident exposure cost が高く、明示使用が少ない record を確認対象にする

## 現 worktree 実装の扱い

`.worktree/memory-usage-metrics` の実装から残してよいもの:

- workspace-local append-only JSONL event log
- `MemoryRead` hook
- `#<slug>` / `/workflow` 経路の hook
- resident injection を明示使用と別イベントにする配線
- record snapshot に file bytes / token estimate を持つこと
- report API の土台

作り替えるもの:

- `ExplicitInvoke` という単一分類
- `MemoryQuery` / `KnowledgeQuery` hit を主指標として記録する設計
- `frequency_per_mtoken`
- cumulative input token を前提にした window 集計
- session 浮上率や複数 session 要件を hard metric にする設計
- `usage_candidate_frequency_threshold` / `usage_protection_frequency_threshold` 系の設定
- frequency threshold による Knowledge 化候補 / protection 判定

## 範囲外

- Doctor / prompt-eval の実装本体（`tickets/prompt-eval-metrics.md`）
- Knowledge の自動作成
- `model_invokation` ON/OFF の完全自動切替
- query hit の計測
- session 浮上率の算出
- record が model output に実際に使われたかの自動判定

## 完了条件

- 明示使用 event が workspace 側に append-only に積まれる
- `MemoryRead` / `#<slug>` / `/workflow` 経路が使用回数として count される
- `MemoryQuery` / `KnowledgeQuery` の検索 hit が使用回数に含まれていない
- resident exposure が使用回数とは別口で記録される
- record ごとの `use_count` / `last_used_at` / `source_breakdown` / resident cost を集計できる
- consolidation / Doctor 側から report API を呼べる
- `n/Mtoken` や session 浮上率を Knowledge 化候補や tidy protection の主判断として使っていない

## 参照

- `docs/plan/memory.md` §使用頻度メトリクス / §判断ルール / §retrieval 経路
- `tickets/memory-search-tools.md`（hook 挿入点）
- `tickets/memory-consolidation.md`（統合 / 整理 両 step の消費者）
- `tickets/prompt-eval-metrics.md`（Doctor / prompt-eval 側の事後評価）
