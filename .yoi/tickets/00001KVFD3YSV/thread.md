<!-- event: create author: "yoi ticket" at: 2026-06-19T07:39:23Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-19T10:19:28Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-19T10:22:09Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body には、read-only Plugin inspection CLI の intent、binding invariants、acceptance criteria、implementation latitude、validation、escalation conditions が揃っている。
- 依存 relation のうち `00001KV5R5V2S` と `00001KV5W3PJ3` は closed で blocker ではない。関連 closed Tickets も context として確認した。
- Risk domain は Plugin package / grant diagnostics / public CLI だが、Ticket は read-only inspection に限定し、Plugin code / WASM / Tool 実行 / package mutation / hidden model context injection を明示的に禁止している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。
- 同時 queued の `00001KVFDX9AF` / `00001KVFDX9AY` は host API implementation で Plugin manifest/grant/runtime/diagnostic 周辺に触るため、conflict / reviewer-coder bottleneck を避け、まず read-only CLI inspection を開始する。両 host API Tickets には waiting / do_not_parallelize plan record を残した。

Evidence checked:
- Ticket `00001KVFD3YSV` body / thread / artifacts。
- `TicketRelationQuery(00001KVFD3YSV)`: depends_on の `00001KV5R5V2S` と `00001KV5W3PJ3` は closed、その他 related/duplicate/supersedes は acceptance blocker ではない。
- `TicketOrchestrationPlanQuery(00001KVFD3YSV)`: 既存 blocker/conflict record なし。今回 `accepted_plan` を記録済み。
- 関連 Tickets `00001KV5R5V2S`, `00001KV5W3PJ3`, `00001KV5W3PHA`, `00001KV5W3PHW` は closed context として確認。
- 同時 queued Tickets `00001KVFDX9AF` / `00001KVFDX9AY` body/relation/plan を確認し、host API implementation は conflict risk が高いため queued のまま待機判断。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`: clean。
- Existing worktree/branch: `00001KVFD3YSV` / plugin-cli-inspection matching branch/worktree はなし。
- Visible Pods: self と peer/restorable Pod のみ。spawned child はなし。
- Current code map:
  - product CLI: `crates/yoi/src/**/*.rs`
  - Plugin manifest / requested permissions: `crates/manifest/src/plugin.rs`
  - Plugin feature / resolver / diagnostics / tool installation: `crates/pod/src/feature/plugin.rs`, `crates/pod/src/pod.rs`
  - Existing grant enforcement from `00001KV5W3PJ3` is merged in Orchestrator branch。

IntentPacket:

Intent:
- `yoi plugin list` と `yoi plugin show <ref>` の read-only CLI inspection を追加する。
- Plugin package discovery、enablement resolution、source/ref/digest/version、requested permissions、configured grants、grant/diagnostic outcome、static Tool/runtime eligibility を人間が確認できるようにする。

Binding decisions / invariants:
- Inspection CLI は read-only。Plugin code / WASM / Tool execution は行わない。
- Package install/remove/enable/disable/grant mutation は行わない。
- Secrets / raw config secret values / unbounded diagnostics を表示しない。
- Unknown permission / unsupported grant / missing grant / digest/version/source mismatch は fail-closed diagnostic として表示する。
- Existing Plugin permission grant enforcement を弱めない。CLI は権限付与の代替ではない。
- Product CLI façade `yoi` が public command を持ち、lower crates は implementation library 境界を保つ。

Requirements / acceptance criteria:
- `yoi plugin list` が configured/discovered Plugin packages の bounded overview を出せる。
- `yoi plugin show <ref>` が1件の package/source/ref/digest/version/requested permissions/grants/diagnostics/tools/static eligibility を表示できる。
- No grant / mismatched grant / unsupported permission / invalid manifest の状態が read-only diagnostic として分かる。
- Missing package/ref は bounded error になる。
- JSON output または structured-friendly output が既存 CLI style に合う形で利用できる。
- Running the inspection command does not execute plugin WASM or Tool code。
- Tests cover list/show success, missing ref, grant mismatch/diagnostic, and no-execution behavior where practical。

Implementation latitude:
- Command shape / output detail / JSON flag は既存 `yoi` CLI patterns に合わせてよい。ただし command names are binding: `yoi plugin list`, `yoi plugin show <ref>`。
- Resolver/helper placement は existing Plugin resolver / manifest / feature codeに合わせてよい。
- If full discovery graph is expensive, implement bounded static resolution sufficient for configured packages and diagnostics, and report limitations。

Escalate if:
- CLI inspection requires executing Plugin WASM / Tool code。
- Existing Plugin resolver does not expose enough data without broad feature API redesign。
- Public output schema needs product decision beyond compact human-readable + optional JSON pattern。
- Grant/permission model must be redesigned rather than inspected。
- Secrets or untrusted package content would need to be printed verbatim。

Validation:
- Focused CLI tests for `yoi plugin list` / `yoi plugin show`。
- Focused Plugin inspection helper / resolver tests。
- Relevant `cargo test` / `cargo check` for `yoi`, `pod`, `manifest` as changed。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi` only if dependency/Cargo.lock/package-source-filter changes occur。

Critical risks / reviewer focus:
- Inspection command accidentally executing Plugin code / WASM。
- CLI output leaking secrets, raw untrusted data, or unbounded diagnostics。
- CLI implying grants are authorization when enforcement still belongs to runtime/Tool path。
- Grant mismatch / no grant / unsupported permission not visible enough for debugging。
- Lower crates depending on product CLI façade。
- Future `https` / `fs` host API Tickets should be able to extend diagnostics without reworking the command boundary。

Next action:
- `queued -> inprogress` を記録し、Ticket records を Orchestrator worktree に commit してから、専用 implementation worktree を作成し Coder Pod を narrow write scope で起動する。root/original workspace は操作しない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-19T10:22:35Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_cli_inspection field: state -->

## State changed

Ticket body/thread, relation metadata, orchestration plan records, related closed Tickets, Orchestrator worktree, visible Pods, existing branch/worktree, and bounded Plugin CLI/code context were checked. Depends-on blockers are closed, no dirty-state blocker or missing planning decision was found. Risk is captured as invariants/reviewer focus. Host API Tickets queued in the same pass were left queued with conflict/waiting records. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T10:23:55Z -->

## Implementation report

Implementation start note:

`queued -> inprogress` acceptance、accepted plan、routing decision / IntentPacket、host API Tickets の waiting/conflict records を記録し、Orchestrator worktree で commit した後に、専用 implementation worktree と Coder Pod を起動した。

Worktree:
- `/home/hare/Projects/yoi/.worktree/00001KVFD3YSV-plugin-cli-inspection`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`

Coder Pod:
- `yoi-coder-00001KVFD3YSV`

Scope / boundaries:
- child runtime workspace root は read scope。
- implementation worktree は write scope。
- root/original workspace と Orchestrator worktree へは書き込まないよう指示済み。
- `.yoi/memory`、runtime/log/lock/local/secret-like `.yoi` paths は編集しないよう指示済み。

Related queued Tickets:
- `00001KVFDX9AF` https host API and `00001KVFDX9AY` fs host API remain queued with waiting/conflict notes because their implementation touches overlapping Plugin runtime/grant/diagnostic surfaces.

Next:
- Coder は read-only Plugin CLI inspection、focused tests、validation、implementation commit を行い、実装報告を返す予定。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T11:03:00Z -->

## Implementation report

Coder implementation report received / Orchestrator validation:

Implementation commit:
- `462de32a` — `plugin: add cli inspection`

Summary:
- Product CLI に read-only Plugin inspection commands を追加。
  - `yoi plugin list`
  - `yoi plugin show <ref>`
  - `--json` support
- Typed inspection report / formatting を追加し、human output は typed report の thin formatting として実装。
- Plugin resolver / diagnostics から package discovery、enablement、source/ref/digest/version、requested permissions、grants、tool/runtime static eligibility、diagnostics を inspection output に出せるようにした。
- Inspection path は Plugin WASM / Tool execution を行わない前提で実装。
- Bounded diagnostics / JSON-friendly structure を追加。

Files changed:
- `crates/manifest/src/plugin.rs`
- `crates/pod/src/feature/plugin.rs`
- `crates/yoi/src/main.rs`
- `crates/yoi/src/plugin_cli.rs`

Orchestrator evidence checked:
- Pod output notification を確認。
- implementation worktree は clean。
- branch `impl/00001KVFD3YSV-plugin-cli-inspection` HEAD は `462de32a`。
- merge-base `d51b610f` からの diff stat: 4 files changed, 1160 insertions, 2 deletions。
- `git diff --check $(git merge-base HEAD orchestration)..HEAD` — passed。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo test -p yoi plugin -- --nocapture` — passed; 4 passed, 0 failed
- `cargo test -p pod plugin -- --nocapture` — passed; 29 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- recorded intent / invariants / acceptance criteria に照らして、read-only Reviewer Pod で外部レビューする。
- 特に read-only/no-execution、JSON typed structure、bounded diagnostics、grant mismatch/no grant/invalid/ambiguous ref coverage、secrets leakage avoidance、product CLI / lower crate boundary、future host API extension point を確認する。

---
