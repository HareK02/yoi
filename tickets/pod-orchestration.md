# Pod オーケストレーション: LLM によるマルチエージェント分業

## 背景

Insomnia の差別化の中核は「複数の Pod が独立プロセスとして並行動作し、LLM が自律的に分業・統括できる」点にある。シングルエージェントの LLM リグが1つのコンテキストで全タスクを処理するのに対し、Insomnia は**タスクを別 Pod に委譲し、結果を集約してフィードバックする**オーケストレーションを LLM 自身に行わせることで、コンテキスト長・専門性・並列性の壁を突破する。

現状の Pod は単独で動くことしかできず、Pod 間の連携は外部の人間が仲介するしかない。LLM が「この調査は別の Pod に任せて、結果が返ってきたら本筋に統合する」という判断を**ツールとして**実行できる仕組みが無い。

## ゴール

Pod の LLM が、ツール呼び出しを通じて別 Pod のライフサイクル全体（生成・指示・結果読み取り・終了）を制御でき、spawned Pod の進捗を非同期に受け取れる基盤を設計・実装する。あわせて、人間がこの Pod ネットワークを監視・介入できる観測経路を用意する。

## コンセプト: 独立したピアとしての Pod

```
Human (GUI / TUI) ─── Pod A (orchestrator)
                          ├── socket ──→ Pod B (researcher)
                          ├── socket ──→ Pod C (coder)
                          │                  └── socket ──→ Pod D (reviewer)
                          └── socket ──→ Pod E (tester)
```

### プロセス独立

- spawn された Pod は **完全に独立したプロセス** として動作する。OS レベルの親子関係（subprocess）は持たない
- spawner が落ちても、spawned Pod は**続行**する。再接続も可能
- spawner と spawned Pod の関係は「socket client として接続している」だけ。関係性はプロセスではなく**ソケット接続**で表現される
- すべての Pod は同格。「誰が spawn したか」は Pod の runtime 属性であり、プロセスの従属関係ではない
- Pod の発見には**レジストリ**（runtime_dir 内の socket パスを列挙、または明示的な登録）が必要

### Scope の分譲

- Pod A が Pod B を spawn するとき、A は自身の scope の一部を B に**譲渡**する
- 譲渡した scope 領域は A の effective scope から **deny** される（A は自分が譲った部分にアクセスできなくなる）
- これにより**構造的に排他制御が保証**される：同一パスに対して write 権限を持つ Pod は常に高々1つ
- Pod B が終了すると、譲渡された scope は A に**返却**される（A の deny が解除される）
- Pod B が scope の一部をさらに Pod D に分譲した場合、B の終了時に D が保持している分は D にそのまま残る（D は独立して生きているため）。D が終了するまで、その scope 領域は誰にも返却されない

### `tickets/scope-exclusion.md` との関係

scope 分譲により Pod 間の write 排他制御は spawn 時に構造的に保証される。`tickets/scope-exclusion.md` が扱おうとしていた「複数 Pod が同一パスに同時 write する問題」は、**本チケットの scope 分譲モデルで吸収される**。

ただし、分譲ではなく**共有 read** (複数 Pod が同じパスを同時に read する) は引き続き許可される。read 同士は衝突しない。

## LLM に公開するツール群

Pod の LLM が使えるツールとして以下を設計する。いずれも通常の Tool trait 実装で、Worker に登録される。

### `SpawnPod`

新しい Pod を起動し、scope の一部を譲渡する。

入力:
- `name`: spawned Pod の識別名（spawner のスコープ内で一意）
- `instruction`: spawned Pod の instruction ファイル参照（省略時は `$insomnia/default`）
- `task`: spawned Pod への最初のメッセージ（spawn 後に即座に run される）
- `scope`: 譲渡する scope 定義（allow / deny ルール）。**spawner の現在の effective scope のサブセットでなければならない**。バリデーションで超過分は拒否

出力:
- spawned Pod の識別子（`pod_id`）と接続状態

内部動作:
- spawner の effective scope から、譲渡する scope 領域を deny に追加（spawner 側の scope を縮小）
- PodFactory のカスケード（user manifest + project manifest）に加え、spawner からの overlay（譲渡 scope + instruction + provider 等）を重ねて spawned Pod の PodManifest を構築
- 独立プロセスとして Pod を起動、socket 確立
- `task` を `Method::Run` で送信

### `SendToPod`

spawned Pod にメッセージを送る。

入力:
- `pod_id`: 対象の Pod
- `message`: 送信するテキスト

出力:
- 送信確認

### `ReadPodOutput`

spawned Pod の最新の出力を読む。

入力:
- `pod_id`: 対象の Pod
- `since`: 前回読んだ時点からの差分のみ取得するオプション（省略時は全出力）

出力:
- 対象 Pod の assistant 応答テキスト（最新ターン or 差分）
- 現在の状態（`running` / `idle` / `stopped`）

### `StopPod`

spawned Pod を終了させ、譲渡した scope を回収する。

入力:
- `pod_id`: 対象の Pod

出力:
- 終了確認
- 回収された scope の要約

内部動作:
- 対象 Pod に graceful shutdown を要求
- 対象 Pod が保持していた scope のうち、さらに下流に分譲されていない分を spawner に返却
- spawner の deny リストから返却分を解除

### `ListPods`

spawner が spawn した Pod の一覧とそれぞれの状態を返す。

入力: なし

出力:
- spawned Pod の `pod_id`、`name`、`status`（running / idle / stopped）、譲渡中の scope 要約、最終応答の要約

## 非同期通知

spawned Pod が出力を生成したとき、spawner の LLM にそれを伝える仕組みが必要。ただし LLM は「ターン」の単位で動くため、イベントストリームをリアルタイムに割り込ませるのは不自然。

### 方針: ターン間フックで集約通知

- spawner のターンが終了した後、次のターンの開始前に**フック**が走り、spawned Pod から届いているイベントを集約する
- 集約された通知は次のターンの**先頭に system message として注入**される（例: `[pod "researcher"] completed: found 3 relevant files`）
- 通知が無い場合は何もしない

### 通知の粒度

- **ターン完了通知**: spawned Pod の1ターンが終わったとき
- **エラー通知**: spawned Pod でエラーが発生したとき
- **終了通知**: spawned Pod が停止したとき（scope 返却が発生する）

ツール出力の内容自体は `ReadPodOutput` で明示的に取りに行くモデル。通知は「何か変化があった」というシグナルのみで、内容全体は含まない（spawner のコンテキストを圧迫しない）。

## Scope の分譲と回収の詳細

### 分譲時

1. spawner が `SpawnPod` で `scope` を指定
2. システムが検証: 指定された scope が spawner の **現在の effective scope のサブセット**であること
3. 検証通過後、spawner の effective scope から譲渡分を deny として差し引く
4. spawned Pod は譲渡された scope を自身の allow として持って起動

### 回収時（StopPod または spawned Pod の自発的終了）

1. 終了する Pod が保持している effective scope を確認
2. そのうち、さらに下流に分譲中の scope は除外（下流 Pod が生きている間は返却されない）
3. 残りの scope を spawner の deny から解除し、spawner の effective scope に復帰

### 分譲チェーンのケース

Pod A が Pod B に `/src` を分譲 → Pod B が Pod D に `/src/core` を分譲:

- A の scope: `/src` は deny
- B の scope: `/src` を持つが `/src/core` は deny（D に分譲済み）
- D の scope: `/src/core`

B が終了すると:
- `/src` のうち D に分譲中の `/src/core` は返却されない（D が生きている）
- `/src` から `/src/core` を除いた部分が A に返却される
- A の scope: `/src` の `/src/core` 以外が復帰。`/src/core` は引き続き deny（D が保持中）
- D が終了すると `/src/core` が A に返却される

### runtime scope state

- Pod の scope は manifest の static 定義だけでは決まらない。分譲と回収による**動的な state** が加わる
- この state を管理するレイヤーが必要（Pod 内のメモリ状態 + 永続化の選択肢）
- spawner がクラッシュして復帰した場合、分譲中の scope を復元する必要がある → scope ledger の永続化が事実上必須

## Pod の設定

### instruction

- spawned Pod の `instruction` は `SpawnPod` の引数で明示指定
- 省略時は `$insomnia/default`
- spawner の instruction を引き継ぐケースはまれ（分業目的の spawn なので、spawned Pod には固有の役割がある）

### provider

- API キーと provider 設定は通常ユーザー manifest / プロジェクト manifest 由来なので、spawned Pod は PodFactory 経由で同じカスケードから取得する
- spawned Pod に異なるモデルを使わせたい場合は overlay で上書きする（例: 調査用 Pod には小さいモデル、コード生成にはフルモデル）

## 人間向けの監視

### 各 Pod は独立して観測可能

- すべての Pod は通常の socket サーバーを持つ。人間は GUI / TUI で任意の Pod に接続して会話を閲覧・介入できる
- Pod 間に主従関係が無いので、人間はどの Pod にも同格にアクセスできる

### Pod ネットワークの可視化

- GUI / TUI が Pod の一覧を表示（spawn 関係をグラフまたはリストで表現）
- 各ノードに status（running / idle / stopped）、保持中の scope 要約、最終ターンの要約を表示
- ノードをクリック/選択すると、その Pod の会話ビューに切り替わる

### 介入

- 人間は任意の Pod に直接メッセージを送れる（通常のクライアントとして接続）
- 人間が Pod を直接 stop できる（scope は spawner に返却される）
- spawner の LLM は spawned Pod に人間が介入したことを（次の通知で）知る

## Pod の発見と登録

プロセスが独立しているため、起動中の Pod を**発見**する仕組みが要る。

候補:
- **A. runtime_dir convention**: 各 Pod が socket path を `runtime_dir/<pod_name>.sock` に作る。ディレクトリを列挙すれば一覧が取れる（現行の仕組みの延長）
- **B. 明示的なレジストリ**: Pod 起動時に共有レジストリ（ファイルまたは IPC）に登録。終了時に削除
- **C. daemon**: 軽量な daemon プロセスがレジストリ役を担う（`crates/daemon` は現在空）

現時点では A が最もシンプル。daemon が必要になるのは「リモート Pod」や「Pod の自動再起動」が必要になってから。

## 設計で決めること

- **Pod の起動方式**: 独立プロセスとして `pod` バイナリを起動する形で確定。起動方法の詳細（fork? 直接 exec? nix-shell 経由?）
- **scope ledger の永続化方式**: ファイルベース / session-store / 別の永続ストア
- **scope の分譲粒度**: パス単位で分譲するか、permission レベル（read / write）でも分譲できるか（例: write だけ譲って read は共有する等）
- **通知の注入方式**: system message として注入するか、専用の `Event` としてターン開始前に Worker に流すか
- **通知のバッファリング**: spawner の1ターンが数分かかる場合、その間のイベントをどこに溜めるか
- **Pod の発見**: A (runtime_dir) / B (レジストリファイル) / C (daemon)
- **リソース制限**: 最大 spawned Pod 数、ネスト深さの上限
- **ツール名**: `SpawnPod` / `SendToPod` / `ReadPodOutput` / `StopPod` / `ListPods` の命名はそのままで良いか
- **Pod ネットワークの可視化**: GUI / TUI どちらに先行実装するか
- **spawner 復帰時の再接続**: scope ledger を読んで spawned Pod のソケットに再接続する手順
- **分譲チェーンの depth limit**: 再帰的 spawn の深さ上限を設けるか

## 完了条件

- Pod の LLM が `SpawnPod` ツールで別 Pod を起動でき、spawned Pod が独立プロセスとして動作する
- spawn 時に scope が分譲され、spawner の effective scope が縮小されることを検証できる
- `SendToPod` で spawned Pod にメッセージを送り、`ReadPodOutput` で応答を読み取れる
- `StopPod` で spawned Pod を graceful に停止でき、scope が spawner に返却される
- `ListPods` で spawned Pod の状態と scope 要約を一覧できる
- spawned Pod のターン完了・エラー・停止が spawner に非同期通知され、次のターン開始時に LLM に伝わる
- spawner が停止しても spawned Pod が続行し、spawner 復帰時に再接続できる
- 人間が GUI または TUI で Pod ネットワークを閲覧でき、任意の Pod に接続して会話を見られる
- spawned Pod がさらに別の Pod を spawn する再帰的なネットワークが動作する
- scope 分譲チェーンの返却（中間ノードの終了時に部分返却）が正しく動作する

## 他チケットとの関係

- **tickets/scope-exclusion.md**: scope 分譲モデルにより**本チケットに吸収される**。write 排他制御は分譲時に構造的に保証されるため、別の排他制御メカニズムは不要。scope-exclusion.md は本チケットの scope 設計がカバーする
- **tickets/protocol-design.md**: Protocol に Pod 間通知のイベント（spawned Pod の状態変化通知など）を追加する必要があるかもしれない。ただし各 Pod は通常の socket サーバーなので、protocol 拡張なしで「複数 socket に繋ぐクライアント」として実装できる可能性もある
- **tickets/native-gui-mvp.md**: Pod ネットワーク可視化は GUI 側の拡張。MVP scope には含まれていないが、本チケットの監視要件は GUI の次フェーズに自然に接続する
- **tickets/tui-pod-spawn-ui.md**: TUI からの Pod spawn UI。本チケットは LLM からの spawn だが、spawn のインフラ（PodFactory + プロセス起動）は共通
- **tickets/tui-pod-shutdown.md**: Pod ネットワークの shutdown 戦略にも影響。Pod を停止したとき scope がどこに返るか
- **tickets/permission-extension-point.md**: spawned Pod のツール実行パーミッションを spawner が制御する可能性

## 範囲外

- **Pod 間の直接メッセージパッシング**: Pod 同士が spawner を介さずに直接通信する仕組み。すべてのやり取りは spawner（オーケストレーター）を介す
- **共有メモリ / 共有状態**: Pod 間のデータ共有はツール経由のテキストメッセージのみ。ファイルシステムを介した暗黙の共有は scope が許す範囲で起きうるが、それは Pod の既存動作の延長であり、分譲により write 衝突は構造的に排除される
- **自動スケーリング / 負荷分散**: Pod の spawn 先は常にローカルマシン。リモート実行やクラウド分散は扱わない
- **課金・トークン予算管理**: spawned Pod が消費するトークンの予算制御や spawner への請求集約は扱わない
- **人間がネットワークの構造を変更する操作**（Pod の spawn 元替え、ネットワークの組み替えなど）
- **daemon の導入**: Pod の発見は runtime_dir convention または軽量レジストリで対応。daemon は必要が明確になってから
