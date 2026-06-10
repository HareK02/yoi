---
title: 'Edit/Writeの同一ファイル変更をToolExecutionContextで直列化する'
state: 'inprogress'
created_at: '2026-06-10T07:49:10Z'
updated_at: '2026-06-10T09:29:06Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-10T08:10:58Z'
---

## 背景

LLM-Worker は approved tool calls を並列実行する。これは維持したい一方で、同じ assistant response 内で同一ファイルに対する複数の `Edit` / `Write` / それに準ずる mutation tool が発行された場合、実行順序が nondeterministic になると、LLM が意図した順序と異なる変更・失敗・部分適用が起き得る。

`ToolExecutionContext` は既に導入済みで、各 tool call に `call_id` / `batch_id` / `call_index` が渡る。これにより、tool 実装側で response-local order と batch identity を参照できるようになった。この Ticket はその基盤を使い、Worker 自体を scheduler にせず、ファイル mutation tool 側で同一ファイル変更の直列化を行う。

## ゴール

`Edit` / `Write` など同一ファイルを変更する built-in tool について、同じ target file への mutation が競合しないように tool 側で直列化し、同一 batch 内では LLM が返した `call_index` 順に処理されるようにする。

## 要件

- Worker の approved tool call 並列実行は維持する。
- Worker に resource scheduling policy / per-file scheduler を持たせない。
- `Edit` / `Write` など file mutation tool 側で、canonical target file path 単位の mutation guard / queue / mutex を持つ。
- 同一 `batch_id` かつ同一 target file の複数 mutation は、`ToolExecutionContext.call_index` の昇順で実行される。
- 異なる file への mutation は不要に直列化しない。
- 異なる `batch_id` の同一 file mutation については、破綻しない排他を維持する。
  - 厳密な global ordering が必要か、単純な per-file mutex で十分かを実装時に判断し、理由を記録する。
- `Edit` と `Write` が同じ file を対象にする場合も、同じ per-file mutation boundary に乗せる。
- path normalization / canonicalization を慎重に扱う。
  - 同じ実ファイルを指す相対/絶対 path、`..`、symlink の扱いを明示する。
  - 既存 scope/permission/path validation と矛盾させない。
- queue 待ち・timeout・panic/drop 時に lock が残り続けないようにする。
- 失敗時は対象 tool call だけが明確な error result を返し、他 file の mutation や Worker 全体を不要に止めない。
- `call_id` / `batch_id` / `call_index` は diagnostics に有用な範囲で使うが、secret/private path 露出や過剰 log を避ける。

## 非目標

- Worker に一般 resource scheduler を実装すること。
- `parallel_tool_calls=false` を導入すること。
- Hook / Interceptor に lock lifecycle を持たせること。
- すべての tool を直列化すること。
- LLM response 全体を sequential execution に戻すこと。
- 複数 process / 複数 Pod 間の完全な distributed file lock をこの Ticket で解くこと。

## 受け入れ条件

- 同一 response / 同一 `batch_id` 内で同じ file に対する複数 `Edit` が発行された場合、`call_index` 昇順に実行される。
- 同一 response / 同一 `batch_id` 内で同じ file に対する `Write` と `Edit` が混在しても、同じ file mutation boundary で直列化される。
- 異なる file に対する `Edit` / `Write` は並列実行可能なままである。
- Worker の tool execution path は引き続き approved calls を並列に起動し、Worker が file scheduling policy を所有しない。
- path equivalence の扱いがテストまたは実装コメントで固定されている。
- mutation 中に tool が error になっても guard が解放され、後続 mutation が永久待ちしない。
- focused tests が追加されている。
  - same-batch same-file multiple `Edit` order;
  - same-batch same-file `Write` + `Edit` order;
  - different-file mutations remain concurrent;
  - failed mutation releases the per-file guard;
  - path normalization/equivalence case。
- `cargo test` の targeted command、`cargo fmt --check`、`git diff --check`、`target/debug/yoi ticket doctor` が通る。

## 実装メモ

- `ToolExecutionContext` 導入 Ticket `00001KTNVGT8G` では、この same-file mutation queue / Edit-Write serialization は明示的に非目標だった。本 Ticket はその follow-up。
- Analytics Ticket `00001KTNS9B50` は same-file multiple Edit の検出であり、実行制御ではない。
