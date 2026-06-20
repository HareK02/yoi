<!-- event: create author: "yoi ticket" at: 2026-06-20T13:29:16Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-20T13:30:16Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-20T13:30:16Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T13:31:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T13:31:52Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- Workspace Dashboard Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- User standing directive: blocker が無いものは並列実行する。現在 `00001KVJHYP4Q` Plugin instance lifecycle Coder が inprogress だが、この Ticket は MCP CLI inspection namespaceであり直接 dependency/conflict はない。Potential shared CLI parser edits are manageable in separate worktree and will be resolved at merge/review boundary。
- Ticket body は `yoi mcp` namespace、list/show/tools/resources/prompts commands、human/JSON output、workspace/profile resolution、read-only inspection boundary、static-vs-live availability、secret/content redaction、acceptance testsを実装可能な粒度で定義している。
- 未解決 blocker relation はない。Relations are `related` context to completed MCP foundation Tickets。
- Orchestrator worktree は clean、matching branch/worktree はなし。
- Risk domain は CLI / MCP / inspection / secrets / read-only boundary だが、Ticket は no server process start、no tool execution、no resources/prompts content fetch、secrets/content non-printing、bounded outputを明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVJKHAFE` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVJKHAFE)`: only non-blocking `related` records to completed MCP config/lifecycle/tools/resources/list_changed Tickets。
- `TicketOrchestrationPlanQuery(00001KVJKHAFE)`: no previous plan records; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `142fdffb`。
  - queued: this Ticket only。
  - inprogress: `00001KVJHYP4Q` Plugin instance lifecycle。
  - visible spawned child: `yoi-coder-00001KVJHYP4Q` running。
  - no matching MCP CLI branch/worktree。

IntentPacket:

Intent:
- Add read-only `yoi mcp` CLI inspection namespace for configured/resolved MCP servers and provider-discovered tools/resources/prompts eligibility。
- Provide bounded human-readable and JSON reports without bypassing runtime Tool/resource/prompt paths。

Binding decisions / invariants:
- CLI inspection must not start MCP server processes。
- CLI inspection must not call MCP tools or fetch resource/prompt content。
- Static config/resolution and live Pod state must be distinguished explicitly。
- If live state is unavailable/unimplemented, output must say `not live` / `unavailable`, not silently stale。
- Secrets, resolved secret/env values, resource contents, and prompt full text must not be printed in human or JSON output。
- External server descriptions/schemas/annotations are untrusted and bounded/truncated。
- Keep MCP separate from Plugin CLI namespace。
- No Streamable HTTP/OAuth/sampling/elicitation/install/update/distribution implementation。

Requirements / acceptance criteria:
- `yoi --help` shows MCP CLI namespace。
- `yoi mcp list --json` returns resolved MCP server bounded structured report。
- `yoi mcp show <server> --json` returns server identity, transport kind, trust policy summary, capabilities summary, diagnostics。
- `yoi mcp tools [<server>] --json` returns Yoi stable tool name, MCP server/tool identity, schema availability, registration status/diagnostics。
- `yoi mcp resources [<server>] --json` and `yoi mcp prompts [<server>] --json` return summaries/eligibility without content fetch。
- Human-readable output distinguishes empty/missing/invalid/unavailable。
- Focused CLI tests cover list/show/tools/resources/prompts, missing server, invalid config, JSON output, and secret/content non-leakage。
- Validation includes fmt, focused tests, check, diff, TicketDoctor, Nix build。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T13:32:15Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_mcp_cli_inspection field: state -->

## State changed

Ticket body/thread, relation metadata, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded MCP CLI inspection context were checked. There is no unresolved blocking dependency, no direct conflict with current Plugin instance lifecycle work, no matching worktree/branch, and no missing planning decision. Accepting this queued Ticket for parallel implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T13:33:06Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `12d7e69f ticket: accept mcp cli inspection`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVJKHAFE-mcp-cli-inspection` on branch `impl/00001KVJKHAFE-mcp-cli-inspection` at `12d7e69f`。
- Spawned Coder Pod `yoi-coder-00001KVJKHAFE` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, MCP server process startup, tools/call/resource/prompt content fetch, Plugin CLI namespace mixing, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T13:48:38Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVJKHAFE`.

Implementation commit:
- `c91f5fc9 mcp: add cli inspection`

Changed areas reported:
- Added `crates/yoi/src/mcp_cli.rs`:
  - Read-only `yoi mcp` inspection reports。
  - Commands: `list`, `show`, `tools`, `resources`, `prompts`。
  - Human and `--json` output。
  - Static resolved config inspection only; no MCP server processes are started。
  - Live/provider-discovered state is explicitly reported as `not_live` / `unavailable`。
  - Env values, secret refs, env refs, args, resource content, and prompt content are redacted/omitted。
  - Resource/prompt operation eligibility is reported without content fetch。
- Updated `crates/yoi/src/main.rs`:
  - Added `mcp` namespace to parser, dispatcher, help output, and parser tests。

Coder validation reported:
- `cargo test -p yoi mcp`: passed。
- `cargo check -p yoi`: passed。
- `cargo fmt --check`: passed。
- `git diff --check`: passed。
- `cargo run -p yoi -- --help | grep -E 'yoi mcp (list|show|tools)'`: passed; help shows MCP namespace。
- `yoi ticket doctor`: passed。

Package impact:
- No dependency, lockfile, source-filter, or `package.nix` changes。
- Nix not run because packaging inputs were not changed。

Known risks / deferrals:
- Live Pod MCP state is not integrated; provider-discovered tools/resources/prompts are therefore reported as `not_live` / `unavailable` rather than stale or fabricated。
- `tools` cannot list Yoi stable provider tool names without live discovery; JSON schema includes future item shape, but item list is empty in static mode。
- Touched `crates/yoi/src/main.rs` CLI parser/help, so there is possible narrow conflict risk with parallel Plugin Service/Ingress work. No Plugin Service/Ingress files were touched。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `c91f5fc9`。
- Diff from acceptance `12d7e69f..HEAD` is one implementation commit touching 2 files, about 1438 insertions / 1 deletion。
- `git diff --check 12d7e69f..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on read-only inspection boundary, no process start/no tools-call/no resource-prompt content fetch, static vs live unavailable honesty, redaction, JSON/human shape, help/tests, and conflict risk。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T13:49:28Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVJKHAFE-r1` against implementation branch `impl/00001KVJKHAFE-mcp-cli-inspection`。
- Review target commit: `c91f5fc9 mcp: add cli inspection`。
- Review baseline: `12d7e69f`。
- Reviewer task focuses on read-only inspection boundary, no process start/no tools-call/no resource/prompt content fetch, static vs live unavailable honesty, secret/content redaction, JSON/human report shape, namespace separation from Plugin CLI, help/tests, scope creep absence, and parallel CLI merge conflict risk。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---
