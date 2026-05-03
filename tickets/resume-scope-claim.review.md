# Review: Resume 時の Scope Claim の改善

## 前提・要件の確認

### R1: resume 時の overlay 合成は cwd 盲信ではなく、当該セッションが過去に持っていた scope を反映する。情報源は session log / registry / その他のいずれでも良いが、何らかの永続情報から復元できること
- **満たされている。** 情報源は session log の `LogEntry::Extension { domain: "pod.scope", .. }` (`crates/session-store/src/session_log.rs:200-207`)。`PodScopeSnapshot { allow, deny }` を `RestoredState.pod_scope` で最新分のみ保持 (`session_log.rs:227-229`, `308-315`)。tui resume では `load_resume_scope` が `restore` を呼んで snapshot を取り出し overlay の `[scope]` に注入する (`crates/tui/src/spawn.rs:401-417`)。pod 側は `restore_from_manifest` が同じ snapshot から `ScopeConfig` を再構築する (`crates/pod/src/pod.rs:2100-2113`)。

### R2: 過去の scope 情報が取得できないセッションは、明示的なエラーで止めるか、ユーザー確認後に fresh claim にフォールバック (黙って広げない)
- **満たされている。** snapshot 無しは tui 側で `SpawnError::MissingResumeScope` (`spawn.rs:55, 410-413`)、pod 側で `PodError::SessionScopeMissing` (`pod.rs:2103, 2405-2408`) として明示的にエラー化する。`restore_from_manifest_rejects_session_without_scope_snapshot` (`crates/pod/tests/restore_test.rs:84-110`) で経路をカバー。

### R3: claim 試行が registry の既存 allocation と衝突した場合、エラーで衝突相手の Pod 名と target rule の双方が伝わる
- **満たされている。** `ScopeLockError::WriteConflict` に `competitor_rule: ScopeRule` を追加 (`crates/pod-registry/src/error.rs:16-21`)、`find_conflict_owner(s)` を `ConflictOwner { pod_name, rule }` を返す形に再設計 (`conflict.rs:84-155`)。`Display` 文言にも competitor の rule.target が含まれる。`partial_deny_does_not_hide_parent_conflict` (`conflict.rs:259-295`) で双方の値を確認。

### R4: 委譲済みエントリが同じセッションの委譲チェーンに属する場合、resume はその範囲を claim せずに進行する
- **満たされている (実装方針には注記あり)。** `register_pod_with_deny` 内の `all_denied` 判定 (`mutate.rs:62-84`) で、登録者の deny セットが衝突相手のルールを完全に覆う場合は衝突を無視する。snapshot に持っていた deny がそのまま委譲済み領域なので、結果として「自分が手放した範囲」は claim せずに進める。
- 注: 「同じセッションの委譲チェーン」かどうかを registry 側で構造的に検証してはおらず、deny マスクへの信頼で運用している。実用上は parent の release_pod が child を reparent し、registry 側の整合性が保たれているため安全。後述の Non-blocking 参照。

## 完了条件の確認

- 親 → 子委譲 → 親 exit → 親セッション resume の成功フロー: `denied_write_region_is_not_claimed_by_restored_parent` (`conflict.rs:233-256`) で確認。
- 衝突する無関係 Pod に対する明確なエラー: `partial_deny_does_not_hide_parent_conflict` (`conflict.rs:259-295`) で確認 (rule + 名前を error から取り出してアサート)。
- 既存 fresh spawn の挙動非変化: `from_manifest` 経路は引き続き `prepare_pod_common` (cwd default scope) を通る (`pod.rs:2439-2445`)。`overlay_uses_resume_scope_snapshot` の対と既存 `cascade_merge_detects_scope_from_any_layer` 等で fresh パスは温存。

## アーキテクチャ・スコープ

- 永続化は session-store の既存の `LogEntry::Extension` ドメイン枠に乗せる選択。新スキーマ追加せず、`pod.scope` ドメインの opaque payload として運用しており、ticket の「過去スコープの永続化スキーマを新規導入するかは実装時に決める」に沿う。crate 越境なし、低レベル基盤の汚染なし。
- `pod-registry` 側の追加は、既存 API を `*_with_deny` への薄いラッパーに置き換える形で互換性を維持。`Allocation::scope_deny` は `#[serde(default)]` で旧 `pods.json` も読める。最小限の拡張で「rule 情報を含めるための最小限の拡張のみ」という ticket 範囲外宣言と整合。
- `Pod::scope_change_sink` + `flush_pending_scope_snapshot` で同期 tool callback と非同期 store の隙間を埋める設計は妥当。flush ポイント (初回 SessionStart 後 / SpawnPod 後 turn 完了 / compact 完了) も網羅されている。
- snapshot 永続のタイミングは「revoke 直後ではなく次の `persist_turn`」。process crash と revoke の間に挟まれるレースは存在するが、その状態でも registry の `WriteConflict` で resume が落ちる (= 黙って広げない) ため、ticket 要件は損なわれていない。

## 指摘事項

### Blocking
- なし。

### Non-blocking / Follow-up
- **deny マスク経由での衝突回避は「自分の deny が覆う」全衝突に効く** (`mutate.rs:67-76`)。実装上は委譲チェーン外の偶発的な所有者にも適用される。現状の registry 整合性を前提にすれば安全だが、要件 4 の文言と実装ロジックの差は将来の混乱の種になりうる。コメント or doctest で「deny にあれば誰の所有でも譲る」という意図を明記しておくとよい (`register_pod_with_deny` の doc に一文)。
- **payload 破損時のメッセージ**。`collect_state` の `serde_json::from_value(...).ok()` (`session_log.rs:313`) は破損 payload を `None` に丸める。結果として resume 側のエラーは「snapshot 無し」と同じ表記になる (`pod.rs:2406`)。要件 2 は満たすが、運用時の切り分けには「破損」と「未保存」の区別が欲しくなる場合がある。Follow-up でログ警告を出すか、エラー variant を分けるかは選択肢。
- **revoke 〜 次 turn の crash race**。`SpawnPodTool` で revoke した直後に親が落ちると、最新 snapshot は revoke 前のもの。resume 時は古い (広い) allow で claim しようとし、registry が `WriteConflict` で阻止する。挙動として安全側だが、エラー文言は「resume too wide」ではなく「pod 衝突」になり、ユーザー視点で原因が分かりにくい。気になるようであれば revoke 直後に同期で snapshot を queue → 次の保存可能ポイントまで bridging する設計の見直し。優先度は低い。

### Nits
- `crates/pod/src/pod.rs:262-263` の空白行差分は noise。
- `prepare_pod_common*` 系の三段呼び出し (`prepare_pod_common`, `_with_scope`, `_from_scope`) はわずかにラップが多い。`_from_scope` だけ private にして `Option<ScopeConfig>` で分岐する形でも構わないが、現状でも読みやすさは確保されているので強い指摘ではない。

## 判断

**Approve (完了可)** — ticket の前提・要件・完了条件はすべて根拠付きで満たされており、`cargo check --workspace` / `cargo test --workspace` も成功する (warnings は既存の dead_code のみ)。アーキテクチャ的にも既存の Extension ドメイン枠 + registry 互換ラッパーで最小限に収まっており、コードベースを歪めていない。Non-blocking の指摘は次フェーズ以降で個別に処理すれば足りる。
