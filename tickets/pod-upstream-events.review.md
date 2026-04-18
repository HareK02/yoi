# Review: pod-upstream-events

実装コミット `78a15bf`（pod-upstream-event実装）に対するレビュー。`cargo build` clean、`cargo test --workspace` 全 pass（366 + 8 の新規テスト含む）。

## 総評

チケット要件の中核（`Method::PodEvent` 新設、`Method::Notify` の `source` 削除、`ScopeRule`/`Permission` を protocol crate へ、子の 4 種 variant 発火、親の system 処理 + LLM 通知、`ScopeSubDelegated` 親連鎖の再発射、冪等ハンドラ）は達成。ただし**在 flight ターン中の `PodEvent` がロスする実装バグ**が 1 件あり、これは修正必須。

## 完了条件の対応

| 要件 | 状態 | 備考 |
|---|---|---|
| `Method::PodEvent(PodEvent)` + serde roundtrip | ✅ | 4 variant 全てのテストあり |
| 子の 4 variant 発火 | ✅ | `TurnEnded` / `Errored` は `run_with_cancel_support` の Ok/Err、`ShutDown` は controller task 終了直前、`ScopeSubDelegated` は `spawn_pod.rs` の registry.add 後 |
| `Errored` は worker 実行エラー限定 | ✅ | `Err(e)` アームのみ、method 拒否応答は発火せず |
| 親の variant 別 system 処理 + LLM 通知 | ⚠️ | IDLE 時は動作、在 flight 中は **ロス**（後述） |
| `ScopeSubDelegated` の再発射 | ✅ | `apply_event_side_effects` 内で `self_parent_socket` 経由で再帰 |
| `ShutDown` 受信で registry + scope lock 解放 | ✅ | `release_scope_silently` が `UnknownPod` を swallow |
| 冪等性（重複受信無害） | ✅ | `DuplicatePodName` swallow、missing ShutDown no-op |
| 送信失敗で子は続行 | ✅ | fire-and-forget、tracing::warn のみ |
| 単体テスト | ✅ | `pod_events_test.rs` 8 件、protocol の roundtrip 4 件 |

## 指摘と判断

### 修正必須

#### 1. 在 flight ターン中の `PodEvent` がロスする

**場所**: `crates/pod/src/controller.rs:579-588`

```rust
Some(Method::PodEvent(_)) => {
    // PodEvent is handled in the main loop (next
    // iteration). Dropping it here is fine because
    // the sender is external and we will see it
    // again via `method_rx` after the current turn
    // ends — ...
}
```

**問題**: 親 Pod が RUNNING / Paused の間に子から `PodEvent` が到着すると、`run_with_cancel_support` 内の `select!` のこのアームが発火し、イベントを**単純に drop** する。コメントは「next iteration で再配送される」と言っているが、`method_rx.recv()` は mpsc の消費型で、一度 recv したメッセージは channel から消える。再配送機構はコード上存在しない。

- 結果: 親が長いターンを回している最中に子が `TurnEnded` / `Errored` / `ShutDown` / `ScopeSubDelegated` を送ると**ロス**する
- チケットの方針は「配信保証なし、`ListPods` ポーリングが真のフォールバック」なので**動作としては成立**するが、ポーリング前に孫の `ScopeSubDelegated` が失われると、親が孫を把握しないまま子が死亡した場合に孫の scope 管理が宙に浮く懸念が残る（ただし `scope_lock::reclaim_stale` で delegated_from が辿られるので最終的には救済される）

**修正**: `run_with_cancel_support` 内で drop せず、`Method::Notify` と同様に notification_buffer に render 結果を push、加えて `apply_event_side_effects` を inline で呼ぶ。これには現状の `run_with_cancel_support` シグネチャに `spawned_registry` を追加する配線変更が必要：

```rust
Some(Method::PodEvent(event)) => {
    apply_event_side_effects(&event, &spawned_registry, self_name, &parent_socket.cloned()).await;
    notification_buffer.push(render_event(&event));
}
```

コメントの「ordering guarantees that matter for PodEvent don't exist anyway」は部分的に正しい（順序は保証しない）が、「消失は許容」とは書いていないのでコメントの根拠にならない。少なくとも**コメントを「ロスを許容する。次ターンで `ListPods` で補う」と正直に書き換える**か、上記の inline 処理に変える。inline 処理を推奨。

### 要議論

#### 2. 孫の `callback_address` が祖父の registry で「死ぬ可能性のある子の socket」を指す

**場所**: `crates/pod/src/pod_events.rs:119-123`

```rust
let callback_address = registry
    .get(parent_pod)  // parent_pod = 送信者の子 (C)
    .await
    .map(|r| r.socket_path)
    .unwrap_or_else(PathBuf::new);
```

B が C から `ScopeSubDelegated { parent_pod: C, sub_pod: D, ... }` を受信すると、B の registry に D を追加する際、D の `callback_address` を「C の socket」に設定する。これは意味的には正しい（D は C に報告する）。しかし C が死ぬと D の PodEvent 送り先が dead socket になり、D の上流報告が機能しなくなる。

- チケットの主張「親が子を経由せず孫を管理することで、子が死んでも孫の scope 管理が維持される」は *scope 管理* の話なので、B の registry に D が居ることだけで満たされる（OK）
- ただし「D の LLM 通知は止まる」という副作用が残る
- 将来 D を B に「再 attach」する仕組み（callback redirection）を作るときのフック点として、`callback_address` の更新 API を先回りで用意するか、今は割り切るか

**判断**: 本チケットは scope 管理の保全が主目的で LLM 通知の連鎖保全は範囲外なので、現状の割り切りで OK。ただしチケット or 範囲外セクションに「子が死んだ後の孫の upstream 通知路は別途」と一行書いておきたい。

#### 3. Controller task 終了時の `ShutDown` 送信レース

**場所**: `crates/pod/src/controller.rs:480-487`

```rust
crate::pod_events::fire_and_forget(
    self_parent_socket.clone(),
    protocol::PodEvent::ShutDown { pod_name: spawner_name.clone() },
);
let _ = shutdown_tx.send(());
```

`fire_and_forget` は `tokio::spawn` で送信を後回しにし、すぐに `shutdown_tx.send(())` で shutdown を通知。`main.rs` が `shutdown_rx` 受信後すぐ `ExitCode::SUCCESS` を返すと、spawn されたタスクは接続・送信を終える前にプロセス終了で殺される可能性がある。

**判断**: これは最後の一発なので `fire_and_forget` ではなく `send_pod_event(...).await` で inline 送信すべき（5 秒 timeout は `connect_and_send` 内で保証されている）。プロセス shutdown 時にのみ発生する特殊ケースなので、修正コストは小さい。

### 軽微

#### 4. `render_event` の `Errored` が message をそのまま埋め込む

Provider エラーメッセージは数 KB 級のこともあるので、LLM 通知に長大な文字列が入る可能性。実運用で問題になれば truncation を後付けで入れれば良い。今は放置で OK。

#### 5. コメントと実装の整合性（指摘 1 関連）

指摘 1 の修正に合わせて、`run_with_cancel_support` の doc comment にある「Transient method rejections such as `AlreadyRunning` are intentionally NOT reported as `Errored`」は正しく機能している（Err ブランチでのみ `fire_and_forget` が呼ばれる）。これは良い点で、維持したい。

## 完了に向けた作業

1. **指摘 1（必須）**: 在 flight 中の `Method::PodEvent` を inline 処理に変更
2. **指摘 3（推奨）**: controller task 終了時の `ShutDown` を `await` 送信に
3. **指摘 2（任意）**: 孫 callback の将来課題を範囲外セクションに一行追記
4. 残りの軽微はそのままで OK

指摘 1 を直したら完了可。
