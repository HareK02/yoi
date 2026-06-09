---
title: 'session analyticsにresponse単位のbatching指標を追加する'
state: 'queued'
created_at: '2026-06-09T08:51:48Z'
updated_at: '2026-06-09T10:31:14Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T10:31:14Z'
---

## 背景

`session-analytics-tooling` を実際の Coder / Reviewer セッションに適用したところ、tool call の総数だけではなく、assistant response 単位の batching が重要だと分かった。

観測例:

- Coder session では `avg tools / tool response = 1.16`、`p90 = 1` で、ほぼ 1 response = 1 tool call だった。
- `Edit` は 40 responses すべてが `1 response = 1 Edit` だった。
- 同じ response 内で同一ファイルに複数 `Edit` している例はなかった。
- その一方で、同じファイルへの edit-only response が 2〜4 回連続している箇所があり、round-trip latency の観点では batching opportunity の可能性がある。
- Reviewer session では編集はないが、tool calls per response は同様に低く、探索時に Read/Grep/Bash の result size と response batching を見る価値がある。

Edit batching 自体の是非は追加調査するが、analytics 側には response 単位の指標が必要。

## ゴール

`session-analytics` に assistant response 単位の tool batching / edit round-trip 指標を追加し、tool call の使い方を速度面から評価できるようにする。

## 要件

- Session JSONL から assistant response / tool result cycle 単位を推定する。
  - 1 assistant response に含まれる tool call 群を集計する。
  - user turn 単位とは別に response 単位の metrics を持つ。
- Tool calls per response を集計する。
  - total responses
  - tool-call responses
  - total tool calls
  - avg / p50 / p90 / max
  - histogram
  - top responses by tool call count
- Edit batching metrics を追加する。
  - Edit calls per response
  - responses containing Edit
  - same-file multiple Edit calls in one response
  - files touched per edit response
  - large edit arguments との相関が取れる形にする。
- Consecutive edit round-trip metrics を追加する。
  - 同一ファイルに対する連続 edit-only responses を検出する。
  - streak length / file path / response range / line range を報告する。
  - 間に Read/Bash/test/tool result dependent step がある場合と、pure edit-only streak を区別する。
- Suspicious diagnostics は断定しない。
  - 連続 Edit が必ず悪いとは言わない。
  - "possible batching opportunity" として報告する。
  - Edit の成功/失敗や前回 result を見て次を決める必要がある場合は正当な分割であり得る。
- JSON report に machine-readable field として追加する。
- Human summary mode がある場合は、top-N の batching observations を表示する。
  - まだ human summary が無い場合は、少なくとも JSON schema に追加し、後続で CLI summary に出せるようにする。

## 非目標

- この Ticket で prompt / workflow を変更して batching を強制すること。
- 複数 Edit を同一 response に並べる方針を決めること。
- ordered patch / EditBatch tool を実装すること。
- Edit が小さいこと自体を悪いと判定すること。

## 受け入れ条件

- `session-analytics` report に response-level tool batching metrics が含まれる。
- Coder session のような `1 response = 1 Edit` 傾向を数値で確認できる。
- 同一ファイルへの consecutive edit-only response streak を検出できる。
- Same-response same-file multiple Edit がある場合に検出できる。
- Tests cover:
  - single response with multiple tools;
  - single response with multiple same-file edits;
  - consecutive edit-only responses to the same file;
  - interleaved Read/test step that breaks or annotates a streak;
  - sessions with no edits.
- `cargo fmt --check`, focused tests, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
