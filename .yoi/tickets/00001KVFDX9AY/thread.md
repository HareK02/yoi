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
