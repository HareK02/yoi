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

<!-- event: implementation_report author: hare at: 2026-06-14T06:24:18Z -->

## Implementation report

Implemented removal/deprecation of yoi.profile.extend and migrated builtin role Profile resources to yoi.profile.import plus explicit Lua assignment.

Changes:
- yoi.profile.extend now fails with a removed-API diagnostic directing users to yoi.profile.import(...) plus assignment; the previous JSON deep-merge implementation was removed.
- Builtin role Lua profiles (companion/intake/orchestrator/coder/reviewer) import builtin:default, then assign each overridden field explicitly. Worker instruction overrides preserve imported worker defaults by assigning p.worker.instruction.
- Focused profile tests now cover explicit assignment, object replacement without hidden deep-merge retention, removed extend diagnostics, and role profile resolution/feature policy.

Validation:
- cargo fmt --check
- git diff --check
- cargo test -p manifest profile::tests:: -- --nocapture
- cargo build -p yoi
- nix build .#yoi (completed; dirty-tree warning expected before commit, result symlink removed)

Risks/notes:
- yoi.profile.extend remains present only as a failing diagnostic stub so callers receive an actionable migration error instead of a nil-call Lua error.
- Scope/delegation authority semantics were not expanded; this change only preserves existing builtin profile scope/delegation declarations while changing composition style.


---

<!-- event: review author: hare at: 2026-06-14T06:29:17Z status: approve -->

## Review: approve

承認します。

Evidence:
- `crates/manifest/src/profile.rs` の Lua API は `yoi.profile.import(...)` を残し、`yoi.profile.extend(...)` は常にエラーにする互換診断スタブに変更されている。旧 `deep_merge_profile_json` と hidden deep-merge composition は削除されており、診断文も `yoi.profile.import(...)` と explicit Lua assignment への移行を示している。
- Builtin role profiles (`resources/profiles/{companion,intake,orchestrator,coder,reviewer}.lua`) は `yoi.profile.import("builtin:default")` 後に Lua の明示代入で必要フィールドだけを上書きしている。feature policy は各 role で全 key を明示し、worker instruction は table mutation で default reasoning 等を保持している。
- scope/delegation は既存 role の明示値を移植しており、profile composition の変更に留まっている。launch policy / authority semantics は別 Ticket `00001KV11DHGZ` の範囲として残され、今回の diff では拡張されていない。
- Tests cover import + explicit assignment, object replacement instead of deep merge, removed extend diagnostic, and builtin role resolution/policy (`profile::tests`).
- Resource embedding/packaging risk is low: changed files are existing `resources/profiles/*.lua` loaded through the existing include table; `cargo build -p yoi` passed.

Validation performed:
- `git diff f709fc10..HEAD` inspected.
- `cargo fmt --check` passed.
- `git diff --check f709fc10..HEAD` passed.
- `cargo test -p manifest profile::tests:: -- --nocapture` passed.
- `cargo build -p yoi` passed.

Not rerun:
- `nix build .#yoi` was not rerun in this reviewer scope because it would write outside the permitted `target/`/Ticket-record areas unless using broader Nix/store/result-link authority. Coder reported it was run; cargo build validated resource embedding on this branch.

Risks / notes:
- Existing callers of `yoi.profile.extend` now fail intentionally; this is the requested break. The retained stub is a diagnostic compatibility trap, not a merge API.


---

<!-- event: implementation_report author: orchestrator at: 2026-06-14T06:34:22Z -->

## Implementation report

Integration outcome: merged and validated.

- Implementation branch: `ticket-00001KTZY8HK2-profile-extend-removal`
- Implementation commit: `7c6070ef profile: remove extend profile composition`
- Review commit: `c12fbd8e ticket: approve profile extend removal`
- Orchestrator merge commit: `58b15ee6 merge: profile extend removal`

Reviewer result:
- `approve`。`yoi.profile.extend` deep-merge composition is removed, builtin role profiles use `yoi.profile.import("builtin:default")` plus explicit Lua assignment, and no scope/delegation authority semantics were expanded into this Ticket.

Orchestrator validation after merge:
- `cargo fmt --check`: PASS
- `git diff --check`: PASS
- `cargo test -p manifest profile::tests:: -- --nocapture`: PASS
- `cargo build -p yoi`: first attempt failed due to disk full during Rust metadata output; after stopping/removing profile child Pods/worktree/target, rerun PASS
- `nix build .#yoi`: PASS (result symlink produced)

Cleanup already performed before rerun validation to free disk:
- stopped `coder-00001KTZY8HK2-profile-extend` and `reviewer-00001KTZY8HK2-profile-extend`
- removed child worktree `/home/hare/Projects/yoi/.worktree/profile-extend-removal`
- deleted branch `ticket-00001KTZY8HK2-profile-extend-removal`

Residual notes:
- Existing users of `yoi.profile.extend` will now get the intended removed-API diagnostic and must migrate to import + explicit Lua assignment.

Next:
- Mark Ticket done. Closure remains separate.
- Re-evaluate queued `00001KV11DHGZ` because its profile-surface conflict wait on this Ticket is now resolved.

---

<!-- event: state_changed author: orchestrator at: 2026-06-14T06:34:34Z from: inprogress to: done reason: merged_and_validated field: state -->

## State changed

Implementation branch was reviewed, approved, merged into the Orchestrator branch as `58b15ee6`, and validated in the Orchestrator worktree. Focused manifest profile tests, formatting, diff check, `cargo build -p yoi`, and `nix build .#yoi` passed after cleanup freed disk space. Ticket implementation work is done; closure remains separate.

---
