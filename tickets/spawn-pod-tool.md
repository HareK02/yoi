# SpawnPod ツール: LLM から Pod を生成する

## 背景

オーケストレーションの起点。LLM が「このタスクを別 Pod に任せたい」と判断したとき、`SpawnPod` ツールを呼び出して新しい Pod プロセスを起動する。

## 依存

- `tickets/scope-lock.md`: scope 分譲の記録基盤

## 仕様

### ツール定義

`SpawnPod` は通常の `Tool` trait 実装として Worker に登録される。

入力:
- `name`: spawned Pod の識別名
- `instruction`: instruction ファイル参照（省略時は `$insomnia/default`）
- `task`: 最初のメッセージ（spawn 後に即座に run される）
- `scope`: 譲渡する scope 定義（allow ルール）。spawner の effective scope のサブセットでなければならない

出力:
- spawned Pod の `name` と接続先 address（socket path）

### 内部動作

1. scope lock file を flock → spawner の effective scope を確認 → 要求された scope がサブセットか検証
2. spawner の allocation に deny を追記 + 新 Pod の allocation を登録（`delegated_from` = spawner）→ unlock
3. PodFactory のカスケード（user / project manifest）に spawner からの overlay（name, pwd, scope, instruction）を重ねて PodManifest を構築
4. `pod` バイナリを独立プロセスとして起動（`Command::new(pod_command)`）
5. spawned Pod の socket が利用可能になるまで待機
6. spawner の callback address を spawned Pod に渡す（`Method::Notify` 経由の受け口として）
7. `task` を `Method::Run` で送信
8. spawn 記録を Pod のランタイム状態に保存

### Pod 起動コマンド

- デフォルト: `pod`（PATH 上のバイナリ）
- 環境変数 `INSOMNIA_POD_COMMAND` またはユーザー manifest で上書き可能
- 引数: `--overlay <toml>` で scope / instruction / name / pwd を渡す

### spawn 記録

Pod が保持する既知の Pod リスト。各エントリ:
- `name`, `name`, `socket_path`, `scope_delegated`, `callback_address`
- spawner 復帰時にこの記録を読んで再接続する

## 設計で決めること

- **spawn 記録の永続化**: session-store に載せるか、runtime_dir にファイルとして書くか
- **socket 待機のタイムアウト**: spawned Pod が socket を開くまでの待機時間
- **callback address の形式**: ローカルでは spawner の socket path、リモートでは `insomnia@host:pod-name`

## 完了条件

- LLM が `SpawnPod` ツールを呼び出すと、新しい Pod プロセスが独立して起動する
- scope lock file に分譲が記録され、spawner の effective scope が縮小する
- 要求 scope が spawner の effective scope を超えていたらツールエラー
- spawned Pod の socket に接続でき、`task` が `Method::Run` で送信される
- spawn 記録が保存され、`ListPods` で参照できる
- spawner が停止しても spawned Pod は続行する

## 範囲外

- Pod 間通信ツール（SendToPod / ReadPodOutput / StopPod / ListPods）は `tickets/pod-comm-tools.md`
- コールバック通知は `tickets/pod-callback.md`
- リモート spawn（SSH 越し）は `docs/network-peering.md` を参照
