# llm-engine 要件

## 前提

a. userメッセージを追加しなくてもagentの途中ママ投げれば、AIはそれを自身の生成途中と認識して普通に継続する
b. KVキャッシュは速度・効率の面で有利で、コンテキストの事後改変はキャッシュヒット率を大幅に下げる
c. ツール・フックの基本的なスキーマ自動化を提供する

## 要件

### R1: Resume/Pause

メッセージの送信と生成のResume、一時停止/再開。

- `Engine::run()` でターンを開始
- フックから `Pause` を返してターンを一時停止
- `Engine::resume()` でユーザーメッセージを追加せず継続
- AIは中断を認識せず、継続として処理する

**実装**: `engine.rs` — `resume()`, `get_pending_tool_calls()`, `EngineResult::Paused`

### R2: 暗黙的KVキャッシュ保証

キャッシュを破壊しうる操作を明示的にブロックせずとも、いつの間にかキャッシュ破壊してた状態にはしたくない。

- Type-stateパターン（`Mutable` / `CacheLocked`）でコンパイル時に保証
- `Engine::lock()` でCacheLocked状態に遷移
- CacheLocked状態ではシステムプロンプトや履歴の変更APIが型レベルで利用不可
- `locked_prefix_len` でプレフィックスの不変性を追跡

**実装**: `state.rs` (sealed trait), `engine.rs` (state-specific impl blocks)

### R3: ツール・フックスキーマ自動化

- `#[tool]` マクロでツール定義を自動生成
- `#[tool_registry]` マクロでツールサーバーを自動構成
- `Hook` traitで10種のフックポイント

**実装**: `llm-engine-macros/`, `tool.rs`, `tool_server.rs`, `hook.rs`

### R4: フックは上層の関心事

フックはLLMクライアント層ではなく、Engine（オーケストレーション）層に配置する。

- LLMクライアント (`llm_client/`) はストリーミングとプロトコルのみ
- Engine層でフック実行、ツール統合、Pause/Resume制御

**実装**: `engine.rs` (hook integration), `hook.rs` (trait definitions)
