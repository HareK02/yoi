---
title: 'Tool実行にToolExecutionContextを渡す'
state: 'inprogress'
created_at: '2026-06-09T09:30:50Z'
updated_at: '2026-06-09T10:43:02Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T10:01:00Z'
---

## 背景

LLM-Worker は tool calls を並列実行する前提で作られている。Worker 側に tool/resource scheduling policy を持たせるのではなく、順序性や排他が必要な tool は tool 実装側で待つ設計にしたい。

そのためには tool 実装が、現在の tool call がどの LLM response / tool batch に属し、response 内で何番目の call なのかを知る必要がある。

現状の `Tool::execute(input_json)` には以下の情報が渡らない:

- tool call id;
- response/tool-batch identity;
- response 内の tool call order;
- LLM call / turn に紐づく実行文脈。

Hook / Interceptor ではこの用途をきれいに満たせない。`pre_tool_call` で lock を取ると、全 pre hooks が tool execution 前に走る構造のため deadlock し得る。また guard を tool execution future の lifetime に渡す API もない。

## ゴール

Tool 実行 API に `ToolExecutionContext` を導入し、tool 実装が response-local order / call identity を参照できるようにする。Worker は並列実行を維持し、scheduling policy は持たない。

## 要件

### API 変更

- 既存 API 互換を優先せず、`Tool::execute` を context 付き signature に置き換える。
- 例:

```rust
async fn execute(&self, input_json: &str, ctx: ToolExecutionContext) -> Result<ToolOutput, ToolError>;
```

- `ToolServer::call_tool` / Worker の tool execution path も context を渡す形に更新する。
- 互換 wrapper や default old-API bridge は原則作らなくてよい。既存 tools を一括更新する。

### ToolExecutionContext

- `ToolExecutionContext` に少なくとも以下を含める:
  - `call_id`: provider/tool call id;
  - `batch_id`: 1つの assistant response / tool-call batch を識別する id;
  - `call_index`: batch 内の tool call order;
  - 必要なら `llm_call_index` / `turn_index` 相当。ただし first pass では過剰に増やさない。
- `batch_id` は tool execution batch ごとに stable であればよい。
  - random UUID でも Worker-local increment でもよい。
  - session log に出す必要があるかは実装時に判断する。
- `call_index` は LLM が出した tool call order を反映する。
- Context は Worker の scheduling 判断に使わない。tool が必要に応じて利用する metadata とする。

### Worker behavior

- Worker はこれまで通り approved tool calls を並列実行する。
- Worker は per-tool parallel-safe / serial / resource conflict policy を持たない。
- Worker は各 tool call に context を付与して並列実行するだけ。
- `pre_tool_call` / `post_tool_call` interceptors の既存意味論は極力維持する。
  - 必要なら `ToolCallInfo` / `ToolResultInfo` に context 相当の情報を追加する。
  - ただし lock/permit lifecycle を interceptor に持たせない。

### Tool update

- 既存 built-in tools を新 signature に更新する。
- 初期段階では多くの tools は context を無視してよい。
- Context を使う具体的な same-file mutation queue / Edit-Write serialization は別 Ticket で扱う。

## 非目標

- Worker に resource scheduling policy を実装すること。
- `Edit` / `Write` の same-file mutex をこの Ticket で実装すること。
- `parallel_tool_calls=false` の導入。
- Hook / Interceptor で lock lifecycle を管理すること。
- Backward-compatible old Tool API を長期維持すること。

## 受け入れ条件

- `Tool::execute` が `ToolExecutionContext` を受け取る。
- Worker が 1 assistant response / tool batch ごとに `batch_id` と `call_index` を付与して tools を並列実行する。
- `call_index` は response 内の tool call order と一致する。
- 既存 built-in tools が新 API に移行され、context を無視しても動作する。
- Interceptor / hook の意味論が壊れていない。
- Tests cover:
  - multiple tool calls in one response receive same `batch_id` and increasing `call_index`;
  - separate tool batches receive distinct `batch_id`;
  - synthetic / skipped / aborted tool calls の context handling が破綻しない;
  - parallel execution is still used for approved calls.
- Focused tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
