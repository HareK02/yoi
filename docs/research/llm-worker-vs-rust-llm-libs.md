# `llm-worker` vs Rust LLM ライブラリ群 比較レポート

調査日: 2026-04-26
対象: `crates/llm-worker` (本プロジェクト) / `rig` / `genai` / `swiftide`

調査方法: 各リポジトリを ローカルに取得し、ソース（README, Cargo.toml, src/, examples/）を直接読解。

---

## 0. TL;DR

```
            低レイヤ ←──────────────────────────────────────→ 高レイヤ
            HTTP wrapper       Worker/Loop         Agent Framework        Pipeline/RAG
            ─────────────      ────────────        ────────────────       ──────────────
genai       ●●●●●               (out-of-scope)
llm-worker                       ●●●●●            (一部)
rig                              ●●●               ●●●●●                  ●●●
swiftide                         ●●                ●●●●                   ●●●●● (主軸)
```

- **`genai`** … マルチプロバイダの「統一 chat client」。エージェントループ・状態管理は明示的にスコープ外。
- **`llm-worker`** … Worker = 「LLM 対話の中央実行器」。低レベル（HTTP）と高レベル（Agent / RAG / Pipeline）の中間に位置するレイヤを精緻に作っている。
- **`rig`** … Agent / Tool / Pipeline / VectorStore を一通り揃えた汎用フレームワーク。Provider 数とエコシステムが最大の武器。
- **`swiftide`** … 一次目的はストリーミング Indexing / Query パイプライン。Agent はあとから足された二の矢で、`async-openai` / `async-anthropic` をラップ。

要点: **`llm-worker` の独自性は「型状態でのキャッシュ保護」「Interceptor による上位層への決定委譲」「Prune を context projection として非破壊で扱う」の 3 点**。これらは他 3 者の誰も持っていない。一方、Provider カバレッジ・VectorStore・Pipeline DAG では rig に大差で負ける。

---

## 1. スコープ宣言（各プロジェクトが自分を何と呼んでいるか）

| プロジェクト | 自称 | 提供する | 提供しない（明示・暗黙） |
|---|---|---|---|
| **llm-worker** | "LLM との対話を管理する低レベル基盤クレート" | Worker による turn 実行、Tool 実行、stream イベント、Interceptor、cache 保護 | RAG / Embedding / VectorStore / Pipeline DAG / 永続化 |
| **rig** | "Ergonomic & modular library for building LLM applications" | Agent / Tool / Pipeline / Vector / Embedding / OTel | (ほぼ全部入り。明示的な non-goal は薄い) |
| **genai** | "Standardizing chat completion APIs across major AI services" | Chat (sync/stream)、Tool 定義、Embedding | **Agent loop / 会話状態管理を明示的に out-of-scope** |
| **swiftide** | "Fast, streaming indexing, query, and agentic LLM applications" / "data pipeline" | Indexing pipeline / Query state machine / Agent / Hook | (LLM HTTP は外部 SDK = `async-openai` 等に委譲) |

`llm-worker` のスコープ宣言は他より控えめで、「何かの上位層から呼ばれる前提」が読み取れる（`Interceptor` の存在がそれを裏付ける）。

---

## 2. レイヤ階層と被り

`Worker` が担っているレイヤを基準にすると、各プロジェクトの「同じ層」と「上下層」は次のように整理できる。

### 同じ層（== 競合）

| 機能 | llm-worker | rig | genai | swiftide |
|---|---|---|---|---|
| Provider 抽象 (HTTP) | ◎ 自前 (4) | ◎ 自前 (20+) | ◎ 自前 (18+) | ○ 外部 SDK ラップ |
| Streaming イベント正規化 | ◎ | ◎ | ◎ | ○ |
| Tool 定義 + 実行ループ | ◎ | ◎ | △ (定義のみ、loop は呼び出し側) | ◎ |
| Multi-turn loop | ◎ | ◎ | ✕ | ◎ |
| Hook / Interceptor | ◎ Interceptor + closure | ◎ PromptHook | △ Resolver のみ | ◎ 6+ Hook |
| 履歴管理 | ◎ Item / ContentPart | ◎ Message | △ ChatRequest を呼び出し側で append | ◎ MessageHistory trait |

### 上下層（== 補完関係）

| 機能 | llm-worker | rig | genai | swiftide |
|---|---|---|---|---|
| Embedding / VectorStore | ✕ | ◎ 10+ | △ (Embed のみ) | ◎ 9+ |
| RAG | ✕ | ◎ | ✕ | ◎ |
| Pipeline DAG | ✕ | ◎ `parallel!` / `try_op` | ✕ | ◎ Indexing pipeline |
| 永続化 | ✕ | △ | ✕ | ◎ MessageHistory swap |
| OTel / Observability | △ tracing のみ | ◎ GenAI Semantic Conv | △ | ◎ Langfuse 等 |
| 型状態でのキャッシュ保護 | **◎ 唯一** | ✕ | ✕ | ✕ |

---

## 3. コアの抽象を並べる

### `llm-worker::Worker<C, S>`
- `C: LlmClient`（プロバイダ）、`S: WorkerState`（`Mutable` | `Locked` の sealed trait）
- ライフサイクル: `Mutable` で履歴/プロンプト編集 → `lock()` → `Locked` で `run()` / `resume()` → `unlock()` で戻る
- `state.rs:1-61`: `Locked` 中は履歴 append-only、`set_system_prompt` 等は型レベルで呼べない
- `prune.rs:66-118`: `prunable_indices()` + `project()` で context のみ縮約。永続履歴は不変 = "context projection"

### `rig::Agent` + `PromptRequest`
- 状態機械型: `max_turns`、`extended_details()` で usage / message tracking
- `PromptHook` trait: `on_completion_call`, `on_completion_response`, `on_tool_call` (skip 可), `on_tool_result`, `on_text_delta`, …
- `ToolSet` + `ToolEmbedding` で RAG 可能なツール
- Pipeline DAG: `Op` trait + `parallel!` macro

### `genai::Client`
- `exec_chat(model, ChatRequest, options) -> ChatResponse`
- `ChatStreamEvent { Start, Chunk, ReasoningChunk, ToolCallChunk, End }`
- Adapter pattern: `to_web_request_data` / `to_chat_response` / `to_chat_stream`
- モデル名の prefix で provider 推論 (`gpt-` → OpenAI, `claude-` → Anthropic)
- 状態管理は呼び出し側（`ChatRequest` を毎ターン clone & append）

### `swiftide-agents::Agent`
- Builder: `llm`, `context`, `tools`, `toolboxes`, `hooks`
- `AgentContext` trait + `DefaultContext` (AtomicUsize で completions ポインタ管理)
- State enum: `Pending` / `Running` / `Stopped(StopReason)`
- Tool execution: `LocalExecutor` と MCP (Model Context Protocol)
- Provider 層は `async-openai` 0.33+ / `async-anthropic` 0.6 を流用

---

## 4. 三大独自点（`llm-worker` だけが持っているもの）

### 4.1 型状態によるキャッシュ保護 — `state::WorkerState`

```
Worker<C, Mutable>           Worker<C, Locked>
  ├─ history_mut()   ✓         ├─ history_mut()   ✗ (compile error)
  ├─ set_system_prompt() ✓     ├─ set_system_prompt() ✗
  ├─ register_tool() ✓         ├─ run() / resume()  ✓
  └─ lock() ─────────────────→ │
                               └─ unlock() ──────────→ Mutable
```

- **何を防ぐか**: KV cache の prefix が変わるような編集を `Locked` 中に行うこと（=次の `run()` で cache miss を引く事故）
- **誰がやっているか**: rig / genai / swiftide のいずれもキャッシュ整合性は「呼び出し側のお気持ち」レベル。genai は `CacheControl::Ephemeral / Stable` enum を持つが、それはマーク用であって状態機械ではない
- **意義**: production の Anthropic / OpenAI prompt cache 利用で、誤った `set_system_prompt` が静かに課金を倍増させる事故を**コンパイル時に潰せる**

### 4.2 Interceptor による上位層への決定委譲 — `interceptor.rs`

`Interceptor` trait が持つフック点:
- `on_prompt_submit` → `PromptAction`
- `pre_llm_request` → `PreRequestAction`
- `pre_tool_call` → `PreToolAction`
- `post_tool_call` → `PostToolAction`
- `on_turn_end` → `TurnEndAction`

**rig の `PromptHook` との違い**: rig のフックは「観察 + skip/abort のフラグ」。`llm-worker` は **戻り値が ActionEnum** で、上位層が次の挙動を選択できる（例: `PreToolAction::Override(custom_result)`）。これは `Worker` を「自律ループ」と「上位 orchestrator から駆動される実行器」の両方として使える設計上の選択。

### 4.3 Context Projection による prune — `prune.rs`

- 削るのは `Item::ToolResult { content }` の中身だけ。`Item` 自体は履歴に残す（summary は維持）
- `project()` は **イミュータブル変換** で永続履歴を変えない
- savings 推定は意図的に上位層に委譲（`prune.rs` 末尾コメント）
- swiftide も MessageHistory に `Summary` メッセージで似たことをやるが、**永続履歴を直接置換**する方式。`llm-worker` の方が rollback / 再生 / debugging に強い

---

## 5. 各競合の「これは llm-worker に勝っている」点

### rig
- **Provider 数**: 20+（OpenAI / Anthropic / Gemini / Bedrock / Vertex / Azure / Groq / DeepSeek / Cohere / Mistral / OpenRouter / Together / xAI / Perplexity / HuggingFace …）
- **VectorStore**: MongoDB / LanceDB / Neo4j / Qdrant / SQLite / SurrealDB / Milvus / ScyllaDB / S3Vectors / HelixDB
- **Pipeline DAG**: `parallel!` macro / `try_op` / agent_ops（Agent を Op として組み立て可能）
- **OTel**: GenAI Semantic Convention に完全準拠
- **WASM**: `rig-core` のみ WASM 互換
- **更新頻度**: 直近 20 commit が 1〜2 週間以内

### genai
- **Provider 数**: 18+
- **薄さ**: 6,539 LOC / 35 deps で multi-provider client が完成している（`llm-worker` の `llm_client/` は 1.5k LOC で 4 provider）
- **Native protocol サポート**: Anthropic reasoning, Gemini thinking, OpenAI strict mode, OpenAI Responses API の `previous_response_id`/`store` を素通し
- **Tool call streaming**: `ToolCallChunk` が分離イベント
- **Resolver** で auth / model / endpoint をフレキシブルに差し替え

### swiftide
- **Indexing pipeline**: streaming loader → transformer → chunker → storage の fluent パイプライン
- **Query state machine**: Pending → Retrieved → Answered の型状態（くしくも型状態だが対象が違う）
- **Hook 粒度**: `before_all` / `before_completion` / `after_completion` / `before_tool` / `after_tool` / `on_new_message` / `on_stop` 等 6+
- **MCP サポート**: ToolExecutor として LocalExecutor と MCP の両方
- **MessageHistory trait** で永続化バックエンド（Redis 等）に差し替え可能
- **既存 SDK の活用**: `async-openai` / `async-anthropic` をベースに薄く乗せる戦略

---

## 6. 各競合の「これは llm-worker の方が良い」点

### vs rig
- **キャッシュ整合性**: rig は `extended_details()` で usage を観測できるが、prefix 変更を**防止**するメカニズムはない
- **依存の薄さ**: rig-core 25.5K LOC + 大量 sub-crate vs llm-worker 5.8K LOC
- **Worker の単一責務**: rig の `Agent` は LLM 呼び出し + Tool + (optional) RAG までを抱え込む。`llm-worker` は orchestrator を分離

### vs genai
- **Tool 実行ループ**: genai は呼び出し側で書く必要がある（README/example で明示）
- **会話履歴の構造化**: genai は `ChatRequest` を毎ターン clone する素朴な API
- **Interceptor**: genai は `Resolver` で初期化時のカスタマイズのみ。実行中フックなし

### vs swiftide
- **Provider 直接実装**: swiftide は `async-openai` 0.33 / `async-anthropic` 0.6 に依存しており、その上流に出来る/出来ないが縛られる。`llm-worker` は HTTP まで自前で握る
- **Agent loop の中核設計**: swiftide-agents は `DefaultContext` の AtomicUsize ベースで素朴。型状態保護なし
- **依存の少なさ**: swiftide-agents 単体でも `async-openai` を呼ぶ前提

---

## 7. 「ライブラリ化したとき競合するか」の解像度

| 相手 | 競合度 | 理由 |
|---|---|---|
| **rig** | 🟥 高 | 同じ "agent + tool + multi-provider" を狙う層。rig は既にエコシステムを持っており、正面衝突は不利 |
| **genai** | 🟨 中 | 層が違う（client vs worker）。むしろ `llm-worker` の `llm_client/` を捨てて genai に乗る選択肢がある |
| **swiftide** | 🟩 低 | 一次目的が違う（pipeline）。agent の中核実行を `llm-worker` に置き換える "embed" 戦略すら成立し得る |

---

## 8. ポジショニング案（仮にライブラリ化するなら）

`llm-worker` がぶつからない・かつ需要がある場所を考えると、次のような立ち位置が現実的:

> **"Production-grade LLM worker primitive — 型状態でキャッシュ整合性を守り、Interceptor で上位層 (rig / swiftide / 自社 orchestrator) から駆動できる低レベル実行器"**

具体的な打ち出し方:
1. **キャッシュ整合性をコンパイル時に保証する唯一のクレート** という明確な売り文句。Anthropic の prompt cache 課金事故で困った人を狙う
2. **Tool 実行ループ + Interceptor** をプリミティブとして提供し、上位フレームワークから plug できることを主張
3. **HTTP まで自前** はやめて、`llm_client/` を外す or feature 化し、`genai` バックエンドを optional 提供する選択肢も検討に値する（実装コストを 1.5k LOC 削れる）
4. **Pipeline / RAG / VectorStore は提供しない** ことを明示。rig / swiftide の代替を狙わないことで、共存ストーリーが書ける

---

## 9. ライブラリ化の判断材料（再）

- **需要がある層か**: yes。rig / swiftide / genai は揃って "もう一段下のキャッシュ整合性プリミティブ" を持っていない
- **既存と被るか**: 上記 3 案でかわせる
- **維持コスト**: API 安定化 + provider 追従が恒常的に乗る。`llm_client/` を外すか genai に寄せるかでだいぶ軽くなる
- **タイミング**: yoi 本体がリリースされ、`Worker` の API が stress test を受ける前に公開すると、後から破壊的変更を強いられる。**先に yoi をリリースして、production 使用例として参照させてからライブラリ化する**方が安全

---

## 付録: 主要ファイルへの参照

### llm-worker
- `crates/llm-worker/src/lib.rs:1-59` — モジュール構成
- `crates/llm-worker/src/worker.rs:101-191` — Worker<C, S>
- `crates/llm-worker/src/state.rs:1-61` — Mutable / Locked 型状態
- `crates/llm-worker/src/interceptor.rs:1-147` — Interceptor trait
- `crates/llm-worker/src/prune.rs:66-118` — context projection
- `crates/llm-worker/src/llm_client/{auth,client,event,transport,types}.rs`
- `crates/llm-worker/src/tool_server.rs:27-52` — Deferred 登録

### rig
- `github.com/0xPlaygrounds/rig/rig/rig-core/src/{agent,completion,tool,pipeline,vector_store,streaming}/mod.rs`

### genai
- `github.com/jeremychone/rust-genai/src/lib.rs:1-25`
- `.../src/client/client_impl.rs:62-67`
- `.../src/chat/chat_request.rs:10-34`
- `.../src/chat/chat_stream.rs:11-86`
- `.../src/adapter/adapter_types.rs:11-59`

### swiftide
- `github.com/bosun-ai/swiftide/swiftide-agents/src/agent.rs:45-100`
- `.../swiftide-agents/src/hooks.rs`
- `.../swiftide-core/src/chat_completion/traits.rs`
- `.../swiftide-indexing/src/pipeline.rs`
- `.../swiftide-query/src/`
