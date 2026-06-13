<!-- event: create author: "yoi ticket" at: 2026-06-13T07:31:09Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-06-13T07:31:25Z -->

## Plan

背景:
- `yoi.profile.extend("builtin:default", { scope = yoi.scope.workspace_read() })` は、Lua レベルの deep merge により base の `scope.intent = "workspace_write"` と override の object が merge される。
- その後の `PodManifestConfig::merge` でも scope allow/deny は加算されるため、role profile が `workspace_read()` を指定しても `builtin:default` 由来の direct workspace write が残り得る。
- 今回の暫定対応では Orchestrator profile を `scope = "workspace_read"` / `delegation_scope = "workspace_write"` にして、object merge ではなく scalar replacement を使った。

要件:
- Profile 継承は維持する。
- `scope` / `delegation_scope` のような authority-bearing field について、意図的に inherited value を置換またはクリアできる API を設計する。
- role profile が default profile の model / compaction / feature defaults 等を継承しつつ、direct authority だけを安全に narrower scope へ置換できるようにする。
- 空 object `{}` で消せるようにするか、`yoi.profile.replace(...)` / `yoi.scope.replace(...)` / `replace = true` などの明示 API にするかは設計で決める。
- 不注意な権限拡張や hidden fallback を避け、resolved profile の authority が読みやすいこと。

受け入れ条件:
- Orchestrator role profile が `builtin:default` を継承しても direct workspace write を要求しないことを test で確認する。
- 既存 profile API 互換を壊す場合は、移行対象を明示する。
- `scope` と `delegation_scope` の merge/replace semantics が docs または test 名から読み取れる。


---
