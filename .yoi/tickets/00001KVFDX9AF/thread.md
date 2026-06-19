<!-- event: create author: "yoi ticket" at: 2026-06-19T07:53:13Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-19T10:19:53Z from: ready to: queued reason: queued field: state -->

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
- `00001KVFDX9AY` fs host API とは WASM Plugin Tool runtime host import boundary、Plugin grant model、diagnostics/tests/package behavior の変更面が重なるため `do_not_parallelize` plan record を残した。

Bounded reason for idle queued:
- conflict / reviewer-coder bottleneck。

Next action:
- `00001KVFD3YSV` の implementation/review/merge outcome を確認後、queued のまま再 routing する。
- その時点で `fs` host API Ticket との ordering / conflict も再確認する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-19T14:25:13Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body には、HTTPS host API intent、binding invariants、acceptance criteria、non-goals、validation、escalation-worthy risk domain が実装可能な粒度で揃っている。
- 依存 relation の `00001KV5W3PHW` minimal WASM runtime、`00001KV5W3PJ3` permission grants、関連 `00001KVFD3YSV` CLI inspection は closed で blocker ではない。
- Risk domain は network / secrets / host API / permission grants だが、Ticket は HTTPS-only、private/local target rejection、grant allowlist、bounded request/response/timeout/diagnostics、no ambient env/network、ordinary Tool result path を binding invariants として明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。
- 同時 queued の `00001KVFDX9AY` fs host API と `00001KVG0HR96` Component Model migration は Plugin runtime/grant/diagnostic/packaging surface が重なるため、waiting/conflict notes を更新し queued のまま待機する。

Evidence checked:
- Ticket `00001KVFDX9AF` body / thread / artifacts。
- `TicketRelationQuery(00001KVFDX9AF)`: depends_on は closed。related Ticket は context であり acceptance blocker ではない。
- `TicketOrchestrationPlanQuery(00001KVFDX9AF)`: 既存 waiting/do_not_parallelize records を確認。今回 `accepted_plan` を記録済み。
- Related completed Tickets:
  - `00001KV5W3PHW` — minimal WASM Tool runtime closed。
  - `00001KV5W3PJ3` — Plugin permission grants closed。
  - `00001KVFD3YSV` — Plugin read-only CLI inspection closed。
- Current queued Tickets:
  - `00001KVFDX9AY` fs host API: do_not_parallelize / waiting reason を維持。
  - `00001KVG0HR96` Component Model migration: migration boundary / conflict waiting note を更新。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`: clean。
- Existing branch/worktree: matching `00001KVFDX9AF` branch/worktree はなし。
- Visible Pods: self / peer / intake only; spawned child capacity is free。
- Current code map:
  - `crates/pod/src/feature/plugin.rs`: Plugin resolver, permission grants, static inspection, WASM tool feature。
  - `crates/pod/src/pod.rs`: WASM Tool runtime / `run_plugin_wasm_tool` / host import validation。
  - `crates/manifest/src/plugin.rs`: Plugin manifest and permission model。
  - `crates/yoi/src/plugin_cli.rs`: read-only inspection output should remain compatible with host API diagnostics。

IntentPacket:

Intent:
- WASM Plugin Tool runtime に、明示 grant された outbound HTTPS request だけを実行できる `https` host API を追加する。
- Plugin は ambient network access を持たず、host API import + requested permission + config grant + allowlist を満たす場合だけ bounded HTTPS request を実行できる。

Binding decisions / invariants:
- Host API name/domain は `https`。`web` ではない。
- HTTPS-only。`http://`、localhost、private IP、link-local、unix socket、file URL、local/private host targets は reject。
- Grant がない場合、network access 前に fail closed。
- host / method / optional path prefix などの allowlist を表現し、grant と request を照合する。
- Request/response は bounded。
  - method allowlist
  - request body size bound
  - header count/size bound
  - response body size bound
  - timeout
  - redirect policy
- Credentials は ambient env から読まない。header/auth は explicit config / secret ref 経由だけ。
- Diagnostics に secret-like header/token/body content を漏らさない。
- HTTPS response は hidden context injection ではなく ordinary Tool result/history path に残す。
- `fs` host API、WebSocket/SSE/timers、Service/Ingress lifecycle、Plugin package manager は non-goals。

Requirements / acceptance criteria:
- Granted Plugin Tool can perform an allowed HTTPS request through host API。
- Missing `host_api.https` grant denies before network access。
- Disallowed host / method / URL scheme denies。
- `http://`, localhost, private IP, link-local, local/private host targets reject。
- Timeout and response-size bounds are enforced。
- Request/response diagnostics are bounded and redact secret-like values。
- No ambient env credentials or ambient network APIs are exposed to WASM。
- Tool result path remains ordinary Tool result/history path。
- Tests cover allowed HTTPS, missing grant, disallowed host/method/scheme/private target, timeout, response truncation, secret redaction, no network without host API import/grant。

Implementation latitude:
- Choose exact ABI/import shape consistent with existing `yoi-plugin-wasm-1` host import design。
- Choose narrow grant config representation for host/method/path allowlist consistent with current Plugin permission grant model。
- Use local deterministic test server/mock if needed for allowed HTTPS/timeout/response bound tests, but keep network-safety tests deterministic。
- Choose bounded response header/body representation that fits existing Tool result error/result types。

Escalate if:
- Implementing HTTPS requires broad runtime executor redesign or Component Model migration first。
- Secure host/method/path grant model cannot be represented without breaking existing permission grant schema。
- SecretRef handling requires new secret-store public API beyond explicit config references。
- Safe private/local-host rejection cannot be implemented deterministically enough for tests。
- Host API result/error shape requires product-visible Tool result schema decision beyond this Ticket。

Validation:
- Focused plugin HTTPS host API tests。
- Relevant `cargo test` / `cargo check` for `pod`, `manifest`, `yoi` as changed。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi` because dependency/package/network code may change。

Critical risks / reviewer focus:
- Network access before grant/allowlist checks。
- Private/local/localhost bypass through DNS, redirects, IPv6, encoded URLs, or scheme confusion。
- Secret leakage in diagnostics or Tool result/history。
- Ambient env credential usage。
- Unbounded request/response bodies or headers。
- Hidden context injection by bypassing normal Tool result path。
- Breaking existing Plugin permission grant enforcement or CLI inspection semantics。

Next action:
- `queued -> inprogress` を記録し、Ticket records を Orchestrator worktree に commit してから、専用 implementation worktree を作成し Coder Pod を narrow write scope で起動する。root/original workspace は操作しない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-19T14:25:28Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_https_host_api field: state -->

## State changed

Ticket body/thread, relation metadata, orchestration plan records, related completed Tickets, Orchestrator worktree, visible Pods, existing branch/worktree, and bounded Plugin host API code context were checked. Depends-on blockers are closed, prior CLI inspection related work is closed, and no dirty-state blocker or missing planning decision was found. `fs` host API and Component Model migration remain queued with conflict/migration waiting records. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---
