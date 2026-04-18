# Pod オーケストレーション: LLM によるマルチエージェント分業

## 背景

Insomnia の差別化の中核は「複数の Pod が独立プロセスとして並行動作し、LLM が自律的に分業・統括できる」点にある。シングルエージェントの LLM リグが1つのコンテキストで全タスクを処理するのに対し、Insomnia は**タスクを別 Pod に委譲し、結果を集約してフィードバックする**オーケストレーションを LLM 自身に行わせることで、コンテキスト長・専門性・並列性の壁を突破する。

現状の Pod は単独で動くことしかできず、Pod 間の連携は外部の人間が仲介するしかない。LLM が「この調査は別の Pod に任せて、結果が返ってきたら本筋に統合する」という判断を**ツールとして**実行できる仕組みが無い。

## ゴール

Pod の LLM が、ツール呼び出しを通じて別 Pod のライフサイクル全体（生成・指示・結果読み取り・終了）を制御でき、spawned Pod の進捗を非同期に受け取れる基盤を設計・実装する。あわせて、人間がこの Pod ネットワークを監視・介入できる観測経路を用意する。

## コンセプト: 独立したピアとしての Pod

```
Human (GUI / TUI) ─── Pod A (orchestrator)
                          ├── callback addr ──→ Pod B (researcher)
                          ├── callback addr ──→ Pod C (coder)
                          │                        └── callback addr ──→ Pod D (reviewer)
                          └── callback addr ──→ Pod E (tester)
```

### プロセス独立

- spawn された Pod は **完全に独立したプロセス** として動作する。OS レベルの親子関係（subprocess）は持たない
- spawner が落ちても、spawned Pod は**続行**する
- すべての Pod は同格。「誰が spawn したか」は Pod の runtime 属性であり、プロセスの従属関係ではない

### Pod の発見と知識

- Pod は**自分が spawn した相手**と**親から明示的に教えてもらった相手**しか知らない
- runtime_dir のスキャンや共有レジストリによる探索は行わない（セキュリティリスク）
- 親抜きで会話すべき Pod があるなら、それは親が明示的に紹介する
- Pod が知っている Pod のリスト = spawn 記録 + 紹介された相手の記録

### 接続モデル: 常時接続なし

- Pod 間の接続は**すべて一時的**。常時接続は張らない
- 操作（メッセージ送信・出力読み取り・停止）は**都度接続の request-response**:
  - ローカル: unix socket に接続 → request → response → 切断
  - リモート: SSH 経由で接続 → request → response → 切断
- 非同期通知は**コールバック**方式: spawned Pod がイベント発生時に spawner のアドレスに一発接続して通知し、即切断（webhook と同じモデル）
- 接続は常に**送信側が開始**する

## Scope の分譲と排他制御

### 原則

- Pod A が Pod B を spawn するとき、A は自身の scope の一部を B に**譲渡**する
- 譲渡した scope 領域は A の effective scope から deny される（A は自分が譲った部分にアクセスできなくなる）
- **write scope の排他制御が保証**される：同一パスに対して write 権限を持つ Pod は常に高々1つ
- read は衝突しない（複数 Pod が同じパスを同時に read 可能）

### Scope lock file

scope の割り当てをマシン上の**単一の lock file** で一元管理する。spawn 系譜を持たない Pod 同士（人間が独立に起動した Pod 等）でも write 衝突を検出できる。

置き場: `$XDG_RUNTIME_DIR/insomnia/scope.lock`

内容:
```json
{
  "allocations": [
    {
      "pod_id": "abc123",
      "pid": 12345,
      "socket": "/run/insomnia/.../pod-a.sock",
      "scope_allow": ["/project/src:write:recursive"],
      "delegated_from": null
    },
    {
      "pod_id": "def456",
      "pid": 12346,
      "socket": "/run/insomnia/.../pod-b.sock",
      "scope_allow": ["/project/src/core:write:recursive"],
      "delegated_from": "abc123"
    }
  ]
}
```

操作は `flock(2)` による advisory lock で排他アクセスする:

| タイミング | 動作 |
|---|---|
| **Pod 起動** | lock → stale 検出（PID 死活）→ 自動回収 → write 衝突チェック → 自分の scope を登録 → unlock |
| **scope 分譲 (SpawnPod)** | lock → spawner の allocation に deny 追記 → 新 Pod の allocation を追加（`delegated_from` に spawner）→ unlock |
| **Pod 正常終了** | lock → 自分の allocation を削除 → `delegated_from` が自分の子が残っていなければ親の deny を解除 → unlock |
| **stale 検出** | `kill(pid, 0)` で生存確認。死んでいたら allocation を削除し、scope を `delegated_from` の親に返却 |

### stale の自動回収

Pod がクラッシュ（正常終了せず PID が消えた）した場合、lock file にエントリが残る。**次に lock file を開いた Pod が stale を検出し自動回収する**:

- Pod B (pid=200, dead): `/project/src` write, delegated_from=A
- Pod D (pid=300, alive): `/project/src/core` write, delegated_from=B

→ B が死んでいる:
1. B の scope `/project/src` のうち D が持つ `/project/src/core` を除いた部分を A に返却
2. B のエントリを削除
3. D の `delegated_from` を A に付け替え

能動的なコールバックが届かなくても、いつか誰かが lock file を開けば回収が走る。

### Effective scope の導出

Pod の effective scope は lock file から導出される:

```
effective_scope = 自分の allocation - Σ(delegated_from が自分を指す子の allocation)
```

Pod は lock file を読むことで自分の effective scope を知れる。ただし常時読むとパフォーマンスに影響するため、キャッシュして scope 変動のコールバック通知で更新するのが実用的。

### 返却

- Pod B が終了すると、scope は `delegated_from` が指す spawner (A) に自動返却される
- 返却先は常に `delegated_from`。プールへの返却や他者への再分配はない

### 又貸し

- Pod B が `/src/core` を Pod D に又貸しする場合、lock file 上で:
  - B の allocation から `/src/core` を除外
  - D の allocation を追加（`delegated_from=B`）
- B は spawner (A) にコールバックで通知:「`/src/core` を D に委譲した」
  - A は D の存在と D の socket address をこの通知で知る（= 親からの紹介）
- B が終了したとき: lock file の stale 回収で D の `delegated_from` が A に付け替わり、D が保持しない残りの scope が A に戻る

### 観測可能性

- `insomnia scope list` 的なコマンドで lock file を読めば、マシン上の全 Pod の scope 割り当てを一覧できる
- 人間が「今どの Pod がどこを write 占有しているか」を確認する手段になる
- 衝突で Pod 起動が拒否されたとき、競合相手の pod_id を lock file から取得してエラーメッセージに含める

## LLM に公開するツール群

Pod の LLM が使えるツールとして以下を設計する。すべて**都度接続の request-response** で動作する。

### `SpawnPod`

新しい Pod を起動し、scope の一部を譲渡する。

入力:
- `name`: spawned Pod の識別名
- `instruction`: instruction ファイル参照（省略時は `$insomnia/default`）
- `task`: 最初のメッセージ（spawn 後に即座に run される）
- `scope`: 譲渡する scope 定義。spawner の effective scope のサブセットでなければならない
- `address`: (自動注入) spawner の callback address

出力:
- spawned Pod の `pod_id` と接続先 address

内部動作:
- scope lock file を flock → write 衝突チェック → spawner の allocation に deny 追記 + 新 Pod の allocation を登録 → unlock
- PodFactory のカスケードに spawner からの overlay を重ねて PodManifest を構築
- 独立プロセスとして Pod を起動
- spawner の callback address を spawned Pod に渡す
- `task` を `Method::Run` で送信

### `SendToPod`

既知の Pod にメッセージを送る（都度接続）。

入力:
- `pod_id`: 対象の Pod
- `message`: 送信するテキスト

出力:
- 送信確認

### `ReadPodOutput`

既知の Pod の最新の出力を読む（都度接続）。

入力:
- `pod_id`: 対象の Pod
- `since`: 前回読んだ時点からの差分のみ取得するオプション

出力:
- 対象 Pod の assistant 応答テキスト（最新ターン or 差分）
- 現在の状態（`running` / `idle` / `stopped`）

### `StopPod`

既知の Pod を終了させ、譲渡した scope を回収する（都度接続）。

入力:
- `pod_id`: 対象の Pod

出力:
- 終了確認
- 回収された scope の要約

### `ListPods`

自分が知っている Pod の一覧と状態を返す。

入力: なし

出力:
- 各 Pod の `pod_id`、`name`、`status`（running / idle / stopped）、譲渡中の scope 要約、最終応答の要約

内部動作:
- spawn 記録を元にリストを構築
- 各 Pod に都度接続して health check（接続できなければ stopped 扱い）
- Hook で定期的に自動実行することも可能

## 非同期通知: コールバック方式

### 仕組み

- spawn 時に spawner が**自分の callback address** を spawned Pod に渡す
  - ローカル: unix socket path
  - リモート: `insomnia@host:pod-name` 形式の SSH address
- spawned Pod がイベント発生時に、spawner の callback address に**一発接続して通知を送り、即切断**する
- spawner が落ちていたら callback が失敗するだけ（spawned Pod は続行する）

### 通知の種類

- **ターン完了**: spawned Pod の1ターンが終わった（spawner は `ReadPodOutput` で内容を取りに行く）
- **scope 又貸し**: spawned Pod が自身の scope の一部をさらに別の Pod に委譲した（spawner の scope 記録を更新する材料）
- **エラー**: spawned Pod でエラーが発生
- **終了**: spawned Pod が停止した（scope 返却のトリガー）

### 通知は「シグナル」のみ

通知にはイベントの種類と最小限のメタデータ（pod_id、scope 変動等）のみを含める。応答テキスト全体のような大きなデータは含まない。spawner が内容を知りたければ `ReadPodOutput` で取りに行く。

### 通知の LLM への伝達

- spawner が受け取ったコールバック通知はバッファに溜められる
- spawner の次のターン開始前に、Hook が溜まった通知を集約してターンの先頭に system message として注入する
- 通知が無い場合は何もしない

### ポーリングでも代替可能

コールバックが届かなくても、spawner は `ListPods` のポーリングで状態を拾える。コールバックは「早く気付くための最適化」であって唯一の手段ではない。

## Pod の設定

### instruction

- spawned Pod の `instruction` は `SpawnPod` の引数で明示指定
- 省略時は `$insomnia/default`
- spawner の instruction を引き継ぐケースはまれ（分業目的の spawn なので固有の役割がある）

### provider

- API キーと provider 設定は PodFactory のカスケード（user / project manifest）から取得
- spawned Pod に異なるモデルを使わせたい場合は overlay で上書き

### Pod 起動コマンド

- 環境変数またはユーザー設定で上書き可能
- デフォルトは `pod`（PATH 上の `pod` バイナリ）
- リモートの場合は SSH 越しに実行

## 人間向けの監視

### 各 Pod に個別接続

- すべての Pod は通常の socket サーバーを持つ。人間は GUI / TUI で Pod に接続して会話を閲覧・介入できる
- ただし、Pod の存在を知る手段は**spawn 記録（+ 紹介）のみ**。人間もオーケストレーター Pod 経由か、事前に Pod の address を知っている必要がある

### Pod ネットワークの可視化

- GUI / TUI がオーケストレーター Pod の spawn 記録を読んで Pod リストを表示
- 各エントリに status、scope 要約、最終応答の要約を表示
- エントリを選択すると、その Pod に接続して会話ビューに切り替わる

### 介入

- 人間は既知の Pod に直接メッセージを送れる（通常のクライアントとして都度接続）
- 人間が Pod を直接 stop できる（scope は spawner に返却される）

## 設計で決めること

- **callback の protocol**: 通知メッセージの具体的なフォーマット。既存の protocol crate (`Event` enum) を拡張するか、別の軽量フォーマットにするか
- **scope の分譲粒度**: パス単位で分譲するか、permission レベル（write だけ渡して read は共有等）でも制御できるか
- **通知のバッファリング**: spawner のターン実行中に溜まるコールバック通知をどこに保持するか
- **リソース制限**: 最大 spawned Pod 数、ネスト深さの上限
- **ツール名**: `SpawnPod` / `SendToPod` / `ReadPodOutput` / `StopPod` / `ListPods` の命名
- **Pod ネットワークの可視化**: GUI / TUI どちらに先行実装するか
- **spawner 復帰時の再接続**: spawn 記録を読んで spawned Pod に再接続する手順。callback address の再登録
- **リモート spawn**: SSH-only モデルの詳細（`docs/network-peering.md` を参照）

## 完了条件

- Pod の LLM が `SpawnPod` ツールで別 Pod を起動でき、spawned Pod が独立プロセスとして動作する
- spawn 時に scope lock file に allocation が記録され、spawner の effective scope が���小される
- 同一パスへの write 衝突が lock file で検出され、Pod 起動が拒否される（競合相手の pod_id がエラーに含まれる）
- stale エントリ（PID が死亡）が自動回収され、scope が `delegated_from` の親に戻る
- `SendToPod` / `ReadPodOutput` が都度接続の request-response で動作する
- `StopPod` で spawned Pod を graceful に停止でき、lock file から allocation が削除されて scope が返却される
- `ListPods` で既知の Pod の状態を一覧でき、health check が機能する
- spawned Pod のターン完了・エラー・停止がコールバックで spawner に通知され、`Method::Notify` 経由で LLM に伝わる
- spawner が停止しても spawned Pod が続行する
- 又貸しが lock file に記録され、コールバック通知により spawner が孫 Pod の存在と scope を把握できる
- `insomnia scope list` 相当のコマンドで lock file を読み、マシン上の全 Pod の scope 割り当てを一覧できる
- 人間が GUI または TUI で Pod リストを閲覧でき、任意の既知 Pod に接続して会話を見られる

## 他チケットとの関係

- **tickets/method-notify.md**: コールバック通知を親 Pod に注入する仕組み。オーケストレーションの非同期通知に必須
- **tickets/protocol-design.md**: コールバック通知の protocol 拡張が必要になりうる
- **tickets/native-gui-mvp.md**: Pod リスト可視化は GUI 側の拡張
- **tickets/tui-pod-spawn-ui.md**: TUI からの Pod spawn UI。spawn のインフラは共通
- **tickets/permission-extension-point.md**: spawned Pod のツール実行パーミッションを spawner が制御��る可能性
- **scope-exclusion (削除済み)**: scope lock file による write 排他で吸収された

## 範囲外

- **Pod 間の直接メッセージパッシング**: すべてのやり取りは spawner を介するか、spawner が明示的に紹介した相手とのみ行う
- **常時接続**: Pod 間の接続はすべて一時的（都度接続 or コールバック一発）
- **runtime_dir のスキャンによる Pod 探索**: セキュリティリスクのため行わない。Pod の存在は spawn 記録 + 明示的な紹介 + lock file のみで把握
- **自動スケーリング / 負荷分散 / リモート実行**: ローカルの単一マシン上での動作に集中。リモート spawn は `docs/network-peering.md` を参照
- **課金・トークン予算管理**
- **環境再現（git clone, コンテナ構築等）**: insomnia の責務外
