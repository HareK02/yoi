# Pod の上流イベント報告 (子 → 親)

## 背景

spawned Pod（子）のライフサイクルに親 Pod が反応する仕組みが必要。反応には 2 系統ある：

1. **system 処理**: `spawn_pods.json` 更新、`scope.lock` 整合、孫 Pod の把握
2. **LLM 通知**: 親 LLM に「子でこれが起きた、次どうする？」を考えさせる

これを実現する通信 primitive を分離して定義する。既存の `Method::Notify`（LLM 向け自由テキスト注入）と混ぜない。

## 依存

- `tickets/spawn-pod-tool.md`: 済。callback address の受け渡しと spawn 記録
- `tickets/scope-lock.md` 済に含まれる: allocation の delegate/reparent は既存
- `Method::PodEvent` の追加は `protocol` crate の拡張

## 設計

### 新しい primitive

```rust
pub enum Method {
    Run { input: String },
    Notify { message: String },           // 既存: 人間・tool → LLM 文脈、副作用なし
    PodEvent(PodEvent),                   // 新: 子 → 親、typed なライフサイクル報告
    Resume,
    Cancel,
    Shutdown,
    GetHistory,
}

pub enum PodEvent {
    /// 子が1ターン終えて IDLE になった
    TurnEnded { pod_name: String },

    /// 子でエラーが発生した（ターンは続行されるとは限らない）
    Errored { pod_name: String, message: String },

    /// 子が停止した
    ShutDown { pod_name: String },

    /// 子が孫 Pod に scope を又貸しした
    ScopeSubDelegated {
        parent_pod: String,               // 又貸し元（= 子自身）
        sub_pod: String,                  // 孫 Pod の名前
        sub_socket: PathBuf,              // 孫 Pod の socket path
        scope: Vec<ScopeRule>,            // 又貸しされた scope
    },
}
```

`Method::Notify` は触らない。`message: String` のまま。

### 子（発信側）

子の Controller が以下のタイミングで親 socket に一発接続して `Method::PodEvent` を送信し、即切断する（fire-and-forget）。

| タイミング | variant |
|---|---|
| `RunEnd { result: Finished }` 発行時 | `TurnEnded` |
| Worker 実行エラー発生時（後述） | `Errored` |
| shutdown シーケンス（controller loop 終了直前） | `ShutDown` |
| `SpawnPod` tool 成功直後 | `ScopeSubDelegated` |

`Errored` の対象は **`run_with_cancel_support` 内で Worker 実行が失敗した `Event::Error`** に限定する。`AlreadyRunning` / `NotPaused` / `NotRunning` のような method 拒否応答はクライアント向けの一時的なフィードバックであり、親に通知すべき子のライフサイクルイベントではない。実装時は発火点を worker エラー経路に絞ること。

送信は非同期 spawn で発射し、await しない。接続失敗はログのみで続行（親が落ちていても子は生きる）。

親 socket のアドレスは spawn 時に `--callback <PATH>` で受け取って保持している（既存）。

### 又貸しの親連鎖

`ScopeSubDelegated` は**直接の親にのみ送る**。曾孫が現れた場合：

1. 孫 (C) が曾孫 (D) を `SpawnPod` する
2. C は自分の親 (B) に `ScopeSubDelegated { parent_pod: C, sub_pod: D, ... }` を送る
3. B は D を自分の `spawned_pods.json` に追加し、さらに B の親 (A) へ `ScopeSubDelegated { parent_pod: B, sub_pod: D, ... }` を再発射する

これを再帰的に繰り返すことで、`scope.lock` の `delegated_from` チェーンと一致した形で root まで D の存在が登録される。各 Pod は「自分の直接の子＋紹介された孫（= 更に下の Pod も含む）」を把握し、孫より下の階層は子経由で管理される。

再発射は `ScopeSubDelegated` 受信時の system 処理の一部として行う（後述の表に反映）。

### 親（受信側）

親 Controller の main loop に `Method::PodEvent` ハンドラを追加：

```rust
Method::PodEvent(event) => {
    // (1) system 処理 — variant ごとに固有
    apply_event_side_effects(&event, &spawned_registry, &lock_path).await;

    // (2) LLM 通知 — variant を文字列にレンダリングしてバッファへ
    let text = render_event(&event);
    pod.push_notification(text);
}
```

variant 別の (1) の中身：

| variant | system 処理 |
|---|---|
| `TurnEnded` | なし |
| `Errored` | なし（LLM に判断させる） |
| `ShutDown` | `spawned_registry.remove(pod_name)`、scope lock を flock して該当 allocation を `release_pod` で解放 |
| `ScopeSubDelegated` | `spawned_registry.add(SpawnedPodRecord { sub_pod, sub_socket, scope, ... })` で孫を追加。続けて自分の親（いれば）へ `ScopeSubDelegated { parent_pod: self, sub_pod, ... }` を再発射（親連鎖参照） |

(2) の `render_event` は一箇所に集約し、`format!("Pod `{pod_name}` finished a turn.")` のような短い human-readable 文字列を返す。

### Controller の配線変更

現状 `PodController::spawn` 内で `SpawnedPodRegistry` は tool 登録のためだけに生成され、main loop の `method_rx` 処理ループには渡っていない。`Method::PodEvent` ハンドラを追加するには：

- `spawned_registry: Arc<SpawnedPodRegistry>` を main loop に `clone` で持ち込む
- `scope_lock::default_lock_path()` を event 処理時に呼ぶ（毎回 open するので保持は不要）
- 親 callback socket（自分の親、トップ Pod では `None`）を `Pod::from_manifest_spawned` 経由で既に保持している値から再利用（再発射で使う）

`apply_event_side_effects` は Controller の main loop から呼ぶ非同期関数として定義する。

### 失敗時のフォールバック

- 子 → 親の PodEvent 送信が失敗しても諦める（再試行しない）
- 親が再起動した場合や送信漏れた場合は、親の `ListPods` ツール（既存）による health check + `scope_lock::reclaim_stale` の stale 回収で不整合を解消する
- これは「コールバックは最適化、ポーリングが真のフォールバック」という方針の継続

## 設計で決めること

- **送信の接続タイムアウト**: `SpawnPod` / pod-comm-tools と揃える（5 秒想定）

### 決定事項

- **順序保証は求めず、ハンドラを冪等・遅延到着に強くする**: fire-and-forget の unix socket 接続は順序を保証しない。`TurnEnded` 直後に `ShutDown` が届いても、逆順で到着しても親側で成立するように `apply_event_side_effects` を設計する。具体的には：
  - `ShutDown` 受信時、すでに registry から削除済みでもエラーにしない（`release_pod` の `UnknownPod` を swallow する既存挙動を踏襲）
  - `TurnEnded` が `ShutDown` より後に届いても、該当 Pod が既に registry にいなければ render だけして終える（LLM 向け通知は出る、system 処理は no-op）
  - `ScopeSubDelegated` で孫が既に registry にいたら上書きせず no-op（`DuplicatePodName` を swallow）
- **`ScopeSubDelegated` の親連鎖は直接の親のみ + 再発射**: 上記「又貸しの親連鎖」セクション参照。曾孫以上は再帰的に再発射で root まで届く

## 完了条件

- `Method::PodEvent(PodEvent)` が `protocol` crate に追加され、serde round-trip テストが通る
- 子の Controller が 4 種の variant を適切なタイミングで親 socket に送信する。`Errored` は Worker 実行エラーにのみ限定されることを確認
- 親の Controller が variant ごとの system 処理を実行し、レンダリングした文字列を LLM 通知バッファに流す
- `ScopeSubDelegated` 受信後、孫 Pod が親の `spawned_pods.json` に現れ、親が更に上位親を持つ場合は上位へ再発射される
- `ShutDown` 受信後、該当 Pod が親の registry から消え、scope lock からも解放される
- イベント到着順が入れ替わっても副作用が安全（冪等）であることを単体テストで確認
- 送信失敗しても子プロセスが続行する
- 各 variant の送受信を検証する単体テスト

## 範囲外

- リモート親への送信（SSH 越し）。ローカル Unix socket のみ
- 配信保証（at-least-once / exactly-once）
- 親再起動時の「見逃したイベント」の再送。ポーリングで補う前提
- `Method::Notify` の `source` フィールド削除（別チケット）
