<!-- event: create author: "yoi ticket" at: 2026-06-15T13:40:15Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T13:59:47Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T14:00:41Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- Prior Plugin package design `00001KT0Z4BK8` は done 済みで、本 Ticket はその設計を踏まえた discovery + explicit enablement resolver の最初の実装として具体化されている。
- Risk flags は plugin / package-loading / discovery / enablement / capability-boundary / startup-restore だが、non-goals と fail-closed / read-only / no-registration invariants が明確で、残る不確実性は typed module/config/resolver design の実装戦術に閉じている。

Evidence checked:
- Ticket body/thread: scope、requirements、non-goals、acceptance criteria、implementation notes、related work を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`425a6c66` 上。
- Visible Pods: implementation child Pod なし。
- Related design context: `00001KT0Z4BK8` done（Plugin package/discovery design）。

IntentPacket:

Intent:
- Plugin package discovery と explicit enablement resolver を typed module として実装し、package presence / discovery / enablement / runtime initialization / contribution registration を明確に分離する。

Binding decisions / invariants:
- Discovery は read-only。package の存在だけで execution / Tool / Hook / Service / Ingress registration を行わない。
- Explicit enablement entry がなければ Plugin は active にならない。
- Source-qualified identity (`user:<id>`, `project:<id>`, `builtin:<id>`) を扱い、ambiguous unqualified id は fail closed。
- Package safety checks は path traversal / root escape / bounded count/size / manifest size / deterministic digest を含む。
- unsupported/incompatible API version、digest mismatch、version mismatch、missing package、duplicate/ambiguous id、unsupported surface/grant は区別可能な diagnostic にする。
- Diagnostics を model-visible context に勝手に差し込まない。
- Plugin code execution / WASM runtime / actual Tool/Hook/Service/Ingress registration / MCP bridge は non-goal。
- No ambient workspace filesystem authority を plugin package discovery から発生させない。

Requirements / acceptance criteria:
- User store `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/*.yoi-plugin` と workspace store `<workspace>/.yoi/plugins/*.yoi-plugin` から package を発見できる。
- Valid package root の `plugin.toml` を parse し typed manifest と deterministic digest を得る。
- Invalid package は startup 全体を不要に壊さず bounded diagnostic で fail closed。
- Package without enablement is not active。
- Explicit enablement resolves package to typed resolved Plugin metadata。
- Tests cover valid user/workspace discovery、discovery-only inactive、explicit enablement、duplicate/ambiguous fail-closed、digest mismatch、path traversal/root escape、unsupported api、malformed manifest、no contribution registration。

Implementation latitude:
- Small typed module/crate-local module を追加してよい。runtime launch code に resolver logic を埋め込まない。
- `.yoi-plugin` archive vs directory minimal implementation は prior design に合わせる。必要なら最小サポート範囲を明示し後続拡張可能にする。
- Exact config/profile shape は既存 Profile / manifest design に合わせて最小 typed structure を追加してよい。
- Startup/restore reproducibility は deterministic re-resolution か resolved digest metadata 保持のどちらかを実装判断。ただし runtime-only mutable state 依存は不可。

Escalate if:
- Profile/manifest authority semantics、Pod restore semantics、secret handling、MCP enablement model を変える必要がある。
- Package archive implementation needs signature/trust/install/update/registry semantics。
- Arbitrary external filesystem/network authority が必要になる。
- Runtime registration/WASM execution なしでは acceptance を満たせないことが判明する。

Validation:
- focused tests for plugin discovery/resolver。
- `cargo fmt --check`。
- relevant `cargo check` / `cargo test`。
- `git diff --check`。
- `nix build .#yoi` if dependencies, runtime resources, packaging, or Cargo.lock changes matter。

Critical risks / reviewer focus:
- discovery vs enablement vs runtime/registration separation。
- fail-closed package safety / diagnostics。
- source-qualified identity and ambiguous refs。
- no contribution registration / no side effects from discovery。
- startup/restore determinism。
- secret-like diagnostic redaction。
- Plugin permission/grant requests not confused with actual grants。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T14:01:19Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / unresolved OrchestrationPlan blocker はなく、Plugin resolver work は同時に開始する Panel startup latency work と主対象が異なるため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KV5R5V2S at: 2026-06-15T14:27:54Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV5R5V2S`:

Commit:
- `a03a9da6 feat: add plugin package resolver`

Changed files:
- `crates/manifest/src/plugin.rs`
  - New typed plugin discovery/resolution module.
- `crates/manifest/src/lib.rs`
  - Added `plugins` config to `PodManifest`.
  - Added parsing test for `[[plugins.enabled]]`.
- `crates/manifest/src/config.rs`
  - Added `plugins` to `PodManifestConfig` and merge/materialization path.
- `crates/manifest/src/profile.rs`
  - Added profile/Lua config support for plugin enablement config.
- `crates/pod/src/spawn/tool.rs`
  - Preserves plugin config when deriving child manifest config.
- `crates/manifest/Cargo.toml`
  - Added `sha2` dependency for deterministic package digests.
- `Cargo.lock`
  - Updated manifest crate dependency metadata.
- `package.nix`
  - Updated `cargoHash`.

Resolver/config shape:
- Config supports explicit enablement through `[[plugins.enabled]]`:
  - `id = "project:<id>" | "user:<id>" | "builtin:<id>"`
  - optional `digest = "sha256:<hex>"`
  - optional `surfaces = ["hook", ...]`
  - optional `[plugins.enabled.config]`
  - optional grants, currently fail closed when authority-bearing/non-empty.
- Discovery stores:
  - User: `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/*.yoi-plugin`
  - Workspace: `<workspace>/.yoi/plugins/*.yoi-plugin`

Implemented behavior:
- Discovery is read-only and returns typed `DiscoveredPluginPackage` metadata only.
- Resolution only activates packages explicitly listed in config and returns typed `ResolvedPlugin` metadata.
- No runtime initialization, code execution, WASM runtime, hook/tool/service/ingress registration, network, package-manager, or MCP behavior was added.
- Source-qualified ids are required; unqualified ids fail closed as ambiguous.
- Unsupported plugin API versions fail closed.
- Digest pins are checked against deterministic package content digests.
- Traversal/root escape/symlink escape checks are enforced.
- Package count, package size, manifest size, entry count, per-file size, and expanded size are bounded.
- Non-empty authority-bearing grants fail closed, so plugins do not receive ambient workspace filesystem authority.

Diagnostics:
- Added typed diagnostic kinds for missing, duplicate, ambiguous, version, digest, API, surface, grant, malformed, traversal, bounds, and I/O.
- Diagnostics avoid full package path/content disclosure and use bounded/sanitized messages.

Validation reported by coder:
- Passed: `cargo fmt --check`
- Passed: `cargo test -p manifest plugin`
- Passed: `cargo test -p manifest`
- Passed: `cargo check`
- Passed: `cargo test -p pod spawn_pod`
- Passed: `git diff --check`
- Passed: `nix build .#yoi`

Residual risks / notes:
- Current archive reader intentionally supports a constrained stored-ZIP profile and fails closed on unsupported compression methods. Broader `.zip` compression support can be added later if package tooling requires it.

---

<!-- event: review author: yoi-reviewer-00001KV5R5V2S at: 2026-06-15T14:37:12Z status: request_changes -->

## Review: request changes

Review result: request_changes

Evidence checked:
- Child worktree only:
  - `/home/hare/Projects/yoi/.worktree/00001KV5R5V2S-plugin-enable-resolver`
  - branch/head `impl/00001KV5R5V2S-plugin-enable-resolver` at `a03a9da6`
- Diff files:
  - `crates/manifest/src/plugin.rs`
  - `crates/manifest/src/config.rs`
  - `crates/manifest/src/lib.rs`
  - `crates/manifest/src/profile.rs`
  - `crates/pod/src/spawn/tool.rs`
  - `crates/manifest/Cargo.toml`
  - `Cargo.lock`
  - `package.nix`
  - `docs/design/plugin-packages.md`
- Ticket record:
  - `.yoi/tickets/00001KV5R5V2S/item.md`
  - `.yoi/tickets/00001KV5R5V2S/thread.md`

Read-only validation performed:
- Passed: `git diff --check 4772c4d6..a03a9da6`

Positive findings:
- Discovery/resolution code is isolated in `crates/manifest/src/plugin.rs`.
- No runtime registration, WASM execution, Tool/Hook/Service/Ingress contribution path was found.
- User/workspace stores are represented.
- Discovery checks include store containment, symlink escape rejection, path normalization, count/size/manifest bounds, duplicate normalized path rejection, and deterministic digest.
- Package presence alone does not activate anything; resolution requires `plugins.enabled`.
- Manifest/profile/child-spawn config plumbing preserves `plugins` config.

Required changes:

1. Version mismatch support is missing.

- Ticket requires enablement entries to express package version/version constraint and requires version mismatch to be a distinct diagnostic.
- `PluginEnablementConfig` currently has `id`, `digest`, `surfaces`, `grants`, `config`, but no version/version constraint field.
- `resolve_enabled_plugins` never compares enablement against `package.manifest.version`.
- `PluginPackageManifest` has `version`, but it is only validated non-empty.
- `PluginDiagnosticKind::Version` currently appears to be used for unsupported API version, so package-version mismatch and API incompatibility are not clearly separated.

Required fix:
- Add a typed version/version requirement field to enablement config, or explicitly documented minimal exact-version field if constraints are deferred.
- Compare it to `manifest.version` during resolution.
- Emit a distinct version-mismatch diagnostic separate from incompatible API version.
- Add tests for version mismatch fail-closed behavior.

2. Startup/restore determinism is not satisfied.

- Ticket requires deterministic startup/restore behavior for the resolved plugin set.
- Implementation preserves authoring config, but no resolved plugin metadata/digest recording or deterministic restore re-resolution path was found.
- Unpinned enablement can resolve to a different package if mutable user/workspace store changes before restore.
- The design doc also states restore should use a resolved plan, not fresh discovery choosing newer packages.

Required fix:
- Either persist resolved plugin identity/digest metadata into resolved manifest/session metadata used for restore, or define and implement deterministic re-resolution semantics that cannot silently change a restored plugin set.
- Add focused test or validation evidence for the chosen restore/reproducibility path.
- If intentionally deferred, Ticket acceptance/report must be updated before approval because current acceptance still requires it.

3. Bounded diagnostic truncation can panic on valid UTF-8.

- `bounded_message` slices a Rust `String` at byte offset 240.
- It is used for TOML parse errors from untrusted plugin manifests.
- If byte 240 falls inside a multibyte UTF-8 character, slicing panics instead of producing a bounded diagnostic.

Required fix:
- Truncate on character boundary using safe helper logic.
- Add malformed manifest test with long multibyte content proving diagnostics remain bounded and non-panicking.
- Consider reducing raw TOML-error content leakage because diagnostics should avoid secret-like path/content leakage.

Additional concern:
- Design doc examples use `schema_version`, but implemented parser requires `api_version`.
- Align schema naming before merge to avoid contradictory package-author guidance.

Conclusion:
- Changes requested. Do not integrate until these blockers are fixed and covered.

---

<!-- event: implementation_report author: yoi-coder-00001KV5R5V2S at: 2026-06-15T14:53:21Z -->

## Implementation report

Follow-up implementation report after reviewer request_changes:

New commit:
- `ede7acfd fix: pin plugin resolution metadata`

Resolved blockers:

1. Version mismatch support
- Added typed exact-version requirement:
  - `PluginExactVersion`
  - `PluginEnablementConfig.version: Option<PluginExactVersion>`
- Resolution now compares `plugins.enabled.version` against `package.manifest.version`.
- Version mismatch fails closed with `PluginDiagnosticKind::Version`.
- Unsupported plugin schema/API version now emits `PluginDiagnosticKind::Api`, distinct from package version mismatch.
- Added focused test:
  - `exact_version_mismatch_fails_closed_with_distinct_diagnostic`

2. Startup/restore determinism
- Added runtime resolved-plan metadata to `PluginConfig`:
  - `resolved: Vec<ResolvedPluginRecord>`
  - `diagnostics: Vec<PluginDiagnostic>`
- Added `resolve_plugin_config_for_startup`.
- Fresh startup resolves enabled plugins and records identity/path/label/digest/version/manifest/enabled surfaces/grants/config into manifest plugin config.
- Restore/snapshot configs with an existing resolved plan are reused as-is and do not silently select newer mutable store contents.
- Wired startup resolution into pod manifest resolution in `crates/pod/src/entrypoint.rs`.
- Preserved resolved plugin plan through config merge.
- Added focused test:
  - `resolved_plan_pins_unpinned_enablement_for_restore`

3. Safe bounded diagnostics
- Replaced byte slicing in `bounded_message` with char-boundary truncation.
- TOML parse diagnostics no longer embed raw untrusted manifest content; diagnostic reports bounded parse message with optional byte span.
- Added focused multibyte malformed manifest test:
  - `malformed_manifest_multibyte_diagnostic_is_bounded_and_redacted`

4. Schema naming alignment
- Plugin package manifest parser now uses `schema_version`, matching `docs/design/plugin-packages.md`.
- Updated tests/helpers accordingly.
- Updated design doc enablement example to include implemented exact `version` field and describe deferred richer constraints.

Validation reported by coder:
- Passed: `cargo fmt --check`
- Passed: `cargo test -p manifest plugin`
- Passed: `cargo test -p manifest`
- Passed: `cargo check`
- Passed: `cargo test -p pod spawn_pod`
- Passed: `git diff --check`
- Passed: `nix build .#yoi`

Repository status:
- Child implementation worktree clean after follow-up commit.

Residual note:
- Version constraints are intentionally minimal exact-version matching for this Ticket. Richer semantic/range constraints remain deferred and documented as follow-up scope.

---

<!-- event: review author: yoi-reviewer-00001KV5R5V2S-r2 at: 2026-06-15T15:05:55Z status: request_changes -->

## Review: request changes

Review result: request_changes

Evidence checked:
- Child worktree only:
  - `/home/hare/Projects/yoi/.worktree/00001KV5R5V2S-plugin-enable-resolver`
  - HEAD `ede7acfd fix: pin plugin resolution metadata`
  - base `4772c4d6`
- Relevant files:
  - `crates/manifest/src/plugin.rs`
  - `crates/pod/src/entrypoint.rs`
  - `crates/pod/src/pod.rs`
  - `docs/design/plugin-packages.md`

Validation performed by reviewer:
- Passed: `git diff --check 4772c4d6..HEAD`
- Passed: `cargo fmt --check`
- `git status --short` clean

Validation not run:
- `cargo test`, `cargo check`, and `nix build` were not rerun because reviewer scope was read-only and those commands write build artifacts. Coder-reported results were treated as evidence only.

Remaining blockers:

1. Startup/restore determinism is incomplete for non-profile / spawn-config launches.

Positive pieces exist:
- `apply_plugin_resolution_plan()` resolves plugins during manifest resolution.
- `resolve_plugin_config_for_startup()` skips fresh discovery when `plugins.resolved` or `plugins.diagnostics` already exists.
- focused helper test confirms a restored resolved plan does not pick newer mutable store contents.

Blocker:
- resolved manifest snapshot is only persisted to Pod metadata when `self.manifest.profile.is_some()`.
- Restore uses saved snapshot if present, otherwise falls back to freshly resolved current manifest.
- A named Pod started from `--manifest` or spawn config can resolve plugin metadata at startup but fail to persist that resolved plan, then later restore can silently re-resolve against newer mutable store contents unless authoring config had digest pin.

Required fix:
- Persist the resolved manifest snapshot for plugin-resolved manifests regardless of `manifest.profile`, or otherwise make non-profile restore deterministically use the original resolved plan.
- Add focused test/validation for the runtime persistence path, not only the helper behavior.

2. Documentation examples still appear broader than the implemented strict parser.

Resolved part:
- `schema_version` naming is now consistent in implementation/tests/minimal docs.

Remaining mismatch:
- docs illustrative `plugin.toml` still includes fields/sections rejected by current `#[serde(deny_unknown_fields)]` parser, such as `[package]` and `[permissions]`.
- docs describe `runtime.kind = "declarative"` as an initial value while implementation rejects runtime kinds other than `"wasm"`.

Required fix:
- Either mark those fields/sections as future/aspirational and provide a minimal currently-valid manifest example, or extend the parser to accept the documented first-pass fields safely.

Prior blockers resolved:
- Version mismatch support is resolved with exact-version field and distinct `Version` vs `Api` diagnostics.
- Safe bounded diagnostics are mostly resolved with UTF-8-boundary truncation and redacted TOML parse diagnostics.
- Discovery/enablement/no-registration boundaries look good.

Conclusion:
- Changes requested. Do not integrate until the non-profile/spawn restore determinism path and docs/parser mismatch are fixed and covered.

---
