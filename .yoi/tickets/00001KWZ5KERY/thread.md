<!-- event: create author: "yoi ticket" at: 2026-07-07T20:51:35Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-08T09:03:55Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-08T09:03:55Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-08T09:11:22Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-08T09:12:55Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Decodal Profile DSL / `ProfileSourceArchive` / Backend archive builder / Runtime archive verify-cache / archive-only SourceLoader / Browser-Backend launch 経路からの Runtime-local FS profile discovery 排除について、binding invariants と acceptance criteria を具体的に列挙している。
- `00001KX0DSMPT` はこの Ticket に `depends_on` している incoming relation であり、この Ticket の blocker ではない。
- typed outgoing relation blocker は 0 件、OrchestrationPlan record は 0 件。
- queued notification は human authorized routing であり、今回の routing acceptance 後にのみ implementation side effect へ進める。
- Decodal API / dependency availability や既存 Lua profile compatibility の細部は bounded implementation investigation に閉じる。不足が設計境界変更に発展する場合は escalation 条件にする。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWZ5KERY)`: incoming `00001KX0DSMPT depends_on 00001KWZ5KERY` あり。outgoing blocker なし。
- `TicketOrchestrationPlanQuery(00001KWZ5KERY)`: 0 件。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: この Ticket 1件のみ。inprogress は 0 件。
- visible Pods: previous child Pods が idle で残っているが、`StopPod` 不使用方針のため停止せず、新規 unique Pod を使う。capacity blocker ではない。
- `TicketDoctor`: 0 errors / 既存 diagnostics のみ。
- Bounded code map: `crates/manifest/src/profile.rs`, `crates/worker/src/entrypoint.rs`, `crates/worker-runtime/src/{worker_backend.rs,runtime.rs,catalog.rs,config_bundle.rs,fs_store.rs,http_server.rs,main.rs}`, `crates/workspace-server/src/{hosts.rs,server.rs,companion.rs}`, builtin Lua profiles under `resources/profiles/*.lua`, profile discovery/setup code in `crates/tui` / `crates/worker`。

IntentPacket:

Intent:
- Browser/Workspace Backend Worker launch の通常経路を Lua/runtime-local filesystem profile discovery から切り離し、Backend が Decodal Profile source graph を `ProfileSourceArchive` tar artifact として構築し、Runtime が archive を verify/cache して archive-contained Decodal sources だけで Worker config を resolve する。

Binding decisions / invariants:
- Runtime は通常 Browser/Backend launch 経路で Workspace filesystem / WorkingDirectory root / `profile_base_dir` から `.yoi/profiles.toml`、Lua file、Lua local module、`.yoi/override.local.toml` を探索しない。
- WorkingDirectory は Worker execution root/cwd/scope 計算にのみ使う。
- Profile source archive と Runtime-local artifact sync は別境界。Plugin/MCP/sandbox/image/package bytes は archive に入れない。
- Memory / Ticket / Objective / repository contents / Worker catalog は Runtime sync 対象ではない。
- Browser-facing API は archive content、archive digest、Runtime store path、raw path、Runtime endpoint/token/socket/session path を出さない。
- `ProfileSourceArchive` manifest/source paths は relative only。absolute path、`..`、symlink/path escape、unsupported source、size/count/depth limit excess を拒否する。
- Decodal import resolution は archive manifest / import map に閉じる。Runtime SourceLoader は filesystem/backend ad-hoc fetch をしない。
- Lua Profile は compatibility-only path として残す場合も Browser/Backend launch 通常経路では使わない。

Requirements / acceptance criteria:
- Decodal dependency/schema により Worker launch config が表現できる。
- builtin role Profiles が Decodal source として利用できる。
- `ProfileSourceArchive` tar format と manifest schema が実装される。
- Backend は profile/role selector から archive を構築し、import closure/digests/archive digest/limits/path safety を検証する。
- Runtime は `ProfileSourceArchiveRef` から archive を prefetch / verify / cache / reuse できる。
- Runtime `ArchiveSourceLoader` は archive-contained source だけから Decodal imports を解決する。
- Worker creation は通常経路で archive-resolved config を使い、Runtime-local FS profile discovery を使わない。
- Worker metadata/audit に archive id/digest/source graph summary が残る。
- Missing profile / ambiguous selector / invalid Decodal / missing import / outside graph / artifact too large / digest mismatch / unsupported plugin package / missing runtime-local artifact が typed diagnostic になる。
- Focused tests が Backend archive build、Runtime archive verify/cache、ArchiveSourceLoader、Worker create without Runtime FS discovery、WorkingDirectory launch success、Browser payload redaction、compat fallback isolation を確認する。

Implementation latitude:
- Decodal source file extension/module layout/schema names、archive manifest field names、cache directory layout、diagnostic enum location、test fixture organization は既存 style に合わせてよい。
- Existing `ConfigBundle` machineryを置き換えるか、archive-resolved config の Runtime-side cache/availability boundaryとして再利用するかは、invariant を守る範囲で選んでよい。
- Compatibility-only Lua path は CLI/debug/test 用に残してよいが、通常 Browser/Backend launch path から分離されていることを tests/comments/diagnostics で明示する。
- Runtime-local artifact availability は full package manager 実装でなく typed check/sync boundary の分離を示す最小実装でよい。

Escalate if:
- Decodal crate/API が Worker config materialization に必要な capabilities を持たない、または semantic redesign が必要で Ticket の範囲を超える場合。
- Browser/Backend launch を raw path/runtime-local profile discovery に戻さないと成立しない場合。
- Secret synchronization、Plugin package manager/signature policy、multi-tenant auth、Profile editor UI、Runtime-local artifact sync full implementation が必要になる場合。
- Archive digest/content/runtime store path を Browser-facing API に出す必要が出た場合。

Validation:
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`（Ticket acceptance に含まれるため実行する。実行不能なら理由を implementation report に明記する）

Current code map:
- Profile discovery/resolution: `crates/manifest/src/profile.rs`, `crates/worker/src/entrypoint.rs` (`resolve_runtime_profile_manifest`)。
- Runtime Worker launch: `crates/worker-runtime/src/worker_backend.rs` (`ProfileRuntimeWorkerFactory`), `runtime.rs`, `catalog.rs`, `config_bundle.rs`, `fs_store.rs`, `http_server.rs`, `main.rs`。
- Backend/Runtime launch bridge: `crates/workspace-server/src/hosts.rs`, `server.rs`, `companion.rs`。
- Builtin profiles: `resources/profiles/*.lua` to migrate/create Decodal equivalents。
- CLI/profile compatibility surfaces: `crates/tui/src/spawn.rs`, `crates/tui/src/setup_model.rs`, `crates/worker/src/spawn/tool.rs`。

Critical risks / reviewer focus:
- Runtime Worker creation path must not silently keep reading `profile_base_dir` / WorkingDirectory / workspace filesystem in Browser/Backend launch。
- Archive verification must be path-safe, digest-checked, bounded, and deterministic。
- Decodal import graph must be closed by manifest/import map; no filesystem/backend dynamic import fallback。
- Browser-facing responses must not leak archive content/digest/runtime store/raw paths/endpoints/secrets。
- Builtin role Decodal sources must cover existing role behavior enough for Browser/Backend launch。
- Compatibility Lua fallback must be isolated and not accidentally used by ordinary Browser/Backend launch。
- Runtime-local artifact availability checks must stay separated from Profile source archive。

---

<!-- event: state_changed author: orchestrator at: 2026-07-08T09:13:16Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after Workspace Dashboard Queue notification authorized Orchestrator routing.

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWZ5KERY)`: incoming dependent `00001KX0DSMPT depends_on 00001KWZ5KERY` あり。this Ticket の blocking outgoing relation は 0 件。
- `TicketOrchestrationPlanQuery(00001KWZ5KERY)`: prior record 0 件。今回 accepted_plan を記録済み。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。
- visible Pods / TicketDoctor / bounded code map を確認。

Acceptance basis:
- concrete missing decision / information は残っていない。
- Ticket の scope は Decodal Profile DSL / ProfileSourceArchive / Runtime archive-only profile resolution / compatibility fallback isolation に明確化されており、Profile editor UI、secret sync、plugin package manager、runtime-local artifact sync full implementation は非目標として明確。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---
