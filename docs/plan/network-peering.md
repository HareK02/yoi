# ネットワーク越しの Pod 協働（将来設計ノート）

本ドキュメントは将来の設計方針を記録するものであり、実装チケットではない。

---

## 動機

ローカルの workspace が Pod 間の発見・通知・scope 会計を提供するが、
これは 1 台のマシンに閉じている。複数マシンにまたがるプロジェクト
（モノレポの異なる部分を別マシンで触る、CI マシンの Pod と開発マシンの
Pod が連携する等）では、マシン間のメッセージングが必要になる。

## 設計原則

- **中央サーバーを作らない**。各マシンが主権を持ち、P2P でやり取りする
- **ローカル workspace を歪めない**。ネットワーク層は workspace の上に
  乗る「他の workspace への郵便」であり、workspace 自体を分散化しない
- **ファイルシステムはマシンローカル**。cross-host の scope 分譲は
  行わない。協働はメッセージベースのタスク委譲で完結する
- **SSH を transport に使う**。開発者の手元に既にあるインフラで、
  鍵管理・暗号化・認証が追加投資なしで手に入る

## アドレッシング

Pod のネットワークアドレスは `insomnia.pod-name@host` の形式を**論理的な
宛先表記**として使う。実際の SSH 接続がこの文字列そのままで行えるかは
transport 方式に依存する（後述）。

- `insomnia` = SSH ユーザー名（固定）
- `host` = 相手マシンのホスト名 or IP
- `pod-name` = 送信先の Pod 名（相手マシン上のローカル workspace 内で一意）

推奨構文は **`insomnia@host:pod-name`**（git 方式）。
詳細は後述の「アドレッシング構文」を参照。

## アドレッシング構文

論理的な宛先表記として **`insomnia@host:pod-name`** を推奨する。
git の `git@github.com:user/repo` と同じ構文で:

- SSH ユーザーは `insomnia` 固定（動的ユーザー名が不要）
- `:` 以降がルーティング情報（Pod 名）
- クライアント側が `insomnia@host:pod-name` をパースし、
  `ssh insomnia@host "insomnia-route pod-name"` に変換する

git がこの方式で `git-upload-pack user/repo` にルーティングしている
のと同じ仕組み。OS レベルの設定（NSS モジュール等）が一切不要で、
ユーザー 1 つ + ForceCommand（またはシェルスクリプト）だけで動く。

## SSH transport の選択肢

### A. 単一ユーザー + コマンド引数

```
ssh insomnia@host send pod-name "message"
```

- 相手マシンにシステムユーザー `insomnia` を 1 つ作る
- `authorized_keys` に接続元 Pod の公開鍵を登録
- ForceCommand または shell スクリプトが第一引数 (`send`) と
  第二引数 (`pod-name`) を解釈してローカル workspace のレジストリから
  Pod の socket を引き、メッセージをルーティング
- **導入コスト最低**。ユーザー 1 つ + スクリプト 1 つで動く
- 宛先が引数に入るので `insomnia.pod-name@host` の見た目にはならない

### B. 鍵ベースルーティング（gitolite 方式）

```
ssh insomnia@host    # 使った鍵でどの Pod 宛か判別
```

- `~insomnia/.ssh/authorized_keys` に Pod ごとのエントリ:
  ```
  command="insomnia-route pod-a",no-port-forwarding,... ssh-ed25519 AAAA... pod-a@remote
  command="insomnia-route pod-b",no-port-forwarding,... ssh-ed25519 AAAA... pod-b@remote
  ```
- SSH 接続時に使われた鍵が `command=` で指定されたルーティング先を決定
- gitolite / Gitea / Gogs で実証済みのパターン
- 接続元は `ssh insomnia@host` だけ。**鍵が宛先を決める**
- クライアント側 SSH config で alias を作れば見た目を整えられる:
  ```
  Host pod-a.host-b
    HostName host-b
    User insomnia
    IdentityFile ~/.config/insomnia/keys/pod-a
  ```
- 鍵の登録が相互に必要（Pod A が Pod B に送るなら、B のマシンの
  authorized_keys に A の公開鍵 + route 先を登録）

### C. 動的ユーザー名

```
ssh insomnia.pod-name@host
```

- `insomnia.pod-name` を OS レベルで有効なユーザー名として解決する:
  - NSS (Name Service Switch) モジュールを書くか `libnss-extrausers` を利用
  - PAM モジュールで認証をフック
  - `sshd_config` で `Match User insomnia.*` → `ForceCommand` でルーティング
- **最も直感的なアドレッシング**だが OS レベルの設定が必要
- insomnia をインストールするだけでは動かない（管理者権限での設定が要る）
- コンテナ環境ではやりやすい（ユーザー管理を自由にできる）

### 推奨

**MVP は A（単一ユーザー + コマンド引数）** から始める。設定が最小で
動くものが作れる。将来 B（鍵ベースルーティング）に進化させると
セキュリティが強化される（Pod 単位での接続制御が可能）。
C は UX は最高だが導入コストが高く、必要が明確になるまで見送る。

## ファイルシステムの境界

Pod はローカルのファイルシステムだけを操作する。ネットワーク越しの
Pod 協働では:

- **scope はマシンローカルに閉じる**。host_a の `/src` と host_b の
  `/src` は別物。cross-host の scope 分譲は行わない
- **協働はタスク委譲**。「このリポジトリでこの変更をして結果を教えて」
  というメッセージで、相手の Pod がローカルに実行する
- **sshfs / NFS 等のネットワークファイルシステムは使わない**。
  透過的なリモートファイルアクセスは壊れやすく、scope モデルと相性が悪い

## ローカル workspace との関係

| 層 | スコープ | 役割 |
|---|---|---|
| Workspace | 1 台のマシン | Pod 発見 / scope 会計 / 通知バス |
| Network peers | マシン間 | メッセージング / タスク委譲 / 結果通知 |

- Network peers は workspace の**上に**乗る。workspace を置き換えない
- ローカルの Pod 間通信は引き続き workspace の Unix socket バスを使う
- リモートの Pod への通信だけ SSH transport を経由する
- Pod から見ると「ローカル peer に送る」と「リモート peer に送る」は
  同じ API で、transport 層が切り替わるだけ（が理想）

## broadcast の扱い

ローカル workspace は Unix socket で 1 本の bus を持てたが、
ネットワーク越しには共有 bus が無い。

- 各マシン（または各 Pod）が **known-peers リスト** を持つ
- broadcast = known-peers を iterate して個別送信
- 規模が数十 Pod なら十分実用的
- 将来的に gossip protocol で peer 発見を自動化できるが、
  MVP では手動登録（`insomnia peer add pod-a@host-b`）で十分

## 受信側のルーティング

SSH 接続を受けた側が、宛先 Pod のローカル socket にルーティングする
仕組みが必要。

```
[SSH 接続] → insomnia-route <pod-name>
                ↓
            workspace registry を参照
                ↓
            /run/insomnia/.../pod-name.sock に転送
                ↓
            Pod が受信・処理
```

`insomnia-route` は:
1. workspace のレジストリを読んで pod-name の socket path を引く
2. socket に接続してメッセージを中継
3. 応答を SSH 接続に返す

workspace が複数ある場合のルーティング（どの workspace の
pod-name か）は追加の設計が必要。Pod 名がマシン上で globally
unique であれば workspace を指定しなくて済む。

## Daemon-less リモート Pod 生成（SSH-only モデル）

リモートホスト上の Pod 生成は **daemon 無しで SSH だけで成立する**。
remote 側に必要なのは `insomnia` バイナリと SSH アクセスのみ。

### 前提

- insomnia は環境再現（git clone, コンテナ構築等）を自身の責務としない。
  作業対象のファイルがリモートに既にあるか、ユーザーが任意の手段で
  用意する前提（git clone, rsync, 手動配置、CI の checkout 等）
- insomnia が転送するのは**セッション（会話履歴）と manifest overlay**
  だけ。コードベースの同期は外部に委ねる
- コンテナ内で動かすか bare metal で動かすかも insomnia は問わない。
  `insomnia` バイナリが動くホストの fs 上で活動する主体がある、
  それだけが前提

### フロー

```
host_a (spawner)                         host_b (remote)
  Pod A                                   (pod binary + ssh のみ)
    │
    ├── ssh: session データを転送 ────────→ ファイル書き込み
    ├── ssh: profile / one-file manifest 入力を転送 ─→ 必要ならファイル書き込み
    ├── ssh: `insomnia pod --profile ... &` ───────→ Pod プロセス起動、socket 作成
    ├── ssh -L: socket を tunnel ─────────→ Pod B の unix socket
    │
    └── localhost:tunnel に接続 ──────────→ Method::Run / Event stream
        （以降はローカル Pod と同じ protocol）
```

### コマンドイメージ

```bash
# 1. session + profile/manifest input を転送
ssh insomnia@host-b "mkdir -p ~/workspaces/task-123/store"
tar cz session/ | ssh insomnia@host-b "tar xz -C ~/workspaces/task-123/store"
scp profile.lua insomnia@host-b:~/workspaces/task-123/profile.lua

# 2. Pod を起動（detach）
ssh insomnia@host-b "insomnia pod --store ~/workspaces/task-123/store \
    --profile ~/workspaces/task-123/profile.lua &"

# 3. socket を tunnel で引っ張る
ssh -L /tmp/pod-b.sock:/run/insomnia/task-123/pod.sock insomnia@host-b

# 4. あとは /tmp/pod-b.sock にローカルと同じ protocol で繋ぐ
```

spawner の `SpawnPod` ツールがこの一連を内部で実行する。LLM から
見たら「ツールを呼んだら Pod ができた」だけ。

### なぜこれで足りるか

- **protocol は変わらない**: SSH tunnel の向こう側は普通の Pod socket。
  ローカルの Pod と同じ `Method` / `Event` でやり取りする
- **scope は host ごとに独立**: cross-host の scope 分譲はそもそも
  成立しないので、workspace の scope 会計は remote には関係しない
- **通知**: SSH tunnel が繋がっている限り `Event` stream がそのまま
  流れる。tunnel が切れたら再接続する
- **環境構築は insomnia の責務外**: git clone するか rsync するかは
  Pod の instruction で指示するか、事前に用意されている前提

### daemon が必要になるケース

SSH-only モデルの制約が、daemon 導入の動機になる:

- **Pod 一覧の取得**: remote の runtime_dir を SSH 越しに `ls` する
  必要がある（daemon がいればレジストリ API で済む）
- **Pod の生存監視**: tunnel が切れたら再接続するまで状態不明
  （daemon がいれば health check を引き受ける）
- **複数の spawner が同じ remote Pod に繋ぐ**: tunnel の共有が面倒
  （daemon がいれば multiplexing できる）
- **workspace サービス（registry / 通知バス）の remote 提供**:
  SSH-only モデルではリモート側に workspace サービスが無い

これらは **MVP では問題にならず**、daemon は「便利にしたくなった
ときの upgrade path」として位置づける。

### リモート側のディレクトリ構成

```
/home/insomnia/                    ← insomnia システムユーザーの home
├── workspaces/
│   ├── <task-or-project-id>/      ← workspace ごとのルート
│   │   ├── repo/                  ← ユーザーが用意した作業ファイル群
│   │   └── store/                 ← session store（spawner から転送）
│   └── ...
└── .ssh/
    └── authorized_keys            ← 接続元 Pod の公開鍵
```

- `insomnia` システムユーザーが SSH 接続先 + ファイル所有者
- `repo/` 配下の準備は insomnia の責務外（git clone, rsync 等は
  ユーザーや instruction が指示）
- `store/` は spawner がセッションデータを書き込む場所

## セキュリティの考慮

- SSH の鍵認証がベースライン。パスワード認証は使わない
- Pod 単位の鍵ペアにより、接続制御の粒度を Pod レベルにできる（B 方式）
- `authorized_keys` の `command=` で実行可能な操作を制限できる
  （`no-port-forwarding`, `no-pty` 等）
- 将来的に Pod 間の trust relationship を定義する仕組みが要るが、
  それは本ドキュメントの範囲外

## 未解決の論点

- SSH transport の具体的な message protocol（JSON-RPC? 独自? protocol crate の拡張?）
- 非同期メッセージの扱い（相手が offline のとき queue するか、fail するか）
- peer 登録の自動化（workspace 内の Pod が自動で peer list を共有する等）
- workspace が複数ある環境での pod-name 解決
- Pod の migration（あるマシンから別のマシンへ Pod を移す）の可能性と scope の扱い
