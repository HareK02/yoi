# 永続化データ構造の設計・実装プラン

## Context

INSOMNIA の `llm-worker` クレートは現在すべてのセッション状態をインメモリで保持しており、
プロセス終了時に会話履歴やターン状態が失われる。
Coding Agent として Pause/Resume・Fork をまたいだセッション継続を実現するため、
永続化レイヤーを追加する。

**設計方針**: Codex CLI / Claude Code と同様の **JSONL append-only ログ** 方式を採用。
セッションログを replay することで Worker 状態を完全に復元する。
Pause/正常終了で永続化データの構造に差異を設けない。
理由: Worker の状態は Pause 時も正常終了時も同じ形（`history: Vec<Item>` + `turn_count` + `request_config`）であり、
APIレスポンスのデータ構造上、両者に本質的な違いがない。
`resume()` は「ユーザー入力を追加せず `run_turn_loop()` に再入する」だけなので、
復元に必要なのは history の中身であり、前回の終了理由ではない。
`RunOutcome` の `Finished`/`Paused` 区分は監査・デバッグ用メタデータであり、replay ロジックの分岐には使わない。

**命名規約**:
- **SessionLog / LogEntry** — 状態復元用の構造化された記録（永続化の本体）
- **EventTrace / TraceEntry** — デバッグ用の生ストリームイベント全録（オプション）

## クレート構成

永続化は `llm-worker` とは別クレートに分離する。
`llm-worker` は永続化を一切知らず、Session ラッパーが外から Worker を包む。

```
crates/
  llm-worker/                ← 既存（LLM クライアント + Worker、変更なし以外は最小限）
  llm-worker-macros/         ← 既存（変更なし）
  llm-worker-persistence/    ← 新規クレート
    Cargo.toml
    src/
      lib.rs              -- モジュールルート・re-exports・SessionId 型エイリアス
      session_log.rs      -- LogEntry enum（JSONL 1行 = 1エントリ）
      event_trace.rs      -- TraceEntry（デバッグ用生イベント記録）
      store.rs            -- Store trait（バックエンド抽象）
      fs_store.rs         -- JSONL ファイルシステム実装
      session.rs          -- Session<C, St> ラッパー + replay/restore
  insomnia/                  ← 将来のトップレベルアプリ

docs/persistence.md          -- 設計ドキュメント（このプランの清書版）
```

依存グラフ:
```
llm-worker-persistence → llm-worker → llm-worker-macros
insomnia (将来) → llm-worker-persistence, llm-worker
```

### llm-worker-persistence/Cargo.toml 依存

```toml
[dependencies]
llm-worker = { path = "../llm-worker" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["fs", "io-util"] }
uuid = { version = "1", features = ["v7", "serde"] }
thiserror = "2"
```

## 既存コードへの変更（llm-worker 側）

### 1. `RequestConfig` に Serialize/Deserialize 追加
- **ファイル**: `crates/llm-worker/src/llm_client/types.rs:504`
- `#[derive(Debug, Clone, Default)]` → `#[derive(Debug, Clone, Default, Serialize, Deserialize)]`

### 2. Worker に復元用セッター追加
- **ファイル**: `crates/llm-worker/src/worker.rs`
- `impl<C: LlmClient> Worker<C, Mutable>` ブロックに追加:
  - `pub fn set_turn_count(&mut self, count: usize)`
  - `pub fn set_last_run_interrupted(&mut self, interrupted: bool)`

### 3. ワークスペース Cargo.toml にメンバー追加
- **ファイル**: `Cargo.toml`（ワークスペースルート）
- `members` に `"crates/llm-worker-persistence"` を追加

## 新規コード: データ型

### LogEntry（session_log.rs）

状態復元に必要な構造化記録。JSONL 1行 = 1エントリ。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEntry {
    // セッション開始（ログ先頭、fork 時は history にシード状態を含む）
    SessionStart {
        ts: u64,
        system_prompt: Option<String>,
        config: RequestConfig,
        history: Vec<Item>,
    },

    // ユーザー入力（worker.rs:229 に対応）
    UserInput { ts: u64, item: Item },

    // アシスタント応答（worker.rs:1040-1041 に対応）
    AssistantItems { ts: u64, items: Vec<Item> },

    // ツール実行結果（worker.rs:897-900, 1072-1076 に対応）
    ToolResults { ts: u64, items: Vec<Item> },

    // Hook 注入 Items（worker.rs:1055 ContinueWithMessages に対応）
    HookInjectedItems { ts: u64, items: Vec<Item> },

    // ターン境界
    TurnEnd { ts: u64, turn_count: usize },

    // KV キャッシュロック/アンロック
    CacheLocked { ts: u64, locked_prefix_len: usize },
    CacheUnlocked { ts: u64 },

    // run/resume の終了結果
    RunOutcome { ts: u64, outcome: Outcome, interrupted: bool },

    // RequestConfig 変更
    ConfigChanged { ts: u64, config: RequestConfig },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome { Finished, Paused, Error { message: String } }
```

**Replay ロジック**: 全エントリ種別を走査し、`*Items` / `UserInput` → history に append、
`TurnEnd` → turn_count 更新、`CacheLocked` → locked_prefix_len 設定。

### TraceEntry（event_trace.rs）

デバッグ用の生ストリームイベント全録。デフォルト OFF。
セッションログとは別ファイル `{session_id}.trace.jsonl` に記録。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub ts: u64,
    pub turn: usize,
    pub event: Event,
}
```

replay 対象外。状態復元には使わない。

### SessionId

`uuid` クレートの UUID v7 をそのまま使用。型エイリアスのみ。

```rust
pub type SessionId = uuid::Uuid;

pub fn new_session_id() -> SessionId {
    uuid::Uuid::now_v7()
}
```

UUID v7 はタイムスタンプ埋め込みで辞書順 = 時系列順。独自フォーマット不要。

### Store trait（store.rs）

```rust
pub trait Store: Send + Sync {
    fn append(&self, id: SessionId, entry: &LogEntry) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn read_all(&self, id: SessionId) -> impl Future<Output = Result<Vec<LogEntry>, StoreError>> + Send;
    fn list_sessions(&self) -> impl Future<Output = Result<Vec<SessionId>, StoreError>> + Send;
    fn create_session(&self, id: SessionId, entries: &[LogEntry]) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn exists(&self, id: SessionId) -> impl Future<Output = Result<bool, StoreError>> + Send;

    // EventTrace 用（デバッグモード時のみ使用）
    fn append_trace(&self, id: SessionId, entry: &TraceEntry) -> impl Future<Output = Result<(), StoreError>> + Send;
}
```

RPITIT (Rust 1.75+) 使用。`async_trait` 不要。

### FsStore（fs_store.rs）

ファイル配置:
- セッションログ: `{root}/{session_id}.jsonl`
- イベントトレース: `{root}/{session_id}.trace.jsonl`

append モードで書き込み。SQLite インデックスなし。

### Session ラッパー（session.rs）

Worker を直接変更せず、**外部ラッパー** として実装:

```rust
pub struct Session<C: LlmClient, St: Store> {
    pub worker: Worker<C, Mutable>,  // pub で直接アクセス可能
    store: St,
    session_id: SessionId,
    config: SessionConfig,
}
```

- `Session::new()` → SessionStart を append
- `Session::run()` → Worker::run() の前後で history.len() を比較、差分をログ記録
- `Session::resume()` → 同上
- `Session::fork()` → 現在の history をシードにした新 SessionStart を書き込み
- `Session::fork_at(store, source_id, entry_idx)` → 任意地点から分岐

**復元**: `restore_session(client, store, session_id)` → read_all → replay_entries → Worker 再構築

### EventTrace 記録（デバッグモード）

`SessionConfig::record_event_trace: bool`（デフォルト `false`）が `true` の場合、
Session が Worker に `OnStreamChunk` Hook を登録。
Hook 内で `TraceEntry` を `{session_id}.trace.jsonl` に append。
セッションログとは完全に分離。

## 実装順序

1. `RequestConfig` に Serialize/Deserialize 追加（llm-worker 側）
2. Worker に `set_turn_count` / `set_last_run_interrupted` 追加（llm-worker 側）
3. `crates/llm-worker-persistence/` クレート作成（Cargo.toml + ワークスペース登録）
4. `session_log.rs` 作成（LogEntry + Outcome + replay_entries）
5. `event_trace.rs` 作成（TraceEntry）
6. `store.rs` 作成（Store trait + StoreError）
7. `fs_store.rs` 作成（JSONL ファイルシステム実装）
8. `session.rs` 作成（Session ラッパー + restore_session）
9. `lib.rs` 作成（re-exports・SessionId 型エイリアス・new_session_id）
10. テスト作成（replay round-trip, FsStore 読み書き, Session::run ログ記録）
11. `docs/persistence.md` 設計ドキュメント作成

## 検証方法

1. **ユニットテスト**: `replay_entries` に手動構築した LogEntry 列を渡し、復元状態を検証
2. **統合テスト**: MockLlmClient + FsStore で Session::run → restore_session → history 一致を確認
3. **Fork テスト**: fork → 新セッションの history が fork 時点と一致することを確認
4. **cargo test**: 既存テストが壊れていないことを確認
5. **cargo clippy / cargo check**: 警告なし
