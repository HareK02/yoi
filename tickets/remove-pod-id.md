# PodId (UUID) の削除

## 背景

Pod は一時的なプロセス的存在であり、永続的アイデンティティは Session が持つ。現在 `PodId = uuid::Uuid` が `Pod` 構造体に存在するが、ファイルシステム・プロトコル・外部発見はすべて `pod_name` ベースで動いており、PodId を使って何かを引くコードがない。

## やること

- `PodId` 型、`new_pod_id()`、`Pod.id` フィールド、`Pod::id()` getter を削除
- `Pod::restore` から `id: PodId` 引数を削除
- `pod` クレートの `uuid` 依存を削除（`SessionId` は llm-worker-persistence 側なので影響なし）
- Pod の識別は `pod_name`（マニフェスト由来）に統一

## 判断根拠

- 「どの Pod か」→ name で十分（同名 Pod は存在しない前提）
- 「どの実行か」→ SessionId が担当済み
- 再接続フロー: name でランタイムディレクトリを発見 → status.json の session_id で Session を復元。PodId の出番がない
