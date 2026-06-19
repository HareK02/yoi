<!-- event: create author: "yoi ticket" at: 2026-06-19T07:53:13Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-19T10:19:52Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T10:22:26Z -->

## Plan

Routing hold / waiting-capacity note:

明示 queue review で Ticket body / relations / orchestration plan / workspace state を確認した。

Current decision:
- この Ticket は `queued` のまま待機。
- 依存 Ticket `00001KV5W3PHW` / `00001KV5W3PJ3` は closed で dependency blocker ではない。
- ただし同時 queued の `00001KVFD3YSV` CLI inspection を先に受理した。CLI inspection は Plugin discovery / enablement / grants / diagnostics の read-only public surface を作る作業で、host API implementation と同じ Plugin manifest/grant/runtime/diagnostic 周辺に触れる。
- `00001KVFDX9AF` https host API とは WASM Plugin Tool runtime host import boundary、Plugin grant model、diagnostics/tests/package behavior の変更面が重なるため `do_not_parallelize` plan record を残した。

Bounded reason for idle queued:
- conflict / reviewer-coder bottleneck。

Next action:
- `00001KVFD3YSV` の implementation/review/merge outcome を確認後、queued のまま再 routing する。
- その時点で `https` host API Ticket との ordering / conflict も再確認する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-19T15:37:24Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body には、`fs` host API intent、binding invariants、acceptance criteria、non-goals、validation、escalation-worthy risk domain が実装可能な粒度で揃っている。
- 依存 relation の `00001KV5W3PHW` minimal WASM runtime、`00001KV5W3PJ3` permission grants、関連 `00001KVFD3YSV` CLI inspection、`00001KVFDX9AF` HTTPS host API は closed で blocker ではない。
- Risk domain は filesystem / path safety / file mutation / permission grants だが、Ticket は Plugin-specific grants、no workspace authority inheritance、path normalization、traversal/symlink/root escape rejection、bounds、safe diagnostics、ordinary Tool result path を binding invariants として明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。
- `00001KVG0HR96` Component Model migration は Plugin runtime / WIT / host API shape / grants / inspection / packaging に広く触れる migration boundary で、active `fs` host API と衝突しやすいため waiting note を更新し queued のまま待機する。

Evidence checked:
- Ticket `00001KVFDX9AY` body / thread / artifacts。
- `TicketRelationQuery(00001KVFDX9AY)`: depends_on は closed。related Ticket は context であり acceptance blocker ではない。
- `TicketOrchestrationPlanQuery(00001KVFDX9AY)`: prior waiting/do_not_parallelize records を確認。HTTPS host API は closed になったため今回 `accepted_plan` を記録済み。
- Related completed Tickets:
  - `00001KV5W3PHW` — minimal WASM Tool runtime closed。
  - `00001KV5W3PJ3` — Plugin permission grants closed。
  - `00001KVFD3YSV` — Plugin read-only CLI inspection closed。
  - `00001KVFDX9AF` — Plugin HTTPS host API closed。
- Current queued Ticket `00001KVG0HR96` Component Model migration: migration boundary / conflict waiting note を更新。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`: clean。
- Existing branch/worktree: matching `00001KVFDX9AY` branch/worktree はなし。
- Visible Pods: self / peers only; spawned child capacity is free。
- Current code map:
  - `crates/pod/src/feature/plugin.rs`: Plugin resolver, permission grants, static inspection, host API eligibility, HTTPS implementation pattern。
  - `crates/pod/src/pod.rs`: WASM Tool runtime / host import validation / Tool execution path。
  - `crates/manifest/src/plugin.rs`: Plugin manifest and permission model。
  - `crates/yoi/src/plugin_cli.rs`: read-only inspection output should remain compatible with fs host API diagnostics。

IntentPacket:

Intent:
- WASM Plugin Tool runtime に、明示 grant された scoped path のみ read/list/write できる `fs` host API を追加する。
- Plugin は Pod/workspace filesystem authority を自動継承せず、Plugin-specific `host_api.fs` grants だけが filesystem authority になる。

Binding decisions / invariants:
- Host API name/domain は `fs`。
- Broad WASI filesystem exposure は禁止。Plugin は ambient filesystem access を持たない。
- Workspace read/write authority は Plugin に自動継承しない。
- Grant がない read/list/write は fail closed。
- Grants は operation kind (`read`, `list`, `write`) と scoped root/prefix/glob 等の最小安全形を持つ。
- Path normalization、`..` traversal rejection、symlink/root escape rejection、allowed root outside rejection は binding。
- Absolute/relative path policy は明確にし、safe default を選ぶ。
- Bounds: path length、read size、write size、directory entry count、diagnostic size。
- Writes は existing file mutation safety と整合し、normalized target file ごとに unsafe race を避ける。
- Diagnostics に file content / secret-like data を漏らさない。
- Tool result path は ordinary Tool result/history path。hidden context injection しない。
- `https` host API、Service/Ingress/File watcher/package manager は non-goals。

Requirements / acceptance criteria:
- Granted Plugin Tool can read an allowed file。
- Granted Plugin Tool can list an allowed directory within bounds。
- Granted Plugin Tool can write an allowed file within bounds。
- Plugin without matching `host_api.fs` grant cannot read/list/write。
- Workspace authority is not inherited by Plugin without Plugin grant。
- `../` traversal、symlink escape、allowed-root escape reject。
- Oversize read/write/list fail closed or truncate according to explicit policy。
- File mutation safety avoids unsafe race with existing Write/Edit semantics。
- Diagnostics do not include file content or secret-like data。
- Tests cover allowed read/list/write, missing grant denied, workspace authority not inherited, traversal/symlink/root escape, bounds, diagnostics redaction, safe write conflict behavior。

Implementation latitude:
- Choose exact ABI/import shape consistent with existing `yoi-plugin-wasm-1` host import design and current HTTPS host API pattern。
- Choose narrow grant config representation for root/prefix/glob/operation allowlist consistent with current Plugin permission grant model。
- Use tempdir/local fixture files for deterministic tests。
- Choose read/list/write response shape consistent with existing Tool result/error types and CLI inspection structure。
- If write serialization requires reusing existing file mutation primitives, keep it narrow and avoid broad Worker scheduler changes。

Escalate if:
- Safe path/symlink/root escape handling cannot be represented without broad filesystem authority redesign。
- write serialization requires broad Worker scheduler or global mutation system redesign。
- Existing Plugin grant schema cannot safely represent fs scopes without breaking HTTPS grants/CLI inspection。
- Broad WASI filesystem exposure appears necessary。
- Product decision is needed for truncate-vs-fail policy beyond Ticket’s bounded latitude。

Validation:
- Focused plugin fs host API tests。
- Relevant `cargo test` / `cargo check` for `pod`, `manifest`, `yoi` as changed。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi --no-link` / `nix build .#yoi` if dependency/package-source-filter changes occur。

Critical risks / reviewer focus:
- Workspace authority leaking into Plugin without Plugin grant。
- Path traversal / symlink / root escape bypass。
- Write race / unsafe mutation behavior。
- File content or secret leakage in diagnostics。
- Unbounded read/list/write outputs。
- Hidden context injection by bypassing normal Tool result path。
- Breaking existing HTTPS host API, permission grants, or CLI inspection semantics。

Next action:
- `queued -> inprogress` を記録し、Ticket records を Orchestrator worktree に commit してから、専用 implementation worktree を作成し Coder Pod を narrow write scope で起動する。root/original workspace は操作しない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-19T15:37:38Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_fs_host_api field: state -->

## State changed

Ticket body/thread, relation metadata, orchestration plan records, related completed Tickets, Orchestrator worktree, visible Pods, existing branch/worktree, and bounded Plugin fs host API code context were checked. Depends-on blockers are closed, HTTPS host API and CLI inspection related work are closed, and no dirty-state blocker or missing planning decision was found. Component Model migration remains queued with migration/conflict waiting record. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T15:39:09Z -->

## Implementation report

Implementation start note:

`queued -> inprogress` acceptance、accepted plan、routing decision / IntentPacket、Component Model migration の waiting record を記録し、Orchestrator worktree で commit した後に、専用 implementation worktree と Coder Pod を起動した。

Worktree:
- `/home/hare/Projects/yoi/.worktree/00001KVFDX9AY-plugin-fs-host-api`
- branch: `impl/00001KVFDX9AY-plugin-fs-host-api`

Coder Pod:
- `yoi-coder-00001KVFDX9AY`

Scope / boundaries:
- child runtime workspace root は read scope。
- implementation worktree は write scope。
- root/original workspace と Orchestrator worktree へは書き込まないよう指示済み。
- `.yoi/memory`、runtime/log/lock/local/secret-like `.yoi` paths は編集しないよう指示済み。

Related queued Ticket:
- `00001KVG0HR96` Component Model migration remains queued due migration boundary / conflict with active fs host API work.

Next:
- Coder は scoped `fs` host API implementation、focused tests、validation、implementation commit を行い、実装報告を返す予定。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T16:01:41Z -->

## Implementation report

Coder implementation report received / Orchestrator validation:

Implementation commit:
- `717c0999` — `plugin: implement fs host api`

Summary:
- Added manifest/config support for `host_api.fs` grants:
  - `PluginFsGrant { root, operations }`
  - `PluginFsOperation::{read,list,write}`
- Implemented WASM host import module `yoi:fs` with:
  - `read`
  - `list`
  - `write`
  - `response_len`
  - `response_read`
- Enforced plugin-specific grant checks; Pod/workspace fs authority is not inherited.
- Added path safety:
  - relative-only request paths;
  - `..` traversal rejection;
  - configured root must be absolute/non-symlink directory;
  - symlink component/target rejection;
  - canonical allowed-root escape checks.
- Added bounds:
  - request descriptor size;
  - path length;
  - read size with explicit truncation flag;
  - write size fail-closed;
  - directory entry count with explicit truncation flag.
- Added write serialization by normalized target plus `O_NOFOLLOW` on Unix.
- Kept fs errors/diagnostics bounded and redacted.
- Extended Plugin CLI inspection to show configured fs grants.

Files changed:
- `crates/manifest/src/plugin.rs`
- `crates/pod/src/feature/plugin.rs`
- `crates/yoi/src/plugin_cli.rs`

Coder reported validation:
- `cargo fmt --check` — passed
- `cargo test -p pod feature::plugin` — passed
- `cargo test -p manifest plugin` — passed
- `cargo test -p yoi plugin_cli` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `git diff --check` — passed
- `nix build` not run by Coder because no dependency/package-source-filter changes.

Orchestrator evidence checked:
- Pod output を確認。
- implementation worktree は clean。
- branch `impl/00001KVFDX9AY-plugin-fs-host-api` HEAD は `717c0999`。
- diff stat from accepted base `6711bcf3..717c0999`: 3 files changed, 1068 insertions, 17 deletions。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin -- --nocapture` — passed; 48 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- recorded intent / invariants / acceptance criteria に照らして、read-only Reviewer Pod で外部レビューする。
- 特に no ambient workspace fs authority、grant-before-filesystem access、path traversal/symlink/root escape、bounds/truncation/fail-closed policy、write serialization/O_NOFOLLOW、diagnostic redaction、ordinary Tool result path、HTTPS/CLI compatibility を確認する。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T16:02:39Z -->

## Plan

Review dispatch:

Reviewer Pod `yoi-reviewer-00001KVFDX9AY` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVFDX9AY-plugin-fs-host-api`
- branch: `impl/00001KVFDX9AY-plugin-fs-host-api`
- commit: `717c0999`

Reviewer focus:
- recorded intent / invariants / acceptance criteria に対する適合性。
- filesystem access が grant checks の前に発生しないこと。
- Workspace/POD filesystem authority が Plugin に自動継承されていないこと。
- traversal / symlink / root escape protections が canonicalization races を含め妥当であること。
- read/list/write bounds と truncate-vs-fail-closed policy が明確で enforce されること。
- write serialization by normalized target と `O_NOFOLLOW` が既存 file mutation safety と整合すること。portability issue がないこと。
- diagnostics に file content / secret-like data が漏れないこと。
- ordinary Tool result/history path だけに戻ること。
- tests が allowed read/list/write、missing grant、no workspace inheritance、traversal、symlink/root escape、bounds、redaction、write safety を十分に覆うこと。
- manifest/permission grant schema と CLI inspection changes が既存 HTTPS grants / Plugin inspection behavior と互換であること。
- HTTPS host API と Component Model migration が non-goals として保たれていること。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree への merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AY at: 2026-06-19T16:08:53Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AY`
- branch: `impl/00001KVFDX9AY-plugin-fs-host-api`
- commit: `717c0999` (`plugin: implement fs host api`)

No blockers found.

Evidence reviewed:
- FS grant schema is explicit and additive: `PluginGrantConfig.fs`, `PluginFsGrant { root, operations }`, and `PluginFsOperation::{read,list,write}` in `crates/manifest/src/plugin.rs`. Existing HTTPS grant shape remains separate.
- Runtime dispatch checks `host_api.fs` before request parsing reaches any filesystem operation: `execute_plugin_fs_request` bounds descriptor size, then calls `authorize_plugin_host_api(record, PluginHostApi::Fs)` before read/list/write dispatch.
- Permission/grant binding is layered correctly:
  - requested `host_api.fs` permission required;
  - configured grant permission required;
  - usable FS scope required;
  - per-operation grants checked in `authorize_fs_path` / `authorize_fs_write_path`.
- No ambient WASI/workspace filesystem authority is exposed. WASM import validation only allows `yoi:tool`, `yoi:https`, and `yoi:fs`; WASI remains rejected. Runtime FS is only through `yoi:fs` imports.
- Path safety is materially addressed:
  - absolute paths, NULs, and `..` traversal rejected;
  - grant root must be absolute, existing, directory, and not final symlink;
  - request path components are symlink-checked;
  - canonicalized targets must remain under the canonical grant root;
  - write target symlinks/dirs rejected;
  - Unix writes add `O_NOFOLLOW`.
- Bounds are explicit:
  - request descriptor, path, read bytes, write bytes, list entry count, and diagnostics are bounded/redacted;
  - read/list truncation is explicit via `truncated`;
  - oversize write fails closed.
- Writes serialize by normalized/canonical target with a shared lock map and use truncate/create with `sync_all`.
- Diagnostics use `PluginFsError::new` with `bounded_message(redact_secret_like(...))`; tests verify secret-like path redaction and no file-content leakage on denial.
- FS responses flow through WASM host response buffers and then ordinary plugin Tool output/error handling; no hidden context/history injection path found.
- CLI inspection reports `configured_fs_grants` alongside existing permission/HTTPS grant inspection without changing HTTPS semantics.

Reviewer validation:
- Static review of changed files and relevant tests.
- Confirmed worktree branch/head.
- `git diff --check 717c0999^..717c0999` — passed.
- Orchestrator-reported validation was reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - `cargo test -p pod feature::plugin`
  - `cargo test -p manifest plugin`
  - `cargo test -p yoi plugin_cli`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Non-blocking risk noted:
- The implementation is still fundamentally path/canonicalization based. It rejects static symlink/root escapes and uses Unix `O_NOFOLLOW` for final write target, but it does not fully eliminate adversarial external TOCTOU races where another local process mutates granted-root directories between checks and open/read/list/write. This is judged within the “feasible scope” of the initial subset and not a blocker for this Ticket, but should be preserved as future hardening context if the threat model expands.

Conclusion:
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AY at: 2026-06-19T16:09:17Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AY`
- branch: `impl/00001KVFDX9AY-plugin-fs-host-api`
- commit: `717c0999` (`plugin: implement fs host api`)

No blockers found.

Evidence reviewed:
- FS grant schema is explicit and additive: `PluginGrantConfig.fs`, `PluginFsGrant { root, operations }`, and `PluginFsOperation::{read,list,write}` in `crates/manifest/src/plugin.rs`. Existing HTTPS grant shape remains separate.
- Runtime dispatch checks `host_api.fs` before request parsing reaches any filesystem operation: `execute_plugin_fs_request` bounds descriptor size, then calls `authorize_plugin_host_api(record, PluginHostApi::Fs)` before read/list/write dispatch.
- Permission/grant binding is layered correctly:
  - requested `host_api.fs` permission required;
  - configured grant permission required;
  - usable FS scope required;
  - per-operation grants checked in `authorize_fs_path` / `authorize_fs_write_path`.
- No ambient WASI/workspace filesystem authority is exposed. WASM import validation only allows `yoi:tool`, `yoi:https`, and `yoi:fs`; WASI remains rejected. Runtime FS is only through `yoi:fs` imports.
- Path safety is materially addressed:
  - absolute paths, NULs, and `..` traversal rejected;
  - grant root must be absolute, existing, directory, and not final symlink;
  - request path components are symlink-checked;
  - canonicalized targets must remain under the canonical grant root;
  - write target symlinks/dirs rejected;
  - Unix writes add `O_NOFOLLOW`.
- Bounds are explicit:
  - request descriptor, path, read bytes, write bytes, list entry count, and diagnostics are bounded/redacted;
  - read/list truncation is explicit via `truncated`;
  - oversize write fails closed.
- Writes serialize by normalized/canonical target with a shared lock map and use truncate/create with `sync_all`.
- Diagnostics use `PluginFsError::new` with `bounded_message(redact_secret_like(...))`; tests verify secret-like path redaction and no file-content leakage on denial.
- FS responses flow through WASM host response buffers and then ordinary plugin Tool output/error handling; no hidden context/history injection path found.
- CLI inspection reports `configured_fs_grants` alongside existing permission/HTTPS grant inspection without changing HTTPS semantics.

Reviewer validation:
- Static review of changed files and relevant tests.
- Confirmed worktree branch/head.
- `git diff --check 717c0999^..717c0999` — passed.
- Orchestrator-reported validation was reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - `cargo test -p pod feature::plugin`
  - `cargo test -p manifest plugin`
  - `cargo test -p yoi plugin_cli`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Non-blocking risk noted:
- The implementation is still fundamentally path/canonicalization based. It rejects static symlink/root escapes and uses Unix `O_NOFOLLOW` for final write target, but it does not fully eliminate adversarial external TOCTOU races where another local process mutates granted-root directories between checks and open/read/list/write. This is judged within the feasible scope of the initial subset and not a blocker for this Ticket, but should be preserved as future hardening context if the threat model expands.

Conclusion:
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AY at: 2026-06-19T16:09:50Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AY`
- branch: `impl/00001KVFDX9AY-plugin-fs-host-api`
- commit: `717c0999` (`plugin: implement fs host api`)

No blockers found.

Evidence reviewed:
- FS grant schema is explicit and additive: `PluginGrantConfig.fs`, `PluginFsGrant { root, operations }`, and `PluginFsOperation::{read,list,write}` in `crates/manifest/src/plugin.rs`. Existing HTTPS grant shape remains separate.
- Runtime dispatch checks `host_api.fs` before request parsing reaches any filesystem operation: `execute_plugin_fs_request` bounds descriptor size, then calls `authorize_plugin_host_api(record, PluginHostApi::Fs)` before read/list/write dispatch.
- Permission/grant binding is layered correctly:
  - requested `host_api.fs` permission required;
  - configured grant permission required;
  - usable FS scope required;
  - per-operation grants checked in `authorize_fs_path` / `authorize_fs_write_path`.
- No ambient WASI/workspace filesystem authority is exposed. WASM import validation only allows `yoi:tool`, `yoi:https`, and `yoi:fs`; WASI remains rejected. Runtime FS is only through `yoi:fs` imports.
- Path safety is materially addressed:
  - absolute paths, NULs, and `..` traversal rejected;
  - grant root must be absolute, existing, directory, and not final symlink;
  - request path components are symlink-checked;
  - canonicalized targets must remain under the canonical grant root;
  - write target symlinks/dirs rejected;
  - Unix writes add `O_NOFOLLOW`.
- Bounds are explicit:
  - request descriptor, path, read bytes, write bytes, list entry count, and diagnostics are bounded/redacted;
  - read/list truncation is explicit via `truncated`;
  - oversize write fails closed.
- Writes serialize by normalized/canonical target with a shared lock map and use truncate/create with `sync_all`.
- Diagnostics use `PluginFsError::new` with `bounded_message(redact_secret_like(...))`; tests verify secret-like path redaction and no file-content leakage on denial.
- FS responses flow through WASM host response buffers and then ordinary plugin Tool output/error handling; no hidden context/history injection path found.
- CLI inspection reports `configured_fs_grants` alongside existing permission/HTTPS grant inspection without changing HTTPS semantics.

Reviewer validation:
- Static review of changed files and relevant tests.
- Confirmed worktree branch/head.
- `git diff --check 717c0999^..717c0999` — passed.
- Orchestrator-reported validation was reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - `cargo test -p pod feature::plugin`
  - `cargo test -p manifest plugin`
  - `cargo test -p yoi plugin_cli`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Non-blocking risk noted:
- The implementation is still fundamentally path/canonicalization based. It rejects static symlink/root escapes and uses Unix `O_NOFOLLOW` for final write target, but it does not fully eliminate adversarial external TOCTOU races where another local process mutates granted-root directories between checks and open/read/list/write. This is judged within the feasible scope of the initial subset and not a blocker for this Ticket, but should be preserved as future hardening context if the threat model expands.

Conclusion:
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AY at: 2026-06-19T16:10:26Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AY`
- branch: `impl/00001KVFDX9AY-plugin-fs-host-api`
- commit: `717c0999` (`plugin: implement fs host api`)

No blockers found.

Evidence reviewed:
- FS grant schema is explicit and additive: `PluginGrantConfig.fs`, `PluginFsGrant { root, operations }`, and `PluginFsOperation::{read,list,write}` in `crates/manifest/src/plugin.rs`. Existing HTTPS grant shape remains separate.
- Runtime dispatch checks `host_api.fs` before request parsing reaches any filesystem operation: `execute_plugin_fs_request` bounds descriptor size, then calls `authorize_plugin_host_api(record, PluginHostApi::Fs)` before read/list/write dispatch.
- Permission/grant binding is layered correctly:
  - requested `host_api.fs` permission required;
  - configured grant permission required;
  - usable FS scope required;
  - per-operation grants checked in `authorize_fs_path` / `authorize_fs_write_path`.
- No ambient WASI/workspace filesystem authority is exposed. WASM import validation only allows `yoi:tool`, `yoi:https`, and `yoi:fs`; WASI remains rejected. Runtime FS is only through `yoi:fs` imports.
- Path safety is materially addressed:
  - absolute paths, NULs, and `..` traversal rejected;
  - grant root must be absolute, existing, directory, and not final symlink;
  - request path components are symlink-checked;
  - canonicalized targets must remain under the canonical grant root;
  - write target symlinks/dirs rejected;
  - Unix writes add `O_NOFOLLOW`.
- Bounds are explicit:
  - request descriptor, path, read bytes, write bytes, list entry count, and diagnostics are bounded/redacted;
  - read/list truncation is explicit via `truncated`;
  - oversize write fails closed.
- Writes serialize by normalized/canonical target with a shared lock map and use truncate/create with `sync_all`.
- Diagnostics use `PluginFsError::new` with `bounded_message(redact_secret_like(...))`; tests verify secret-like path redaction and no file-content leakage on denial.
- FS responses flow through WASM host response buffers and then ordinary plugin Tool output/error handling; no hidden context/history injection path found.
- CLI inspection reports `configured_fs_grants` alongside existing permission/HTTPS grant inspection without changing HTTPS semantics.

Reviewer validation:
- Static review of changed files and relevant tests.
- Confirmed worktree branch/head.
- `git diff --check 717c0999^..717c0999` — passed.
- Orchestrator-reported validation was reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - `cargo test -p pod feature::plugin`
  - `cargo test -p manifest plugin`
  - `cargo test -p yoi plugin_cli`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Non-blocking risk noted:
- The implementation is still fundamentally path/canonicalization based. It rejects static symlink/root escapes and uses Unix `O_NOFOLLOW` for final write target, but it does not fully eliminate adversarial external TOCTOU races where another local process mutates granted-root directories between checks and open/read/list/write. This is judged within the feasible scope of the initial subset and not a blocker for this Ticket, but should be preserved as future hardening context if the threat model expands.

Conclusion:
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AY at: 2026-06-19T16:11:01Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AY`
- branch: `impl/00001KVFDX9AY-plugin-fs-host-api`
- commit: `717c0999` (`plugin: implement fs host api`)

No blockers found.

Evidence reviewed:
- FS grant schema is explicit and additive: `PluginGrantConfig.fs`, `PluginFsGrant { root, operations }`, and `PluginFsOperation::{read,list,write}` in `crates/manifest/src/plugin.rs`. Existing HTTPS grant shape remains separate.
- Runtime dispatch checks `host_api.fs` before request parsing reaches any filesystem operation: `execute_plugin_fs_request` bounds descriptor size, then calls `authorize_plugin_host_api(record, PluginHostApi::Fs)` before read/list/write dispatch.
- Permission/grant binding is layered correctly:
  - requested `host_api.fs` permission required;
  - configured grant permission required;
  - usable FS scope required;
  - per-operation grants checked in `authorize_fs_path` / `authorize_fs_write_path`.
- No ambient WASI/workspace filesystem authority is exposed. WASM import validation only allows `yoi:tool`, `yoi:https`, and `yoi:fs`; WASI remains rejected. Runtime FS is only through `yoi:fs` imports.
- Path safety is materially addressed:
  - absolute paths, NULs, and `..` traversal rejected;
  - grant root must be absolute, existing, directory, and not final symlink;
  - request path components are symlink-checked;
  - canonicalized targets must remain under the canonical grant root;
  - write target symlinks/dirs rejected;
  - Unix writes add `O_NOFOLLOW`.
- Bounds are explicit:
  - request descriptor, path, read bytes, write bytes, list entry count, and diagnostics are bounded/redacted;
  - read/list truncation is explicit via `truncated`;
  - oversize write fails closed.
- Writes serialize by normalized/canonical target with a shared lock map and use truncate/create with `sync_all`.
- Diagnostics use `PluginFsError::new` with `bounded_message(redact_secret_like(...))`; tests verify secret-like path redaction and no file-content leakage on denial.
- FS responses flow through WASM host response buffers and then ordinary plugin Tool output/error handling; no hidden context/history injection path found.
- CLI inspection reports `configured_fs_grants` alongside existing permission/HTTPS grant inspection without changing HTTPS semantics.

Reviewer validation:
- Static review of changed files and relevant tests.
- Confirmed worktree branch/head.
- `git diff --check 717c0999^..717c0999` — passed.
- Orchestrator-reported validation was reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - `cargo test -p pod feature::plugin`
  - `cargo test -p manifest plugin`
  - `cargo test -p yoi plugin_cli`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Non-blocking risk noted:
- The implementation is still fundamentally path/canonicalization based. It rejects static symlink/root escapes and uses Unix `O_NOFOLLOW` for final write target, but it does not fully eliminate adversarial external TOCTOU races where another local process mutates granted-root directories between checks and open/read/list/write. This is judged within the feasible scope of the initial subset and not a blocker for this Ticket, but should be preserved as future hardening context if the threat model expands.

Conclusion:
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---
