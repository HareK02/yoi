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
| `Event::Error` 発行時 | `Errored` |
| shutdown シーケンス（controller loop 終了直前） | `ShutDown` |
| `SpawnPod` tool 成功直後 | `ScopeSubDelegated` |

送信は非同期 spawn で発射し、await しない。接続失敗はログのみで続行（親が落ちていても子は生きる）。

親 socket のアドレスは spawn 時に `--callback <PATH>` で受け取って保持している（既存）。

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
| `ScopeSubDelegated` | `spawned_registry.add(SpawnedPodRecord { sub_pod, sub_socket, scope, ... })`（孫を直接把握する。親が子を経由せず孫を管理することで、子が死んでも孫の scope 管理が維持される） |

(2) の `render_event` は一箇所に集約し、`format!("Pod `{pod_name}` finished a turn.")` のような短い human-readable 文字列を返す。

### 失敗時のフォールバック

- 子 → 親の PodEvent 送信が失敗しても諦める（再試行しない）
- 親が再起動した場合や送信漏れた場合は、親の `ListPods` ツール（既存）による health check + `scope_lock::reclaim_stale` の stale 回収で不整合を解消する
- これは「コールバックは最適化、ポーリングが真のフォールバック」という方針の継続

## 設計で決めること

- **送信の接続タイムアウト**: `SpawnPod` / pod-comm-tools と揃える（5 秒想定）
- **同時多発イベントの順序保証**: `TurnEnded` 直後に `ShutDown` が起きた場合、親側で順序を保証する必要があるか。現状は fire-and-forget で並列送信されうる
- **`ScopeSubDelegated` の親連鎖**: 孫がさらに曾孫を spawn したとき、曾孫の `ScopeSubDelegated` は誰に送る？（子に送り、子が親に転送? or 最上位の root まで届ける? or 直接の親だけで十分?）

## 完了条件

- `Method::PodEvent(PodEvent)` が `protocol` crate に追加され、serde round-trip テストが通る
- 子の Controller が 4 種の variant を適切なタイミングで親 socket に送信する
- 親の Controller が variant ごとの system 処理を実行し、レンダリングした文字列を LLM 通知バッファに流す
- `ScopeSubDelegated` 受信後、孫 Pod が親の `spawned_pods.json` に現れる
- `ShutDown` 受信後、該当 Pod が親の registry から消え、scope lock からも解放される
- 送信失敗しても子プロセスが続行する
- 各 variant の送受信を検証する単体テスト

## 範囲外

- リモート親への送信（SSH 越し）。ローカル Unix socket のみ
- 配信保証（at-least-once / exactly-once）
- 親再起動時の「見逃したイベント」の再送。ポーリングで補う前提
- `Method::Notify` の `source` フィールド削除（別チケット）
