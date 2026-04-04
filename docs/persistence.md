# 永続化設計

## 概要

`llm-worker-persistence` クレートは、`llm-worker` の `Worker` セッション状態を
JSONL append-only ログとして永続化する。ログを replay することで Worker 状態を完全に復元する。

## 設計方針

- **JSONL append-only ログ**: 1セッション = 1つの `.jsonl` ファイル。書き込みは末尾追記のみ。
- **Pause/正常終了で構造に差異なし**: Worker の状態は Pause 時も正常終了時も同じ形
  (`history: Vec<Item>` + `turn_count` + `request_config`)。
  `resume()` は「ユーザー入力を追加せず `run_turn_loop()` に再入する」だけなので、
  復元に必要なのは history の中身であり、前回の終了理由ではない。
  `RunOutcome` の `Finished`/`Paused` 区分は監査用メタデータであり、replay の分岐には使わない。
- **クレート分離**: `llm-worker` は永続化を知らない。`Session` ラッパーが外から Worker を包む。

## 命名規約

| 名前 | 用途 |
|---|---|
| **SessionLog / LogEntry** | 状態復元用の構造化された記録（永続化の本体） |
| **EventTrace / TraceEntry** | デバッグ用の生ストリームイベント全録（オプション、デフォルト OFF） |

## クレート構成

```
llm-worker-persistence → llm-worker → llm-worker-macros
```

`llm-worker-persistence` は `llm-worker` に依存するが、逆方向の依存はない。

## ファイル配置

```
{root}/{session_id}.jsonl          -- セッションログ
{root}/{session_id}.trace.jsonl    -- イベントトレース（デバッグ時のみ）
```

`SessionId` は UUID v7（`uuid` クレート）。タイムスタンプ埋め込みで辞書順 = 時系列順。

## LogEntry

各エントリは Worker の特定の状態変更に対応する:

| エントリ | Worker 上の対応箇所 | replay での効果 |
|---|---|---|
| `SessionStart` | セッション開始 / fork | system_prompt, config, history を初期化 |
| `UserInput` | `worker.rs:229` | history に追加 |
| `AssistantItems` | `worker.rs:1040-1041` | history に追加 |
| `ToolResults` | `worker.rs:897-900, 1072-1076` | history に追加 |
| `HookInjectedItems` | `worker.rs:1055` | history に追加 |
| `TurnEnd` | `worker.rs:1033` | turn_count を更新 |
| `CacheLocked` | `Worker::lock()` | locked_prefix_len を設定 |
| `CacheUnlocked` | `Worker::unlock()` | locked_prefix_len を 0 に |
| `RunOutcome` | `run()` / `resume()` 終了時 | interrupted フラグのみ（監査用） |
| `ConfigChanged` | `set_*` メソッド群 | config を更新 |

## Session ラッパー

```rust
pub struct Session<C: LlmClient, St: Store> {
    pub worker: Worker<C, Mutable>,
    store: St,
    session_id: SessionId,
}
```

- `Session::new()` — SessionStart を書き込み
- `Session::run()` — Worker::run() の前後で history を比較、差分をログ記録
- `Session::resume()` — 同上
- `Session::restore()` — ログを replay して Worker を再構築
- `Session::fork()` — 現在の history をシードにした新セッションを作成
- `Session::fork_at()` — 任意のログ地点から分岐

## Store trait

```rust
pub trait Store: Send + Sync {
    fn append(&self, id: SessionId, entry: &LogEntry) -> impl Future<...> + Send;
    fn read_all(&self, id: SessionId) -> impl Future<...> + Send;
    fn list_sessions(&self) -> impl Future<...> + Send;
    fn create_session(&self, id: SessionId, entries: &[LogEntry]) -> impl Future<...> + Send;
    fn exists(&self, id: SessionId) -> impl Future<...> + Send;
    fn append_trace(&self, id: SessionId, entry: &TraceEntry) -> impl Future<...> + Send;
}
```

初期実装は `FsStore`（ファイルシステム JSONL）。RPITIT 使用、`async_trait` 不要。
