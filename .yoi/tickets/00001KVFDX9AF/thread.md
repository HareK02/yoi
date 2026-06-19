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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T14:26:32Z -->

## Implementation report

Implementation start note:

`queued -> inprogress` acceptance、accepted plan、routing decision / IntentPacket、fs host API / Component Model migration の waiting records を記録し、Orchestrator worktree で commit した後に、専用 implementation worktree と Coder Pod を起動した。

Worktree:
- `/home/hare/Projects/yoi/.worktree/00001KVFDX9AF-plugin-https-host-api`
- branch: `impl/00001KVFDX9AF-plugin-https-host-api`

Coder Pod:
- `yoi-coder-00001KVFDX9AF`

Scope / boundaries:
- child runtime workspace root は read scope。
- implementation worktree は write scope。
- root/original workspace と Orchestrator worktree へは書き込まないよう指示済み。
- `.yoi/memory`、runtime/log/lock/local/secret-like `.yoi` paths は編集しないよう指示済み。

Related queued Tickets:
- `00001KVFDX9AY` fs host API remains queued due existing do_not_parallelize / conflict with HTTPS host API.
- `00001KVG0HR96` Component Model migration remains queued due migration boundary / conflict with active host API work.

Next:
- Coder は granted HTTPS host API implementation、focused tests、validation、implementation commit を行い、実装報告を返す予定。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T15:04:07Z -->

## Implementation report

Coder implementation report received / Orchestrator validation:

Implementation commit:
- `73775275` — `plugin: implement https host api`

Summary:
- Added typed HTTPS host API grant scope to Plugin permission model:
  - hosts
  - methods
  - optional path prefixes
  - bounded request/response options
- Implemented `yoi:https` WASM host import handling in the Plugin Tool runtime。
- Added deterministic local TLS / HTTPS test harness for runtime tests。
- Added URL validation / network safety checks:
  - HTTPS-only;
  - reject `http://`;
  - reject localhost/private/link-local/local targets;
  - reject disallowed hosts/methods/schemes before network access。
- Added request/response bounds and diagnostics:
  - body/header limits;
  - response truncation;
  - timeout;
  - bounded/redacted error messages。
- Preserved no ambient env credentials and ordinary Tool result/history path semantics。
- Updated read-only Plugin CLI inspection so HTTPS host API grant details / diagnostics remain inspectable。
- Updated `package.nix` cargo hash for new dependency/build closure.

Files changed:
- `Cargo.lock`
- `crates/manifest/src/plugin.rs`
- `crates/pod/Cargo.toml`
- `crates/pod/src/feature/plugin.rs`
- `crates/pod/src/pod.rs`
- `crates/yoi/src/plugin_cli.rs`
- `package.nix`

Coder reported validation:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin::tests -- --nocapture` — passed; 36 passed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Orchestrator evidence checked:
- Pod output を確認。
- implementation worktree は clean。
- branch `impl/00001KVFDX9AF-plugin-https-host-api` HEAD は `73775275`。
- diff stat from accepted base `1fdef32a..73775275`: 7 files changed, 1799 insertions, 97 deletions。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin::tests -- --nocapture` — passed; 36 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- recorded intent / invariants / acceptance criteria に照らして、read-only Reviewer Pod で外部レビューする。
- 特に grant-before-network、private/local rejection、redirect/DNS bypass、bounds/timeouts、secret redaction、ordinary Tool result path、no ambient env credentials、CLI inspection compatibility、dependency/package.nix correctness を確認する。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T15:04:59Z -->

## Plan

Review dispatch:

Reviewer Pod `yoi-reviewer-00001KVFDX9AF` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVFDX9AF-plugin-https-host-api`
- branch: `impl/00001KVFDX9AF-plugin-https-host-api`
- commit: `73775275`

Reviewer focus:
- recorded intent / invariants / acceptance criteria に対する適合性。
- network access が grant/allowlist checks の前に発生しないこと。
- private/local/localhost/link-local target rejection が DNS / IPv6 / redirect / scheme edge cases を含め妥当であること。
- timeout / request-response size / header bounds が enforce されること。
- secret-like values が errors / diagnostics / Tool results に漏れないこと。
- ambient env credential / ambient network access path が WASM に exposed されていないこと。
- HTTPS response が ordinary Tool result/history path だけに戻ること。
- allowed HTTPS / missing grant / disallowed host-method-scheme-private target / timeout / truncation / redaction / no network without grant/import tests が意味を持つこと。
- manifest/permission grant schema changes が existing grants / CLI inspection と整合していること。
- `Cargo.lock` / `package.nix` update が妥当で、`nix build .#yoi --no-link` validation と整合していること。
- `fs` host API と Component Model migration が non-goals として保たれていること。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree への merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AF at: 2026-06-19T15:10:37Z status: request_changes -->

## Review: request changes

Review result: request_changes

対象:
- Ticket: `00001KVFDX9AF`
- branch: `impl/00001KVFDX9AF-plugin-https-host-api`
- commit: `7377527f` (`plugin: implement https host api`)

Blockers:

1. Private/local target rejection is bypassable with IPv4-mapped IPv6 literals.
   - `validate_static_https_target` parses the URL host as `IpAddr` and delegates IPv6 literals to `is_forbidden_ipv6` (`crates/pod/src/feature/plugin.rs:622-631`, `665-669`).
   - `is_forbidden_ipv6` rejects loopback/unspecified/ULA/link-local/multicast (`699-705`) but does not check `Ipv6Addr::to_ipv4_mapped()` / IPv4-compatible forms.
   - Rust confirms `::ffff:127.0.0.1` is not `is_loopback()` as IPv6, while `to_ipv4_mapped()` is `Some(127.0.0.1)`.
   - A granted request to `https://[::ffff:127.0.0.1]/...` would therefore pass the current private/local filter if the grant host matches, violating localhost/private IP rejection and the IPv6 edge-case reviewer focus.

2. DNS private-address validation is TOCTOU and does not bind the checked address set to the actual connection.
   - `ReqwestPluginHttpsClient::execute` calls `validate_dns_target(url)` before constructing/sending the request (`crates/pod/src/feature/plugin.rs:365-405`).
   - `validate_dns_target` resolves with `ToSocketAddrs` and rejects private/local addresses in that returned set (`641-655`), but `reqwest` performs its own DNS resolution later during `builder.send()` (`405`).
   - DNS rebinding / split answers between the preflight lookup and reqwest’s lookup can pass validation on a public address then connect to a private/local address.
   - Ticket critical risks explicitly include DNS/private/local bypasses before network access.

Requested changes:
- Reject IPv4-mapped IPv6 addresses by applying the existing IPv4 forbidden checks to mapped IPv4 addresses; add tests for mapped loopback/private/link-local examples.
- Bind DNS validation to the actual connection path. Either pin validated public `SocketAddr`s into the reqwest client/request resolver path, or otherwise ensure the actual connected remote address is validated / not private.
- Add deterministic test coverage around the real validation edge cases where feasible.

Supporting positive findings:
- Grant checks happen before request parsing/DNS/network dispatch in `execute_plugin_https_request`, and WASM import validation denies `yoi:https` imports without requested/configured `host_api.https` grant before instantiation.
- Scheme, embedded credentials, method allowlist, request descriptor/body/header count/size, response body/header bounds, timeout, no redirects, and `no_proxy()` are present.
- WASM imports are narrow (`yoi:tool`, `yoi:https` only), WASI/ambient imports are rejected, and `yoi:fs` remains explicitly unimplemented.
- Response path remains ordinary plugin Tool output/history path.
- Manifest/CLI schema additions are mostly compatible: new `PluginPermission::HostApi`, `PluginHttpsGrant`, defaulted `grants.https`, and CLI display of configured HTTPS grants.

Test gap:
- Current HTTPS-focused tests use `MockHttpsClient` for allowed/denied/timeout/truncation behavior. That is useful for grant ordering, but it does not exercise the real `ReqwestPluginHttpsClient` DNS filtering, redirect policy, TLS/HTTPS path, or actual timeout/read-limit behavior. Given the security-sensitive DNS/IP requirements, add coverage for the real validation edge cases above.

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T15:12:19Z -->

## Plan

Review follow-up dispatch:

Reviewer `yoi-reviewer-00001KVFDX9AF` は `request_changes` を返したため、Ticket review として記録し、既存 Coder Pod `yoi-coder-00001KVFDX9AF` に修正依頼を送った。

Blockers:
1. IPv4-mapped IPv6 literals が private/local target rejection を bypass し得る。
   - `::ffff:127.0.0.1` などを IPv6 として見るだけでは loopback/private 判定に漏れる。
   - `Ipv6Addr::to_ipv4_mapped()` 等で mapped IPv4 を取り出し、既存 IPv4 forbidden checks を適用する必要がある。

2. DNS private-address validation が TOCTOU で actual connection path に bind されていない。
   - preflight `ToSocketAddrs` validation 後に `reqwest` が別途 DNS resolution するため、DNS rebinding / split answers で public answer validation 後に private/local address へ接続し得る。
   - validated public `SocketAddr`s を reqwest client/request resolver path に pin する、または actual connected remote address を検証する必要がある。

Required fixes sent to Coder:
- IPv4-mapped IPv6 addresses を mapped IPv4 として private/local/link-local/loopback checks に通す。
- mapped loopback/private/link-local の tests を追加する。
- DNS validation を actual connection path に bind する設計へ修正する。
- DNS / actual connection edge の deterministic test coverage を追加する。
- Prior positives を維持する: grant-before-network、WASM import grant validation、HTTPS-only、embedded credentials rejection、method allowlist、bounds、timeout、no redirects、`no_proxy()`、ordinary Tool result path、manifest/CLI schema compatibility。

Next:
- Coder の修正 commit / validation report を待つ。
- 修正後、network-safety edge cases と grant-before-network invariants を中心に再レビューする。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T15:22:56Z -->

## Implementation report

Coder review-fix implementation report received / Orchestrator validation:

Incremental fix commit:
- `85683f17` — `plugin: harden https target validation`

Summary:
- Hardened IPv6 target validation:
  - IPv4-mapped IPv6 addresses now apply existing IPv4 private/local/link-local checks.
  - IPv4-compatible embedded forms are also checked through IPv4 rules.
  - Host canonicalization strips IPv6 brackets before IP parsing.
- Removed DNS validation TOCTOU gap:
  - DNS resolution now produces validated public `SocketAddr`s.
  - Those exact validated addresses are pinned into the `reqwest` client via `resolve_to_addrs`, so `reqwest` does not perform a separate unvalidated DNS lookup for hostname requests.
  - Private DNS answers are rejected before client build / network dispatch.
- Added deterministic tests for:
  - IPv4-mapped loopback/private/link-local rejection;
  - IPv4-compatible private rejection;
  - DNS resolution pinning to validated public socket addresses;
  - private DNS answer rejection before client build.

Files changed in incremental fix:
- `crates/pod/src/feature/plugin.rs`

Coder reported validation:
- `cargo test -p pod feature::plugin::tests` — passed; 39 tests
- `cargo test -p manifest plugin` — passed; 17 tests
- `cargo test -p yoi plugin_cli` — passed; 10 tests
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo fmt --check` — passed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Orchestrator evidence checked:
- Pod output を確認。
- implementation worktree は clean。
- branch `impl/00001KVFDX9AF-plugin-https-host-api` HEAD は `85683f17`。
- incremental diff stat from previous review target `7377527f..85683f17`: 1 file changed, 170 insertions, 21 deletions。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin::tests -- --nocapture` — passed; 39 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- previous `request_changes` blockers が解消されたか、read-only Reviewer Pod で再レビューする。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T15:23:58Z -->

## Plan

Review re-dispatch:

Reviewer Pod `yoi-reviewer-00001KVFDX9AF-r2` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVFDX9AF-plugin-https-host-api`
- branch: `impl/00001KVFDX9AF-plugin-https-host-api`
- commits:
  - `7377527f` — initial HTTPS host API implementation
  - `85683f17` — target validation hardening fix

Reviewer focus:
- previous IPv4-mapped/compatible IPv6 private/local bypass blocker が解消されているか。
- DNS validation が validated address pinning により actual reqwest connection path に bind され、unvalidated resolution path が残っていないか。
- mapped IPv6 / DNS pinning / private DNS answer rejection tests が deterministic で意味を持つか。
- prior positives: grant-before-network、WASM import grant validation、HTTPS-only、embedded credentials rejection、method allowlist、bounds、timeout、no redirects、`no_proxy()`、ordinary Tool result path、manifest/CLI schema compatibility が維持されているか。
- `resolve_to_addrs` usage や host canonicalization に新しい security regression がないか。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree への merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AF-r2 at: 2026-06-19T15:29:22Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AF`
- branch: `impl/00001KVFDX9AF-plugin-https-host-api`
- commits:
  - `7377527f` — `plugin: implement https host api`
  - `85683f17` — `plugin: harden https target validation`

No remaining blockers found.

Evidence reviewed:
- IPv4-mapped / compatible IPv6 bypass is closed.
  - Host canonicalization strips IPv6 brackets before IP parsing via `normalize_host_literal` / `canonical_host`.
  - `validate_static_https_target` parses canonical hosts as `IpAddr` and applies `validate_public_ip`.
  - `is_forbidden_ipv6` delegates mapped/compatible embedded IPv4 addresses through `ipv6_embedded_ipv4`, then applies existing IPv4 private/loopback/link-local/broadcast/special checks.
  - `ipv4_mapped_ipv6_targets_deny_before_network` covers `::ffff:127.0.0.1`, `::ffff:10.0.0.1`, `::ffff:169.254.169.254`, and compatible `::10.0.0.1`, with mock network call count asserted as zero.
- DNS validation is bound to the reqwest connection path.
  - `ReqwestPluginHttpsClient::execute` calls `resolve_https_target_for_client` before building the client.
  - Hostname URLs resolve to `SocketAddr`s, reject empty/private/local answers, and return those exact validated addresses.
  - reqwest builder pins each validated domain using `resolve_to_addrs(domain, &pinned_resolution.addrs)`, with no remaining separate unvalidated DNS lookup path visible in this implementation.
  - IP-literal URLs skip DNS pinning only after static IP validation.
- DNS tests are deterministic and meaningful at the unit boundary.
  - `FakeHttpsResolver` avoids live DNS/network.
  - `dns_resolution_is_pinned_to_validated_public_socket_addresses` verifies resolver input, returned pinned domains/addrs, and reqwest builder acceptance of `resolve_to_addrs`.
  - `dns_resolution_rejects_private_addresses_before_client_build` verifies private DNS answers fail before client build/network dispatch.
- Prior positives remain preserved.
  - Grant checks happen before request validation/client dispatch in `execute_plugin_https_request`.
  - WASM import validation checks `yoi:https` authorization before allowing the import.
  - HTTPS-only, embedded-credential rejection, host/method/path allowlist, request/header/body bounds, response truncation, timeout, no redirects, `no_proxy()`, sensitive-header rejection/redaction, and ordinary Tool result flow remain present.
  - Existing tests cover missing grant before network, disallowed host/method/scheme/private targets before network, timeout/redaction, response truncation, no network without HTTPS import, and ordinary Tool result path.

Reviewer validation:
- Static diff/code review and worktree status inspection.
- Orchestrator-reported successful runs were reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - focused pod/manifest/yoi plugin tests
  - `git diff --check`
  - `nix build .#yoi --no-link`

Conclusion:
- Previous request_changes blockers are resolved.
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AF-r2 at: 2026-06-19T15:29:45Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AF`
- branch: `impl/00001KVFDX9AF-plugin-https-host-api`
- commits:
  - `7377527f` — `plugin: implement https host api`
  - `85683f17` — `plugin: harden https target validation`

No remaining blockers found.

Evidence reviewed:
- IPv4-mapped / compatible IPv6 bypass is closed.
  - Host canonicalization strips IPv6 brackets before IP parsing via `normalize_host_literal` / `canonical_host`.
  - `validate_static_https_target` parses canonical hosts as `IpAddr` and applies `validate_public_ip`.
  - `is_forbidden_ipv6` delegates mapped/compatible embedded IPv4 addresses through `ipv6_embedded_ipv4`, then applies existing IPv4 private/loopback/link-local/broadcast/special checks.
  - `ipv4_mapped_ipv6_targets_deny_before_network` covers `::ffff:127.0.0.1`, `::ffff:10.0.0.1`, `::ffff:169.254.169.254`, and compatible `::10.0.0.1`, with mock network call count asserted as zero.
- DNS validation is bound to the reqwest connection path.
  - `ReqwestPluginHttpsClient::execute` calls `resolve_https_target_for_client` before building the client.
  - Hostname URLs resolve to `SocketAddr`s, reject empty/private/local answers, and return those exact validated addresses.
  - reqwest builder pins each validated domain using `resolve_to_addrs(domain, &pinned_resolution.addrs)`, with no remaining separate unvalidated DNS lookup path visible in this implementation.
  - IP-literal URLs skip DNS pinning only after static IP validation.
- DNS tests are deterministic and meaningful at the unit boundary.
  - `FakeHttpsResolver` avoids live DNS/network.
  - `dns_resolution_is_pinned_to_validated_public_socket_addresses` verifies resolver input, returned pinned domains/addrs, and reqwest builder acceptance of `resolve_to_addrs`.
  - `dns_resolution_rejects_private_addresses_before_client_build` verifies private DNS answers fail before client build/network dispatch.
- Prior positives remain preserved.
  - Grant checks happen before request validation/client dispatch in `execute_plugin_https_request`.
  - WASM import validation checks `yoi:https` authorization before allowing the import.
  - HTTPS-only, embedded-credential rejection, host/method/path allowlist, request/header/body bounds, response truncation, timeout, no redirects, `no_proxy()`, sensitive-header rejection/redaction, and ordinary Tool result flow remain present.
  - Existing tests cover missing grant before network, disallowed host/method/scheme/private targets before network, timeout/redaction, response truncation, no network without HTTPS import, and ordinary Tool result path.

Reviewer validation:
- Static diff/code review and worktree status inspection.
- Orchestrator-reported successful runs were reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - focused pod/manifest/yoi plugin tests
  - `git diff --check`
  - `nix build .#yoi --no-link`

Conclusion:
- Previous request_changes blockers are resolved.
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---

<!-- event: review author: yoi-reviewer-00001KVFDX9AF-r2 at: 2026-06-19T15:32:15Z status: approve -->

## Review: approve

Review result: approve

対象:
- Ticket: `00001KVFDX9AF`
- branch: `impl/00001KVFDX9AF-plugin-https-host-api`
- commits:
  - `7377527f` — `plugin: implement https host api`
  - `85683f17` — `plugin: harden https target validation`

No remaining blockers found.

Evidence reviewed:
- IPv4-mapped / compatible IPv6 bypass is closed.
  - Host canonicalization strips IPv6 brackets before IP parsing via `normalize_host_literal` / `canonical_host`.
  - `validate_static_https_target` parses canonical hosts as `IpAddr` and applies `validate_public_ip`.
  - `is_forbidden_ipv6` delegates mapped/compatible embedded IPv4 addresses through `ipv6_embedded_ipv4`, then applies existing IPv4 private/loopback/link-local/broadcast/special checks.
  - `ipv4_mapped_ipv6_targets_deny_before_network` covers `::ffff:127.0.0.1`, `::ffff:10.0.0.1`, `::ffff:169.254.169.254`, and compatible `::10.0.0.1`, with mock network call count asserted as zero.
- DNS validation is bound to the reqwest connection path.
  - `ReqwestPluginHttpsClient::execute` calls `resolve_https_target_for_client` before building the client.
  - Hostname URLs resolve to `SocketAddr`s, reject empty/private/local answers, and return those exact validated addresses.
  - reqwest builder pins each validated domain using `resolve_to_addrs(domain, &pinned_resolution.addrs)`, with no remaining separate unvalidated DNS lookup path visible in this implementation.
  - IP-literal URLs skip DNS pinning only after static IP validation.
- DNS tests are deterministic and meaningful at the unit boundary.
  - `FakeHttpsResolver` avoids live DNS/network.
  - `dns_resolution_is_pinned_to_validated_public_socket_addresses` verifies resolver input, returned pinned domains/addrs, and reqwest builder acceptance of `resolve_to_addrs`.
  - `dns_resolution_rejects_private_addresses_before_client_build` verifies private DNS answers fail before client build/network dispatch.
- Prior positives remain preserved.
  - Grant checks happen before request validation/client dispatch in `execute_plugin_https_request`.
  - WASM import validation checks `yoi:https` authorization before allowing the import.
  - HTTPS-only, embedded-credential rejection, host/method/path allowlist, request/header/body bounds, response truncation, timeout, no redirects, `no_proxy()`, sensitive-header rejection/redaction, and ordinary Tool result flow remain present.
  - Existing tests cover missing grant before network, disallowed host/method/scheme/private targets before network, timeout/redaction, response truncation, no network without HTTPS import, and ordinary Tool result path.

Reviewer validation:
- Static diff/code review and worktree status inspection.
- Orchestrator-reported successful runs were reviewed:
  - `cargo fmt --check`
  - `cargo check -p pod -p manifest -p yoi`
  - focused pod/manifest/yoi plugin tests
  - `git diff --check`
  - `nix build .#yoi --no-link`

Conclusion:
- Previous request_changes blockers are resolved.
- Implementation satisfies the recorded Ticket acceptance criteria and is ready for merge/validation in the Orchestrator worktree.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T15:32:56Z -->

## Implementation report

Merge / validation report:

Implementation branch `impl/00001KVFDX9AF-plugin-https-host-api` は Orchestrator worktree の `orchestration` branch に merge 済み。

Merge:
- merge commit: `6beb8625` — `merge: plugin https host api`
- implementation commits:
  - `7377527f` — `plugin: implement https host api`
  - `85683f17` — `plugin: harden https target validation`
- changed files:
  - `Cargo.lock`
  - `crates/manifest/src/plugin.rs`
  - `crates/pod/Cargo.toml`
  - `crates/pod/src/feature/plugin.rs`
  - `crates/yoi/src/plugin_cli.rs`
  - `package.nix`

Validation in Orchestrator worktree after merge:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin::tests -- --nocapture` — passed; 39 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Review:
- First review requested changes for IPv4-mapped IPv6 private/local bypass and DNS validation TOCTOU.
- Coder fixed target validation by applying IPv4 checks to mapped/compatible IPv6 forms and binding DNS validation to reqwest via validated address pinning.
- Re-review `yoi-reviewer-00001KVFDX9AF-r2` returned `approve` with no remaining blockers.

Outcome:
- Granted HTTPS host API is implemented for WASM Plugin Tools.
- Missing grant denies before network access.
- Host/method/path allowlist and HTTPS-only/private-local target restrictions are represented and tested.
- DNS validation is pinned to the actual reqwest connection path.
- Request/response bounds, timeout, no redirects, `no_proxy()`, secret redaction, ordinary Tool result path, and CLI inspection compatibility are present.
- `fs` host API and Component Model migration remain non-goals for this Ticket.
- Root/original workspace was not read/written/merged/validated for this Ticket, per Panel Queue instruction.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T15:33:57Z -->

## Implementation report

Merge / validation report:

Implementation branch `impl/00001KVFDX9AF-plugin-https-host-api` は Orchestrator worktree の `orchestration` branch に merge 済み。

Merge:
- merge commit: `6beb8625` — `merge: plugin https host api`
- implementation commits:
  - `7377527f` — `plugin: implement https host api`
  - `85683f17` — `plugin: harden https target validation`
- changed files:
  - `Cargo.lock`
  - `crates/manifest/src/plugin.rs`
  - `crates/pod/Cargo.toml`
  - `crates/pod/src/feature/plugin.rs`
  - `crates/yoi/src/plugin_cli.rs`
  - `package.nix`

Validation in Orchestrator worktree after merge:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin::tests -- --nocapture` — passed; 39 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Review:
- First review requested changes for IPv4-mapped IPv6 private/local bypass and DNS validation TOCTOU.
- Coder fixed target validation by applying IPv4 checks to mapped/compatible IPv6 forms and binding DNS validation to reqwest via validated address pinning.
- Re-review `yoi-reviewer-00001KVFDX9AF-r2` returned `approve` with no remaining blockers.

Outcome:
- Granted HTTPS host API is implemented for WASM Plugin Tools.
- Missing grant denies before network access.
- Host/method/path allowlist and HTTPS-only/private-local target restrictions are represented and tested.
- DNS validation is pinned to the actual reqwest connection path.
- Request/response bounds, timeout, no redirects, `no_proxy()`, secret redaction, ordinary Tool result path, and CLI inspection compatibility are present.
- `fs` host API and Component Model migration remain non-goals for this Ticket.
- Root/original workspace was not read/written/merged/validated for this Ticket, per Panel Queue instruction.

---
