# Review: パーミッション: パターンベースのツール実行制御

## 前提・要件の確認
- `[permissions]` が無い場合は無効、ある場合は `default_action` 必須: 満たされている。`PodManifestConfig.permissions` は `Option` で、resolve 時に `permissions.default_action` 不在を `MissingField` にしている (`crates/manifest/src/config.rs:39-42`, `crates/manifest/src/config.rs:430-440`)。
- `[[permissions.rule]]` と first-match / fallback 評価: 満たされている。rule は `rename = "rule"` で TOML 形に対応し、`PermissionHook::action_for` が宣言順 `find` + `default_action` fallback を行う (`crates/manifest/src/lib.rs:246-276`, `crates/pod/src/permission.rs:25-35`)。
- built-in `PreToolCall` Hook として、ユーザー Hook より先に評価: 満たされている。Pod 構築直後に `apply_permissions_from_manifest` が呼ばれ、permission hook は通常の user hook 登録前に `HookRegistryBuilder` へ追加される (`crates/pod/src/pod.rs:336-337`, `crates/pod/src/pod.rs:2295-2296`, `crates/pod/src/permission.rs:39-45`)。
- `allow`: 満たされている。permission action `Allow` は `PreToolAction::Continue` に変換される (`crates/pod/src/permission.rs:49-56`)。
- `deny` は turn Abort ではなく synthetic error tool result: 満たされている。`PreToolAction::SyntheticResult` が追加され、Worker は tool を実行せず result を commit する (`crates/llm-worker/src/interceptor.rs:48-64`, `crates/llm-worker/src/worker.rs:757-769`, `crates/llm-worker/src/worker.rs:1136-1144`)。
- provider-visible `is_error` の保持: 満たされている。`ToolResult` / `Item::ToolResult` / session replay に `is_error` が通り、Anthropic request へ `tool_result.is_error` が投影される (`crates/llm-worker/src/tool.rs:281-310`, `crates/llm-worker/src/llm_client/types.rs:74-89`, `crates/session-store/src/logged_item.rs:34-40`, `crates/llm-worker/src/llm_client/scheme/anthropic/request.rs:327-347`)。
- `ask`: 現時点のスコープとしては満たされている。型として受け付け、承認 protocol 未実装環境では待機せず fail-closed synthetic error result にしている (`crates/pod/src/permission.rs:53-56`, `docs/pod-factory.md:218-235`)。

## アーキテクチャ・スコープ
- `llm-worker` には汎用的な `SyntheticResult` action だけを追加し、manifest policy 本体は Pod 層の `permission.rs` に置かれているため、層の分離は保たれている。
- 新しい trait は追加されておらず、既存の Hook / Interceptor 境界上で実装されている。
- `Scope` の物理境界には手を入れておらず、permission は tool 実行ポリシーとして分離されている。
- `ask` の protocol/UI 実装に踏み込まず fail-closed として明示している点は、今回の段階的実装として妥当。

## 指摘事項
### Non-blocking / Follow-up
- mixed allow/deny 時の tool result 順序が tool call 順とずれる可能性がある — synthetic result は `synthetic_results` に集めて実行済み result の末尾へ `extend` されるため、先頭 call が deny で後続 call が allow の場合、履歴上の result 順が逆転する (`crates/llm-worker/src/worker.rs:745-769`, `crates/llm-worker/src/worker.rs:821-823`)。provider が call id で対応を取れる限り致命的ではないが、会話の時系列としては call 順を保つ方が安全。
- manifest から Pod へ permission が実際に効く統合テストが薄い — manifest resolve と `PermissionHook` 単体、および Worker の synthetic result はカバーされているが、`[permissions]` を含む Pod 構築から tool deny までの結合テストがあると回帰検出しやすい。

## 判断
Approve with follow-up — 要件は満たしており設計上の逸脱もないが、result 順序と Pod 統合テストは後続で固める価値がある。

## 確認コマンド
- `cargo test -p pod permission -- --nocapture`
