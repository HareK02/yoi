# pod

独立したエージェント実行単位「Pod」を実装するクレート。LLM ワーカーセッションをマニフェスト設定・ファイルスコープ制約と組み合わせ、Unix ソケット経由の双方向通信で操作可能にする。

## 公開型

### コア

- `Pod<C, St>` — LLM ワーカーセッション + マニフェスト + スコープのラッパー（`run()`, `resume()`, `from_manifest()`）
- `PodRunResult` — 実行結果（`Finished`, `Paused`）
- `PodError` — エラー型

### 制御

- `PodController` — Pod ライフサイクルを管理するアクター（`spawn()` でタスク起動）
- `PodHandle` — Pod への操作ハンドル（`send()`, `subscribe()`）
- `PodSharedState` / `PodStatus` — 共有状態（`Idle`, `Running`, `Paused`）

### ランタイム

- `RuntimeDir` — `$XDG_RUNTIME_DIR/yoi/{pod_name}/` 配下のランタイムディレクトリ管理（ステータス・履歴のアトミック書き込み）
- `SocketServer` — Pod Protocol 用 Unix ソケットサーバー
