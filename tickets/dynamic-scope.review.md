# Review: 動的 Scope 変更

レビュー対象コミット: `0d66b39 feat: dynamic-scopeの実装`
（作業ツリーは clean、実装はすべてこの一コミットに集約。`fa84d48 fix: SpawnPodの起動経路の問題・を修正` は本チケット直前の SpawnPod の起動経路修正で、本チケットの前提を整えるもの。）

## 前提・要件の確認

- **「Pod 実行中に scope を追加でき、追加後のツール実行が新しい scope を反映する」**: 満たされている。`SharedScope` (`ArcSwap<Scope>` + `Mutex<()>`) を `crates/manifest/src/scope.rs:265-340` に新設し、`ScopedFs` の hot-path (`crates/tools/src/scoped_fs.rs:95,128`) で `self.inner.scope.load()` を毎回読む。`crates/tools/src/scoped_fs.rs:343-380` の単体テスト (`add_allow_rule_through_shared_scope_grows_readable_set`) で動的反映を検証。
- **「Pod 実行中に scope を縮小でき、縮小後のツール実行が制限を反映する」**: 満たされている。`Scope::with_added_deny_rules` (`crates/manifest/src/scope.rs:202-212`) と `SharedScope::update` 経由で deny 畳み込み。`crates/tools/src/scoped_fs.rs:382-412` の `revoke_write_through_shared_scope_blocks_subsequent_writes` で Write 剥奪と Read 残存の双方を assertion。
- **「scope 変更が pod-registry と Pod 内 Scope の両方に整合的に反映される」**: 現状の唯一の縮小経路 (SpawnPodTool) について満たされている。`crates/pod/src/spawn/tool.rs:188-197` で registry に `delegate_scope`、`crates/pod/src/spawn/tool.rs:238-249` で `exec_child` 成功後に `Permission::Write` のみを deny で畳み込む。registry の `is_within_effective_write` (`crates/pod-registry/src/conflict.rs:49-72`) が Write のみアービトレートする実装と意味論が一致しており、Read を残す判断は正当。
- **「単体テストで動的追加・縮小後の permission チェックが検証される」**: 満たされている。
  - manifest 側: `with_added_allow_rules_grows_readable_set` / `with_added_deny_rules_demotes_write_to_read` / `shared_scope_load_returns_current_value` / `shared_scope_update_replaces_view_atomically` / `shared_scope_clones_share_state` (`crates/manifest/src/scope.rs:660-741`)。
  - tools 側: `add_allow_rule_through_shared_scope_grows_readable_set` / `revoke_write_through_shared_scope_blocks_subsequent_writes` / `shared_scope_changes_propagate_across_clones` (`crates/tools/src/scoped_fs.rs:343-444`)。
  - SpawnPod 側: `spawn_pod_delegates_scope_and_sends_run` で成功時の Write→Read demote、`spawn_pod_rejects_scope_outside_spawner` / `spawn_pod_rolls_back_reservation_when_socket_never_appears` で失敗時の scope 不変を assertion (`crates/pod/tests/spawn_pod_test.rs:200-310,378-386`)。
- **範囲外**: `protocol::Method::GrantScope/RevokeScope` は導入なし、履歴・監査ログも追加なし。範囲外宣言と一致。コメントには将来の `GrantScope` 拡張点として残してあるのみ (`crates/manifest/src/scope.rs:191`、`crates/pod/src/controller.rs:239`)。

## アーキテクチャ・スコープ

- **layer 分離**: `SharedScope` を manifest crate に置いた判断は妥当。`ScopedFs` (tools)、`Pod` (pod) の両方が型に依存できる共通土台になる。`llm-worker` は触っておらず、低レベル基盤の責務範囲を逸脱していない。
- **クレート命名 / 依存追加**: 新規クレートは追加なし。`arc-swap = "1"` を `crates/manifest/Cargo.toml` に追加。`cargo add` 由来とおぼしきフォーマット (`= "1"`)。新規プレフィックスやトップレベル変更なし。
- **bash-output Read rule の畳み込み (`crates/pod/src/controller.rs:248-256`)**: 旧来は `scope_for_tools` を `ScopeConfig` 経由で再構築して per-tool だけ別 scope を持たせていた。これを Pod の `SharedScope` 自体に push する形に統一したのは設計として綺麗。Pod 側の `summary()` (greeting) や spawn 子に伝搬する scope にも一貫して反映されるため、agent 視点・delegate 視点での scope 認識が一致する。
- **SpawnPod 縮小セマンティクス**: 「Write のみ revoke、Read は残す」は registry の effective_write 設計と一致しており、本実装でクロスチェックの意味論が初めてプロセス内に反映された。これは ticket の中心要件である「pod-registry と Pod 内 Scope の整合」を実装上 enforced にしたという意味で重要な前進。
- **範囲外への滲み出し**: 確認した範囲では存在しない。`SharedScope::store` / `Pod::add_scope_rules` / `Pod::revoke_scope_rules` の API 追加は将来の `GrantScope` 想定の前駆だが、現状未使用。これは下記 Non-blocking 参照。

## 指摘事項

### Non-blocking / Follow-up

- **`Pod::add_scope_rules` / `Pod::revoke_scope_rules` (`crates/pod/src/pod.rs:273-300`) が未使用**: production code は `pod.scope().update(...)` (`crates/pod/src/controller.rs:248`) や `spawner_scope.update(...)` (`crates/pod/src/spawn/tool.rs:244`) を直接叩いており、`Pod` の薄いラッパは呼ばれていない・テストも無い。今回 ticket ゴールが「Pod 内変更経路だけで十分」なら、(a) ラッパを削って `pod.scope()` のみを公開する、(b) ラッパ経由に統一する、のどちらかに揃えると API surface の二重化が消える。将来 `protocol::Method::GrantScope` を追加する時にラッパ側に責務を集約するつもりであればコメントでその意図を残しておくと迷子にならない。
- **`SharedScope::store` (`crates/manifest/src/scope.rs:317-321`) が未使用**: 現状 `update` で全要件が満たせている。今すぐ削っても良いし、外部入力 (`GrantScope`) で一括上書きを想定するなら残しても良いが、その場合は使い所をコメントで明示すべき。docstring の「Concurrent `update` callers ... see this store reflected on their next iteration」は誤解を招く: `update` は内部 `Mutex<()>` を取るので `store` と直列化されており、interleave しない。
- **bash-output Read rule の重複追加リスク (`crates/pod/src/controller.rs:248-256`)**: 同一プロセスで `start_pod` 系を複数回回した場合、`with_added_allow_rules` は dedup しないので `allow(Read, .../bash-output)` が累積する。現在の利用パターンでは Pod = 1プロセスなので発生しないが、SharedScope に「すでにこの rule があれば skip」のチェックを入れるか、idempotent な API (`upsert`) を検討する余地あり。今回の責務外。
- **`scope_snapshot()` の使い分け一貫性 (`crates/pod/src/pod.rs:626`)**: 同じ Pod 内で `self.scope.snapshot()` を直接呼んでいる箇所と `Pod::scope_snapshot()` を経由する箇所が混在している。意味は同じだが将来 snapshot に副作用 (例: 計測) を仕込みたい時に呼び出し点を一箇所にまとめておけると安心。

### Nits

- **`crates/pod/src/spawn/tool.rs:243-249`**: `revoke_write` を `if !revoke_write.is_empty()` でガードしているが `update` 自体は no-op input でも安全 (`with_added_deny_rules([])` は同じ scope を返すだけ)。ガードは可読性目的なら残して良い。
- **`crates/manifest/src/scope.rs:332-340` の docstring**: `store` の文言が `update` と中途半端に被る。`update` の方を「常にこちらを使う」のリコメンドにし、`store` は「外部から完全な scope を持ち込む時のみ」と明示するとよい。
- **`crates/pod/src/spawn/tool.rs:115-128` の field doc**: 「Mirrors the pod-registry's `effective_write` semantics: Write is the only permission tracked across Pods, so revocation only touches Write.」の説明が良い。pod-registry/conflict.rs 側にもこの意味論を「Pod 内 Scope と一貫させている」旨追記してもよい (双方向リンク)。

## 判断

**Approve with follow-up** — ticket の完了条件はすべて満たされており、テストも要件と十分対応している。アーキテクチャ上の歪みは無く、`SharedScope` の置き場 (manifest) と `ScopedFs` のホット読み戦略 (`load()` で都度スナップショット) は filtering 設計として無理がない。SpawnPod 経由の Write-only revoke は registry の意味論と整合。指摘した API の重複 (`Pod::add/revoke_scope_rules`、`SharedScope::store`) はブロッカーではないが、protocol 拡張前に方針を一度整理すると将来の混乱を防げる。
