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

<!-- event: decision author: hare at: 2026-06-14T02:14:43Z -->

## Decision

決定:
- `yoi.profile.extend` に replace/clear API を足す方向ではなく、`extend` 自体を廃止する。
- Profile 継承は `local p = yoi.profile.import("builtin:default"); p.field = ...; return p` の ordinary Lua assignment に寄せる。
- deep merge helper が必要なら、semantics が明示された別 API として後続で検討する。
- scope/delegation_scope の問題は `00001KV11DHGZ` で launch policy に移すため、この Ticket では scope replacement API を作らない。


---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T06:08:28Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-14T06:10:02Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket is queued and explicitly decides to deprecate/remove `yoi.profile.extend` in favor of `yoi.profile.import` plus explicit Lua assignment.
- Relation checks show no blocker; related launch-policy Ticket `00001KV11DHGZ` is not a dependency for this implementation.
- Risk is bounded to Lua Profile API/resources/tests.

IntentPacket:
- Migrate builtin Profile resources away from `yoi.profile.extend`, keep `import`, and make `extend` fail clearly or emit a deprecation diagnostic. Remove docs/tests that suggest scope replacement APIs.

Binding invariants:
- Do not solve concrete scope/delegation authority here; that belongs to `00001KV11DHGZ`.
- Do not add ambiguous replacement/clear API as part of `extend`.
- Builtin resources must use import + explicit Lua assignment.

Validation:
- focused profile resolution tests, resource/profile tests, `cargo fmt --check`, `git diff --check`, `cargo build -p yoi`; `nix build .#yoi` if resource packaging is affected.

---

<!-- event: state_changed author: orchestrator at: 2026-06-14T06:10:45Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Ticket evidence, related records, orchestration plan, and clean workspace state were checked. No blockers remain; accept for implementation before worktree/spawn side effects.

---
