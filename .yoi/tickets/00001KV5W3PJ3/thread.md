<!-- event: create author: "yoi ticket" at: 2026-06-15T14:48:59Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-18T13:11:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-18T13:11:45Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。
- `depends_on 00001KV5W3PHW` は done/closed 済みで、Plugin Tool execution path が存在するため typed relation blocker は解消済み。
- 本 Ticket は Plugin manifest の requested permissions と Profile/config grants を照合し、enablement / Tool registration / Tool execution / future host API dispatch の enforcement points を明確にする実装であり、`https` / `fs` host API 実装や broad policy UI は non-goal として明確。
- permission / grant-enforcement / capability-boundary / tool-execution risk は高いが、fail-closed conditions、diagnostics、PreToolCall alignment、external_write handling が Ticket に具体化されているため implementation-ready と判断する。

Evidence checked:
- Ticket body/thread: requirements、initial grant model、acceptance criteria、non-goals、related work を確認。
- Ticket relations: outgoing `depends_on 00001KV5W3PHW` は done/closed。related design `00001KSXRQ4G8` は blocker ではない。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`b6685af3` 上。
- Visible Pods/worktrees: active implementation child なし。

IntentPacket:

Intent:
- Plugin requested permissions と explicit grants を typed model で照合し、Plugin Tool registration/execution と future host API dispatch が grant なしでは fail closed になる boundary を実装する。

Binding decisions / invariants:
- Package presence / discovery / Tool registration だけで execution authority を得ない。
- Requested but not granted は fail closed。
- Unknown permission kind / unsupported grant / overly broad ambiguous grant は fail closed または explicit diagnostic。
- Grant は package ref / source-qualified identity / digest / version と結びつけ、mismatch grant は使わない。
- Permission declarations/grants を ambient workspace FS/network authority として扱わない。
- `https` / `fs` host API の実行実装は non-goal。ただし requested/granted 型と denial diagnostics は扱う。
- Tool effect / external_write metadata は existing permission / PreToolCall path と矛盾させない。
- Denial diagnostics は bounded/safe で、hidden model context injection しない。

Requirements / acceptance criteria:
- Grant なしの Plugin Tool は登録または実行されず safe diagnostic になる。
- Granted Tool だけが登録または実行可能になる。
- Requested surface missing、requested tool missing、external_write missing、host API missing、digest/version/source mismatch、unknown permission kind は fail closed。
- Denied reason が diagnostic / trace で確認できる。
- Existing PreToolCall / Tool permission path と矛盾しない。
- Tests cover no grant denies Plugin Tool execution, grant allows specific Plugin Tool, unrelated package grant does not apply, digest mismatch denies, requested surface missing denies, external_write missing denies, unknown permission kind fails closed, bounded safe denial reason。

Implementation latitude:
- Initial grant model は `surfaces.tool`, tool names/namespaces, external_write flag, `host_api.https`, `host_api.fs` を狭く始める。
- Enforcement point のうち Tool registration と Tool execution のどちらで落とすかは実装判断。ただし model-visible schema gating と runtime execution gating の境界を実装報告に明記する。
- Existing Plugin resolver / Tool surface / WASM runtime code に合わせて typed structs / diagnostics を配置してよい。

Escalate if:
- PreToolCall / Tool metadata public API の broad redesign が必要。
- Grants を安全に package digest/version/source と結合できない。
- Future host API dispatch boundary を追加するには runtime executor redesign が必要。
- Permission UI/policy semantics を product decision として固定する必要が出る。

Validation:
- focused plugin permission/grant tests。
- relevant `cargo test` / `cargo check`。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi` if dependencies/Cargo.lock/package changes occur。

Critical risks / reviewer focus:
- fail-closed grant enforcement at registration and execution。
- source/digest/version binding。
- no ambient authority accidentally granted。
- external_write/effect metadata and PreToolCall consistency。
- bounded safe diagnostics。
- Plugin Tool execution path from `00001KV5W3PHW` is gated by grants。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-18T13:11:51Z from: queued to: inprogress reason: orchestrator_acceptance_dependency_done field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。dependency `00001KV5W3PHW` は done/closed で relation blocker は解消済み。Orchestrator workspace は clean。implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T13:12:47Z -->

## Implementation report

Implementation start failure / recovery note:

`queued -> inprogress` acceptance と worktree 作成は完了したが、Coder Pod spawn に失敗した。

Created worktree:
- `/home/hare/Projects/yoi/.worktree/00001KV5W3PJ3-plugin-permission-grants`
- branch: `impl/00001KV5W3PJ3-plugin-permission-grants`

Failure:
- `SpawnPod` failed before child Pod creation.
- Error: `failed to spawn /home/hare/Projects/yoi/target/debug/yoi (deleted) pod: No such file or directory (os error 2)`

Interpretation:
- Runtime binary path for this long-lived Orchestrator process again points at a deleted executable. This is an operational/runtime issue, not a Ticket requirement issue.
- No Coder Pod was started and no implementation commit exists in the child worktree yet.

Recovery needed:
- Refresh/restart the Orchestrator runtime or provide a valid Pod runtime command so `SpawnPod` can launch children again.
- After recovery, reuse the existing clean child worktree/branch and resume Coder spawn for this in-progress Ticket.

State handling:
- Ticket remains `inprogress` because acceptance and worktree side effects already happened; it is not silently returned to `queued`.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T13:56:44Z -->

## Implementation report

Implementation resume note:

Coder Pod の再開を試み、最初の `SpawnPod` は child scope に runtime workspace root `/home/hare/Projects/yoi` の read scope が含まれていなかったため失敗した。

Recovery:
- 既存 implementation worktree `/home/hare/Projects/yoi/.worktree/00001KV5W3PJ3-plugin-permission-grants` を再利用。
- child scope を workspace root read + implementation worktree write に狭く設定して再試行。
- Coder Pod `yoi-coder-00001KV5W3PJ3` の起動に成功。

Current status:
- Ticket は `inprogress` のまま。
- Coder は Plugin permission grants 実装、focused validation、commit、実装報告を行う予定。
- root/original workspace は触らず、implementation worktree 上で作業するよう指示済み。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T14:15:12Z -->

## Implementation report

Coder implementation report received:

Commit:
- `b1ba1599` — `plugin: enforce permission grants`

Summary:
- typed plugin permission declarations/grants を追加。
  - `surfaces.tool`
  - specific tool names
  - tool namespaces
  - `external_write`
  - future `host_api.https` / `host_api.fs`
- grant binding を source-qualified package id / deterministic digest / exact package version に結合。
- registration gating を `PluginToolFeature::install` に実装。
  - ungranted / unrequested plugin tools は登録されない。
  - partial grant では authorized tools のみ登録し、skipped tools は bounded diagnostics。
- runtime execution gating を `run_plugin_wasm_tool` に実装。
  - WASM 読み込み/実行前に binding と requested/granted tool authority を再確認。
- future host API boundary として `validate_wasm_imports` が `yoi:https` / `yoi:fs` imports を認識し、non-implemented boundary error の前に permission check。
- actual `https` / `fs` host API は実装していない。
- `external_write` を plugin tool manifest metadata に追加し、requested+granted の場合のみ許可。
- Existing PreToolCall/tool permission path は変更せず、plugin grant checks を追加の fail-closed gate とした。

Files changed:
- `crates/manifest/src/plugin.rs`
- `crates/pod/src/feature/plugin.rs`
- `crates/pod/src/pod.rs`

Validation reported by Coder:
- `cargo fmt --check`
- `cargo check -p manifest -p pod`
- `cargo test -p pod plugin -- --nocapture` — 27 passed
- `cargo test -p manifest plugin -- --nocapture` — 17 passed
- `git diff --check`

Orchestrator evidence checked:
- Pod output を確認。
- implementation worktree は clean。
- commit `b1ba1599` が branch `impl/00001KV5W3PJ3-plugin-permission-grants` の HEAD。
- diff stat: `crates/manifest/src/plugin.rs`, `crates/pod/src/feature/plugin.rs`, `crates/pod/src/pod.rs` に 575 insertions / 19 deletions。

Routing classification: review_needed

Next action:
- recorded intent / invariants / acceptance criteria に照らして、read-only Reviewer Pod で外部レビューする。
- 特に fail-closed grant enforcement、source/digest/version binding、no ambient authority、external_write/effect metadata、bounded diagnostics、existing PreToolCall path との整合性を確認する。

---
