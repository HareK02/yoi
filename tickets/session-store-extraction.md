# session-store: persistence クレートの再構成

## 背景

`llm-worker-persistence` は名前・構造ともに llm-worker のサブクレートに見えるが、
実態はセッション管理という上位層の関心を持っている。

現状の `Session` は Worker を wrap して `run()`/`resume()` をインターセプトするが、
永続化のためにレイヤーとして呼び出しパスに噛む必要はない。
Worker からセッション状態を抜き出して保存する／復元するだけで十分。

## 方針

- クレート名を `llm-worker-persistence` → `session-store` に変更
- `Session` の Worker wrap を廃止し、save/restore の関数群にする
- Pod が Worker を直接保持し、run 後に session-store の関数を呼ぶ
- `llm-worker` への型依存（`Item`, `RequestConfig`）はそのまま残す（構造的に層にならなければ問題ない）

## 現状の構造

```
Controller → Pod → Session (wraps Worker) → Worker
                    ↑ run()/resume() をインターセプト
```

`pod.session_mut().worker_mut()` と2段潜る必要がある。

## 変更後の構造

```
Controller → Pod → Worker (直接保持)
               │
               └─ run 後に session_store::save_delta(store, ...) を呼ぶ
                  restore 時に session_store::restore(store, id) → state を返す
```

## 変更内容

### session-store クレート（旧 llm-worker-persistence）

- `Session` struct を廃止
- save 系関数を提供: history delta の記録、turn end、outcome 等
- restore 関数: ログ再生 → `RestoredState` を返す（Worker は作らない）
- `Store` trait, `FsStore`, `LogEntry`, ハッシュチェーンはそのまま維持
- fork / fork_at も関数として残す

### pod クレート

- `Pod` が `Worker` を直接フィールドに持つ
- `Pod::run()` 内で Worker を呼び、その後 session-store の save 関数を呼ぶ
- `Pod::restore()` は session-store から `RestoredState` を受け取り、Worker に適用
- Controller は `pod.worker()` / `pod.worker_mut()` で直接アクセス

### 影響範囲

- `Session` を使っている箇所: `pod.rs`, `controller.rs`, テスト
- `SessionError` が消えるので、`PodError` から `SessionError` variant を除去し、`StoreError` に置換
