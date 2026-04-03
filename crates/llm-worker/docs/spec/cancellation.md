# 非同期キャンセル仕様

Workerの非同期キャンセル機構についての仕様ドキュメント。

## 概要

`tokio::sync::mpsc`チャネル（バッファサイズ1）を用いて、別タスクからWorkerの実行を安全にキャンセルできる。Worker内部では`tokio::select!`により、ストリーム受信・ツール実行の各フェーズでキャンセルシグナルを検知する。

## 基本的な使い方

### cancel() メソッドによるキャンセル

```rust
let worker = Arc::new(Mutex::new(Worker::new(client)));

// 実行タスク
let w = worker.clone();
let handle = tokio::spawn(async move {
    w.lock().await.run("prompt").await
});

// キャンセル（try_sendによる非同期安全な送信）
worker.lock().await.cancel();
```

### cancel_sender() によるキャンセル

ロックを取得せずにキャンセルする場合、事前に`Sender`を取得しておく。

```rust
let worker = Arc::new(Mutex::new(Worker::new(client)));

// ロック中にSenderを取得
let cancel_tx = {
    let w = worker.lock().await;
    w.cancel_sender()
};

// 実行タスク
let worker_clone = worker.clone();
let task = tokio::spawn(async move {
    let mut w = worker_clone.lock().await;
    w.run("Tell me a long story").await
});

// 別タスクからキャンセル（ロック不要）
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = cancel_tx.send(()).await;
});

task.await?;
```

## API

| メソッド / フィールド | 説明                                          |
| --------------------- | --------------------------------------------- |
| `cancel()`            | `try_send`でキャンセルをトリガー              |
| `cancel_sender()`     | `mpsc::Sender<()>`のcloneを返す               |
| `is_cancelled()`      | キャンセルキューにシグナルがあるか確認        |
| `last_run_interrupted()` | 前回のrunが中断されたかどうか              |

## キャンセル検知ポイント

Worker内部には複数のキャンセル検知ポイントが存在する。

### 1. ターンループ先頭

```rust
loop {
    if self.try_cancelled() {
        self.timeline.abort_current_block();
        return Err(WorkerError::Cancelled);
    }
    // ...
}
```

各ターンの開始時に`try_recv()`でキャンセルキューを確認する。

### 2. ストリーム取得時

```rust
let mut stream = tokio::select! {
    stream_result = self.client.stream(request) => stream_result?,
    cancel = self.cancel_rx.recv() => {
        self.timeline.abort_current_block();
        return Err(WorkerError::Cancelled);
    }
};
```

LLMクライアントへのリクエスト送信中にキャンセル可能。

### 3. ストリーム受信中

```rust
loop {
    tokio::select! {
        event_result = stream.next() => {
            // イベント処理
        }
        cancel = self.cancel_rx.recv() => {
            self.timeline.abort_current_block();
            return Err(WorkerError::Cancelled);
        }
    }
}
```

ストリーミング中のイベント受信ループで、各イベント間にキャンセルが割り込める。

### 4. ツール並列実行中

```rust
let mut results = tokio::select! {
    results = join_all(futures) => results,
    cancel = self.cancel_rx.recv() => {
        self.timeline.abort_current_block();
        return Err(WorkerError::Cancelled);
    }
};
```

`join_all`によるツール並列実行中にもキャンセル可能。

## キャンセル時の処理フロー

```
キャンセルシグナル検知
    ↓
timeline.abort_current_block()  // 進行中ブロックの終端処理
    ↓
last_run_interrupted = true     // 中断フラグをセット
    ↓
Err(WorkerError::Cancelled) を返す
    ↓
finalize_interruption()         // 中断の最終処理
    ↓
run_on_abort_hooks("Cancelled") // on_abort フック呼び出し
    ↓
Err(WorkerError::Cancelled) を返す（呼び出し元へ）
```

## キャンセルキューの管理

### drain_cancel_queue

`run_turn_loop()`の開始時に、キューに溜まった古いキャンセルシグナルを排出する。これにより、前回のキャンセルが次回の`run()`に影響することを防ぐ。

```rust
fn drain_cancel_queue(&mut self) {
    loop {
        match self.cancel_rx.try_recv() {
            Ok(()) => continue,
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        }
    }
}
```

### try_cancelled

ノンブロッキングでキャンセル状態を確認する。チャネルがdisconnectedの場合もキャンセル扱いとなる。

```rust
fn try_cancelled(&mut self) -> bool {
    match self.cancel_rx.try_recv() {
        Ok(()) => true,
        Err(TryRecvError::Empty) => false,
        Err(TryRecvError::Disconnected) => true,
    }
}
```

## 中断状態の管理

### last_run_interrupted フラグ

Workerは`last_run_interrupted`フラグで前回の実行が中断されたかどうかを追跡する。

- `run()` / `resume()` の開始時に`false`にリセット
- エラー発生時に`true`にセット
- `Pause`による中断時にも`true`にセット
- 正常終了（`WorkerResult::Finished`）時に`false`にセット

### finalize_interruption

すべての`run()`/`resume()`の結果は`finalize_interruption()`を経由して返される。結果が`Err`の場合、中断理由を抽出して`on_abort`フックを呼び出す。

```rust
async fn finalize_interruption<T>(&mut self, result: Result<T, WorkerError>) -> Result<T, WorkerError> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            self.last_run_interrupted = true;
            let reason = match &err {
                WorkerError::Aborted(reason) => reason.clone(),
                WorkerError::Cancelled => "Cancelled".to_string(),
                _ => err.to_string(),
            };
            self.run_on_abort_hooks(&reason).await?;
            Err(err)
        }
    }
}
```

## on_abort フック

`on_abort`フックはキャンセルだけでなく、あらゆる中断時に発火する。

**入力**: `&mut String` - 中断理由

**発火条件**:

- `WorkerError::Cancelled` -- reason: `"Cancelled"`
- `WorkerError::Aborted(reason)` -- reason: フックが指定した理由
- `WorkerError::Client(e)` -- reason: エラーの表示文字列
- `WorkerError::Tool(e)` -- reason: エラーの表示文字列
- `WorkerError::Hook(e)` -- reason: エラーの表示文字列

```rust
struct CleanupHook;

#[async_trait]
impl Hook<OnAbort> for CleanupHook {
    async fn call(&self, reason: &mut String) -> Result<(), HookError> {
        tracing::info!("Worker aborted: {}", reason);
        Ok(())
    }
}
```

## resume() との関係

`resume()`はPause状態からの再開に使用される。内部では`run_turn_loop()`を呼び出し、保留中のツール呼び出し（historyに`FunctionCall`があるが対応する`FunctionCallOutput`がないもの）を検出して実行を再開する。

`resume()`中もキャンセルは同様に機能し、`finalize_interruption()`経由で`on_abort`フックが発火する。

## WorkerError の種別

| エラー種別            | 発生条件                                      |
| --------------------- | --------------------------------------------- |
| `Cancelled`           | mpscチャネル経由のキャンセルシグナル受信       |
| `Aborted(String)`     | フックによるAbort/Cancel、またはstream hookのPause |
| `Client(ClientError)` | LLMクライアントのエラー                        |
| `Tool(ToolError)`     | ツール実行エラー                               |
| `Hook(HookError)`     | フック実行中のエラー                           |
| `ConfigWarnings(Vec)` | サポートされていない設定オプション             |
