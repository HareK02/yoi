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

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T11:03:51Z -->

## Plan

Review dispatch:

Reviewer Pod `yoi-reviewer-00001KVFD3YSV` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVFD3YSV-plugin-cli-inspection`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commit: `462de32a`

Reviewer focus:
- recorded intent / invariants / acceptance criteria に対する適合性。
- `yoi plugin list` / `yoi plugin show <ref>` と `--json` が read-only typed inspection surface になっていること。
- Plugin WASM / Tool code / hooks / services / ingress を実行していないこと。
- human output が typed report の thin formatting であり、JSON structure が stable であること。
- active / disabled / missing / rejected / partial の status vocabulary と package/tool-level outcome が適切であること。
- missing ref / ambiguous unqualified ref / invalid manifest / digest-version-source mismatch / missing grant-denied permission / unsupported permission diagnostics が bounded かつ安全であること。
- secrets/raw untrusted content leakage がないこと。
- product CLI ownership と lower-crate boundary が保たれていること。
- future `https` / `fs` host API Tickets が inspection output を拡張しやすいこと。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree へ merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVFD3YSV at: 2026-06-19T11:07:58Z status: request_changes -->

## Review: request changes

Review result: request_changes

対象:
- Ticket: `00001KVFD3YSV`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commit: `462de32a` (`plugin: add cli inspection`)

Blocker:
- Status vocabulary が Ticket の binding acceptance criteria を満たしていない。

Evidence:
- Ticket body では package status として `active`, `disabled`, `missing`, `rejected`, `partial` を要求している。
- 実装は代わりに以下を JSON / human output に出している。
  - `enabled`
  - `enabled-with-diagnostics`
  - `configured-blocked`
  - `configured-missing`
  - `discovered`
  - `diagnostic`
- `crates/yoi/src/plugin_cli.rs:605-617` が required vocabulary ではない status strings を計算している。
- Human list/show は `item.status` を直接表示している。
- JSON output も同じ item/snapshot を serialize している。
- Tests も現状の wrong vocabulary を固定しており、`enabled` / `enabled-with-diagnostics` を期待している。

Why this blocks acceptance:
- Valid enabled Plugin が `active` として出ない。
- discovered but not enabled Plugin が `disabled` として出ない。
- enabled but missing package が `missing` として出ない。
- invalid manifest / incompatible api / digest mismatch / grant mismatch / invalid schema などが `rejected` として出ない。
- mixed tool eligibility / partial rejection が `partial` として表現されない。
- Status は typed JSON/report surface の一部なので、単なる human formatting の差分ではない。

Additional concerns to address with the blocker:
- CLI contract の acceptance coverage が不足している。
  - JSON list/show structure。
  - human list の active / disabled / rejected / missing。
  - show active with grants。
  - invalid manifest。
  - digest mismatch。
  - missing grant。
  - ambiguous ref。
  - no runtime execution。
- 現在の `yoi` tests は resolved package、missing ref、grant mismatch、parser behavior だけで、requested status vocabulary や JSON structure を十分に pin していない。
- Human `list` output が enabled surfaces を表示せず、`tools=<len>` と diagnostics count に留まっている。Ticket の human overview requirement は surfaces を明示している。

Positive findings:
- CLI surface は `crates/yoi` 側にあり、lower crates が product CLI façade に依存している様子はない。
- Inspection path は read-only / non-executing に見える。
  - snapshot build は config/profile resolution、plugin discovery、enabled-plugin resolution、static inspection に留まる。
  - static inspection は manifest/runtime metadata、grants、host API permissions、tool eligibility を見るだけで、WASM runtime/tool execution path を呼んでいない。
- Missing / ambiguous CLI refs は bounded message で fail closed している。
- Diagnostics/output は一部 bounded で、list truncation / diagnostic truncation / `bound_text` がある。

Requested changes:
- typed report / JSON / human output の status vocabulary を Ticket 指定の `active`, `disabled`, `missing`, `rejected`, `partial` に合わせる。
- `partial` の判定を package usable but some surfaces/tools rejected の意味で表現する。
- Human `list` output に enabled surfaces を出す。
- Tests を更新/追加し、requested vocabulary と JSON structure を pin する。
- Acceptance coverage gap のうち、少なくとも status vocabulary / JSON list-show / active-disabled-rejected-missing / missing grant / ambiguous ref / no-execution の主要契約を明確に覆う。

Validation note:
- Reviewer は read-only source/diff inspection を実施。
- cargo/nix validation は Orchestrator reported validation を確認した。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T11:08:37Z -->

## Plan

Review follow-up dispatch:

Reviewer `yoi-reviewer-00001KVFD3YSV` は `request_changes` を返したため、Ticket review として記録し、既存 Coder Pod `yoi-coder-00001KVFD3YSV` に修正依頼を送った。

Main blocker:
- typed report / JSON / human output の status vocabulary が Ticket の binding acceptance criteria と一致していない。
- Ticket は `active`, `disabled`, `missing`, `rejected`, `partial` を要求しているが、実装は `enabled`, `enabled-with-diagnostics`, `configured-blocked`, `configured-missing`, `discovered`, `diagnostic` を出している。
- status は typed JSON/report surface の一部なので、human formatting だけの差分ではない。

Required fixes sent to Coder:
- status vocabulary を `active`, `disabled`, `missing`, `rejected`, `partial` に合わせる。
- `partial` を、package usable だが一部 surface/tool が rejected/diagnostic である状態として表現する。
- human `list` output に enabled surfaces を表示する。
- requested vocabulary と JSON structure を tests で pin する。
- JSON list/show、human active/disabled/rejected/missing、show active with grants、invalid manifest、digest mismatch、missing grant、ambiguous ref、no-execution の主要契約を追加/補強する。

Next:
- Coder の修正 commit / validation report を待つ。
- 修正後、同じ CLI contract / read-only no-execution / bounded diagnostics focus で再レビューする。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T11:17:04Z -->

## Implementation report

Coder review-fix implementation report received / Orchestrator validation:

Incremental fix commit:
- `b5f10ab7` — `plugin: align inspection statuses`

Summary:
- Plugin inspection typed report / JSON / human output の status vocabulary を Ticket 指定の `active`, `disabled`, `missing`, `rejected`, `partial` に合わせた。
- `partial` status を mixed eligibility / diagnostics のある usable package として表現。
- Human `yoi plugin list` output に enabled surfaces を含めるように更新。
- JSON list/show structure と status vocabulary を tests で pin。
- active / disabled / missing / rejected / partial、missing grant、ambiguous ref、no-execution などの CLI contract coverage を補強。

Files changed in incremental fix:
- `crates/yoi/src/plugin_cli.rs`

Orchestrator evidence checked:
- Pod output notification を確認。
- implementation worktree は clean。
- branch `impl/00001KVFD3YSV-plugin-cli-inspection` HEAD は `b5f10ab7`。
- incremental diff stat from previous review target `462de32a..b5f10ab7`: 1 file changed, 266 insertions, 42 deletions。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo test -p yoi plugin -- --nocapture` — passed; 8 passed, 0 failed
- `cargo test -p pod plugin -- --nocapture` — passed; 29 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- previous `request_changes` blocker が解消されたか、read-only Reviewer Pod で再レビューする。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T11:17:48Z -->

## Plan

Review re-dispatch:

Reviewer Pod `yoi-reviewer-00001KVFD3YSV-r2` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVFD3YSV-plugin-cli-inspection`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commits:
  - `462de32a` — initial implementation
  - `b5f10ab7` — status vocabulary / test coverage fix

Reviewer focus:
- previous `request_changes` blocker が解消されているか。
- typed report / JSON / human output が `active`, `disabled`, `missing`, `rejected`, `partial` vocabulary に揃っているか。
- `partial` semantics が usable package with rejected/diagnostic surfaces/tools として表現されているか。
- human `list` output に enabled surfaces が含まれるか。
- JSON list/show structure と CLI contract tests が十分に pin されているか。
- read-only/no-execution、bounded diagnostics、missing/ambiguous ref fail-closed、grant/digest/invalid diagnostics、product CLI boundary、no secrets/unbounded raw output が維持されているか。
- future `https` / `fs` host API inspection extension を妨げていないか。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree への merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVFD3YSV-r2 at: 2026-06-19T11:23:25Z status: request_changes -->

## Review: request changes

Review result: request_changes

対象:
- Ticket: `00001KVFD3YSV`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commits:
  - `462de32a` — `plugin: add cli inspection`
  - `b5f10ab7` — `plugin: align inspection statuses`

Previous blocker status:
- status vocabulary blocker は解消済み。
- `active`, `disabled`, `missing`, `rejected`, `partial` が typed report / JSON / human output に使われ、old vocabulary は出力から除去されている。

Remaining blocker:
- typed report / JSON / human output が Plugin API/schema version と package path / inspectable package location をまだ出していない。

Evidence:
- Ticket requirements / acceptance は list overview に package path と API version、show に package path と version / API version を要求している。
- `PluginInspectionItem` は `package`, `version`, `digest` を持つが、`api_version` / `schema_version` と `package_path` field がない。
- builder は `package` を package label から埋めており、inspectable package location/path ではない。
- Human `list` は `version`, `digest`, `source`, `enabled_surfaces`, tools, diagnostics を表示するが、API version と package/path を表示しない。
- Human `show` は `package`, `version`, `digest` などを表示するが、API/schema version を表示しない。
- Tests は JSON fields を一部 pin しているが、API/schema version や package path presence は pin していない。

Why this blocks acceptance:
- CLI inspection の主目的は、Plugin package discovery / enablement resolution / source/ref/digest/version / requested permissions / grants / diagnostics を read-only に確認できること。
- Ticket は package path と API version を明示的な list/show output contract として要求しており、typed JSON/report surface に含まれていないと、human output の薄い formatting でも確認できない。

Positive findings:
- read-only/no-execution boundary は維持されているように見える。
- future `https` / `fs` inspection extension は `PluginStaticInspection.host_apis` と human `host_api_permissions` rendering があり、拡張可能に見える。
- bounded diagnostics は `MAX_*` constants と `bound_text` により維持されている。
- Status vocabulary fix は requested terms を満たし、tests でも pin されている。

Requested changes:
- typed report / JSON に Plugin API/schema version field を追加する。
- typed report / JSON に package path / inspectable package location field を追加する。
- human `list` overview に package path/location と API/schema version を表示する。
- human `show` に package path/location と API/schema version を表示する。
- Tests で JSON list/show と human list/show の API/schema version と package path/location presence を pin する。
- 既存の bounded output / no secret leakage 方針に従い、path/location 表示は必要最小限かつ deterministic にする。

Reviewer validation:
- Final branch と `462de32a..b5f10ab7` の static review。
- Ticket requirements を再確認。
- `git diff --check develop...HEAD` — passed。
- targeted `rg` for old status terms。
- Cargo/nix validation は Orchestrator reported validation を確認。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T11:24:06Z -->

## Plan

Review follow-up dispatch:

Reviewer `yoi-reviewer-00001KVFD3YSV-r2` は `request_changes` を返したため、Ticket review として記録し、既存 Coder Pod `yoi-coder-00001KVFD3YSV` に修正依頼を送った。

Resolved:
- Previous status vocabulary blocker は解消済み。`active`, `disabled`, `missing`, `rejected`, `partial` は typed report / JSON / human output と tests に反映されている。

Remaining blocker:
- typed report / JSON / human output に Plugin API/schema version と package path / inspectable package location が不足している。
- Ticket は list overview に package path と API version、show に package path と version / API version を要求している。
- 現在の `PluginInspectionItem` は `package`, `version`, `digest` を持つが、`api_version` / `schema_version` と `package_path` field がない。
- `package` は package label 由来であり、inspectable package location/path ではない。

Required fixes sent to Coder:
- typed report / JSON に Plugin API/schema version field を追加する。
- typed report / JSON に package path / inspectable package location field を追加する。
- human `list` overview に package path/location と API/schema version を表示する。
- human `show` に package path/location と API/schema version を表示する。
- JSON list/show と human list/show の API/schema version と package path/location presence を tests で pin する。
- path/location 表示は bounded/deterministic にし、secret/raw untrusted content leakage を避ける。

Next:
- Coder の修正 commit / validation report を待つ。
- 修正後、同じ typed report contract / read-only no-execution / bounded diagnostics focus で再レビューする。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T11:32:48Z -->

## Implementation report

Coder review-fix implementation report received / Orchestrator validation:

Incremental fix commit:
- `dfa966db` — `plugin: report inspection package metadata`

Summary:
- Plugin inspection typed report / JSON / human output に package path / inspectable package location と Plugin API/schema version を追加。
- Human `yoi plugin list` overview に package path/location と API/schema version を表示。
- Human `yoi plugin show <ref>` に package path/location と API/schema version を表示。
- JSON list/show と human list/show tests で package path/location と API/schema version presence を pin。
- Existing status vocabulary fix (`active`, `disabled`, `missing`, `rejected`, `partial`) は維持。

Files changed in incremental fix:
- `crates/yoi/src/plugin_cli.rs`

Orchestrator evidence checked:
- Pod output notification を確認。
- implementation worktree は clean。
- branch `impl/00001KVFD3YSV-plugin-cli-inspection` HEAD は `dfa966db`。
- incremental diff stat from previous review target `b5f10ab7..dfa966db`: 1 file changed, 129 insertions, 3 deletions。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo test -p yoi plugin -- --nocapture` — passed; 8 passed, 0 failed
- `cargo test -p pod static_inspection -- --nocapture` — passed; 2 passed, 0 failed
- `cargo test -p pod plugin -- --nocapture` — passed; 29 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- previous `request_changes` blocker が解消されたか、read-only Reviewer Pod で再レビューする。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T11:34:29Z -->

## Plan

Review re-dispatch:

Reviewer Pod `yoi-reviewer-00001KVFD3YSV-r3` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVFD3YSV-plugin-cli-inspection`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commits:
  - `462de32a` — initial implementation
  - `b5f10ab7` — status vocabulary fix
  - `dfa966db` — package path / API-schema version metadata fix

Reviewer focus:
- previous package path / API-schema version blocker が typed report / JSON / human list / human show で解消されているか。
- tests が package path/location と API/schema version presence を十分に pin しているか。
- status vocabulary は引き続き `active`, `disabled`, `missing`, `rejected`, `partial` だけを使い、semantics が正しいか。
- read-only/no-execution、bounded diagnostics、missing/ambiguous ref fail-closed、grant/digest/invalid diagnostics、product CLI boundary、no secrets/unbounded raw output が維持されているか。
- future `https` / `fs` host API inspection extension を妨げていないか。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree への merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVFD3YSV-r3 at: 2026-06-19T11:41:24Z status: request_changes -->

## Review: request changes

Review result: request_changes

対象:
- Ticket: `00001KVFD3YSV`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commits:
  - `462de32a` — `plugin: add cli inspection`
  - `b5f10ab7` — `plugin: align inspection statuses`
  - `dfa966db` — `plugin: report inspection package metadata`

Resolved:
- Previous metadata blocker is resolved. Typed JSON/report fields now include `package_path`, `schema_version`, and `api_version`; human list/show also include those fields, and tests pin their presence.
- Status vocabulary remains correct: `active`, `disabled`, `missing`, `rejected`, `partial`.

Remaining blocker:
- Static inspection does not validate Tool definition/schema validity, so invalid Tool schemas can be reported as `active`.

Evidence:
- Ticket intent explicitly asks inspection to answer whether the “tool schema [is] valid” and says `rejected` covers “invalid schema etc.”
- Runtime registration path validates Tool names and input schemas:
  - `crates/pod/src/feature/plugin.rs:260-277` validates `validate_tool_name(...)` and `validate_input_schema(...)` before registration.
  - `validate_tool_name` / `validate_input_schema` are defined in `crates/pod/src/feature/plugin.rs`.
- Read-only inspection path does not mirror those checks:
  - `inspect_resolved_plugin_static` checks runtime ABI, host API permission diagnostics, and `authorize_plugin_tool(...)` grant/permission checks.
  - Tool eligibility is set from `authorize_plugin_tool(...)` only.
- Therefore a resolved plugin with valid grants but invalid `input_schema` can get `tool.eligible = true`, `static_eligible = true`, and top-level status `active`.
- Current CLI tests do not pin invalid Tool schema behavior; fixtures appear to use valid `input_schema = { type = "object" }` only.

Why this blocks acceptance:
- CLI inspection is supposed to provide static Tool/runtime eligibility and reject invalid schemas without executing Plugin code.
- If invalid Tool schemas are reported as `active`, the typed report / JSON / human output overstates eligibility and misses a required fail-closed diagnostic.

Requested changes:
- Static inspection should include the same static Tool-definition validity that registration enforces, at least Tool name and `input_schema` validation.
- Invalid tool name / invalid input schema should produce bounded diagnostics and mark affected tool/package as `rejected` or `partial` as appropriate.
- Add focused JSON/human tests for invalid Tool schema behavior.
- Preserve read-only / no-execution behavior.

Positive findings:
- Previous metadata blocker is fixed.
- Status vocabulary is fixed and tested.
- Read-only/no-execution shape remains good.
- Future `https` / `fs` inspection extension point remains present through static host API permission eligibility.

Validation note:
- Reviewer performed static code/diff review and relied on Orchestrator-reported cargo/nix validation. No mutations were made by Reviewer.

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T11:42:23Z -->

## Plan

Review follow-up dispatch:

Reviewer `yoi-reviewer-00001KVFD3YSV-r3` は `request_changes` を返したため、Ticket review として記録し、既存 Coder Pod `yoi-coder-00001KVFD3YSV` に修正依頼を送った。

Resolved:
- Status vocabulary blocker は解消済み。
- Package path/location + API/schema version blocker も解消済み。

Remaining blocker:
- Static inspection が Tool definition/schema validity を検証していないため、invalid Tool schema が `active` として報告され得る。
- Runtime registration path は `validate_tool_name(...)` と `validate_input_schema(...)` を実行しているが、read-only inspection path の `inspect_resolved_plugin_static` は runtime ABI / host API permission / `authorize_plugin_tool(...)` grant check に留まり、同じ static Tool-definition validity を mirror していない。
- Ticket intent は “tool schema [is] valid” の inspection と `invalid schema` を `rejected` として扱うことを要求している。

Required fixes sent to Coder:
- Static inspection に、registration path と同等の static Tool definition validity を追加する。少なくとも Tool name と `input_schema` validation を含める。
- Invalid tool name / invalid input schema は bounded diagnostics にする。
- affected tool/package は全 tools invalid/unusable なら `rejected`、一部 usable / 一部 invalid なら `partial` として表現する。
- invalid Tool schema behavior の focused JSON/human tests を追加する。
- read-only/no-execution behavior を維持する。

Next:
- Coder の修正 commit / validation report を待つ。
- 修正後、static inspection schema validity / no-execution / typed status semantics を中心に再レビューする。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T13:35:49Z -->

## Implementation report

Coder review-fix implementation report received / Orchestrator validation:

Incremental fix commit:
- `982a1b75` — `plugin: validate inspected tool schemas`

Summary:
- Static Plugin inspection が Tool definition validity を検証するようになった。
  - invalid tool names
  - duplicate tool names
  - invalid `input_schema`
- Tool-definition diagnostics は bounded で JSON/human inspection output に含まれる。
- Invalid tool schema/name は tool を ineligible にする。
- Package status semantics:
  - 全 tools unusable の場合は `rejected`
  - 少なくとも1つ usable で一部 rejected の場合は `partial`
- read-only / no-execution behavior は維持。

Files changed in incremental fix:
- `crates/pod/src/feature/plugin.rs`
- `crates/yoi/src/plugin_cli.rs`

Coder reported validation:
- `cargo fmt` — passed
- `cargo test -p yoi plugin` — passed
- `cargo test -p pod static_inspection` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo fmt --check` — passed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Orchestrator evidence checked:
- Pod output を確認。
- implementation worktree は clean。
- branch `impl/00001KVFD3YSV-plugin-cli-inspection` HEAD は `982a1b75`。
- incremental diff stat from previous review target `dfa966db..982a1b75`: 2 files changed, 230 insertions, 3 deletions。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo test -p yoi plugin -- --nocapture` — passed; 10 passed, 0 failed
- `cargo test -p pod static_inspection -- --nocapture` — passed; 5 passed, 0 failed
- `cargo test -p pod plugin -- --nocapture` — passed; 33 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- previous `request_changes` blocker が解消されたか、read-only Reviewer Pod で再レビューする。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T13:36:54Z -->

## Plan

Review re-dispatch:

Reviewer Pod `yoi-reviewer-00001KVFD3YSV-r4` を read-only scope で起動した。

Review target:
- worktree: `/home/hare/Projects/yoi/.worktree/00001KVFD3YSV-plugin-cli-inspection`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commits:
  - `462de32a` — initial implementation
  - `b5f10ab7` — status vocabulary fix
  - `dfa966db` — package path / API-schema version metadata fix
  - `982a1b75` — static Tool schema/name validation fix

Reviewer focus:
- previous Tool name / `input_schema` static inspection blocker が解消されているか。
- invalid / duplicate Tool definitions が bounded diagnostics になり、package status が `rejected` / `partial` へ正しく反映されるか。
- invalid schema/name behavior の JSON/human tests が十分か。
- prior fixes: status vocabulary、package path/location、API/schema version が維持されているか。
- read-only/no-execution、bounded diagnostics、missing/ambiguous ref fail-closed、grant/digest/invalid diagnostics、product CLI boundary、no secrets/unbounded raw output が維持されているか。
- future `https` / `fs` host API inspection extension を妨げていないか。

Related queue note:
- 新規 queued Ticket `00001KVG0HR96` は body/relations/plan/workspace state を確認し、Component Model runtime migration が Plugin runtime/grant/inspection/packaging に広く触れる migration boundary であるため、現在の `00001KVFD3YSV` review outcome まで queued のまま待機する waiting-capacity note を記録済み。

Next:
- Reviewer output を確認し、approve なら Orchestrator worktree への merge/validation/cleanup/close-ready 処理へ進む。request_changes なら blocker を記録して Coder に戻す。

---

<!-- event: review author: yoi-reviewer-00001KVFD3YSV-r4 at: 2026-06-19T13:45:01Z status: request_changes -->

## Review: request changes

Review result: request_changes

対象:
- Ticket: `00001KVFD3YSV`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commits:
  - `462de32a` — `plugin: add cli inspection`
  - `b5f10ab7` — `plugin: align inspection statuses`
  - `dfa966db` — `plugin: report inspection package metadata`
  - `982a1b75` — `plugin: validate inspected tool schemas`

Resolved:
- round-4 Tool schema/name blocker は解消済み。
- Static inspection は duplicate tool names、invalid names、invalid `input_schema`、recursive/schema member issues を検出する。
- Invalid Tool definitions は typed static inspection で ineligible になり、bounded diagnostics が出る。
- `partial` は mixed usable/unusable tools を表現できる。
- JSON/human invalid schema/name coverage が追加されている。
- Prior metadata/status fixes も維持されている。

Remaining blocker:
- Configured invalid / incompatible package が `rejected` ではなく `missing` と報告され得る。

Evidence:
- Ticket acceptance は `enabled but absent package => missing` と `invalid manifest / incompatible API => rejected with diagnostic` を区別している。
- `crates/yoi/src/plugin_cli.rs` の snapshot construction は configured enablements を diagnostics より先に記録する。
  - config enablement が `configured = true` と inferred `package_path` を設定する。
  - discovery/resolution diagnostics for invalid package files は後から attach される。
- しかし `ItemBuilder::finish` の status precedence は概ね次の順序になっている。

```rust
} else if self.discovered && !self.configured {
    "disabled"
} else if self.configured && !self.discovered {
    "missing"
} else {
    "rejected"
}
```

- そのため `project:bad` が明示 enable され、`.yoi/plugins/bad.yoi-plugin` が存在するが manifest invalid / unsupported API version の場合、discovery diagnostic は出るが discovered package は作られない。builder は `configured=true`, `discovered=false`, diagnostics present となり、status が `missing` になる。
- Current tests は unconfigured invalid manifests as rejected と configured digest mismatch as rejected を覆るが、configured invalid/incompatible packages を覆っていない。

Why this blocks acceptance:
- CLI inspection は package discovery / enablement resolution の debugging surface であり、存在するが invalid/incompatible な package と、存在しない package を区別する必要がある。
- invalid manifest / incompatible API を `missing` と表示すると、原因調査を誤誘導し、Ticket の `rejected` semantics を満たさない。

Requested changes:
- Configured package に discovery/resolution diagnostic が attach されている場合、absent package `missing` よりも `rejected` を優先する。
- enabled but truly absent package だけを `missing` にする。
- Add tests for configured invalid manifest / incompatible API package reporting `rejected` with diagnostic in JSON/human output.
- Preserve current fixes: status vocabulary, package path/API fields, Tool schema validation, read-only/no-execution.

Reviewer validation:
- Static review only, per read-only boundary and because Orchestrator already reran validation.
- Inspected final branch clean at HEAD `982a1b75`, incremental diff `dfa966db..982a1b75`, final implementation in `crates/pod/src/feature/plugin.rs`, `crates/yoi/src/plugin_cli.rs`, and relevant Ticket acceptance text.

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T13:45:45Z -->

## Plan

Review follow-up dispatch:

Reviewer `yoi-reviewer-00001KVFD3YSV-r4` は `request_changes` を返したため、Ticket review として記録し、既存 Coder Pod `yoi-coder-00001KVFD3YSV` に修正依頼を送った。

Resolved:
- Tool schema/name static inspection blocker は解消済み。
- Prior status vocabulary、package path/location、API/schema version fixes も維持されている。

Remaining blocker:
- Configured invalid / incompatible package が `rejected` ではなく `missing` と報告され得る。
- Ticket acceptance は enabled but absent package => `missing` と、invalid manifest / incompatible API => `rejected` with diagnostic を区別している。
- `ItemBuilder::finish` の status precedence により、明示 enable された `.yoi/plugins/bad.yoi-plugin` が存在するが invalid manifest / unsupported API version の場合、diagnostic は attach されるが discovered package は作られず、`configured=true`, `discovered=false`, diagnostics present で `missing` になる可能性がある。

Required fixes sent to Coder:
- Configured package に discovery/resolution diagnostic が attach されている場合、absent package `missing` より `rejected` を優先する。
- `missing` は enabled/configured package refs が本当に absent の場合だけにする。
- configured invalid manifest / incompatible API package が JSON/human output で `rejected` + diagnostic になる tests を追加する。
- Prior fixes: status vocabulary、package path/API fields、Tool schema validation、read-only/no-execution、bounded diagnostics を維持する。

Next:
- Coder の修正 commit / validation report を待つ。
- 修正後、configured invalid/incompatible package status semantics を中心に再レビューする。

---

<!-- event: review author: yoi-reviewer-00001KVFD3YSV-r4 at: 2026-06-19T13:51:48Z status: request_changes -->

## Review: request changes

Review result: request_changes

対象:
- Ticket: `00001KVFD3YSV`
- branch: `impl/00001KVFD3YSV-plugin-cli-inspection`
- commits:
  - `462de32a` — `plugin: add cli inspection`
  - `b5f10ab7` — `plugin: align inspection statuses`
  - `dfa966db` — `plugin: report inspection package metadata`
  - `982a1b75` — `plugin: validate inspected tool schemas`

Resolved:
- round-4 Tool schema/name blocker は解消済み。
- Static inspection は duplicate tool names、invalid names、invalid `input_schema`、recursive/schema member issues を検出する。
- Invalid Tool definitions は typed static inspection で ineligible になり、bounded diagnostics が出る。
- `partial` は mixed usable/unusable tools を表現できる。
- JSON/human invalid schema/name coverage が追加されている。
- Prior metadata/status fixes も維持されている。

Remaining blocker:
- Configured invalid / incompatible package が `rejected` ではなく `missing` と報告され得る。

Evidence:
- Ticket acceptance は `enabled but absent package => missing` と `invalid manifest / incompatible API => rejected with diagnostic` を区別している。
- `crates/yoi/src/plugin_cli.rs` の snapshot construction は configured enablements を diagnostics より先に記録する。
  - config enablement が `configured = true` と inferred `package_path` を設定する。
  - discovery/resolution diagnostics for invalid package files は後から attach される。
- しかし `ItemBuilder::finish` の status precedence は概ね次の順序になっている。

```rust
} else if self.discovered && !self.configured {
    "disabled"
} else if self.configured && !self.discovered {
    "missing"
} else {
    "rejected"
}
```

- そのため `project:bad` が明示 enable され、`.yoi/plugins/bad.yoi-plugin` が存在するが manifest invalid / unsupported API version の場合、discovery diagnostic は出るが discovered package は作られない。builder は `configured=true`, `discovered=false`, diagnostics present となり、status が `missing` になる。
- Current tests は unconfigured invalid manifests as rejected と configured digest mismatch as rejected を覆るが、configured invalid/incompatible packages を覆っていない。

Why this blocks acceptance:
- CLI inspection は package discovery / enablement resolution の debugging surface であり、存在するが invalid/incompatible な package と、存在しない package を区別する必要がある。
- invalid manifest / incompatible API を `missing` と表示すると、原因調査を誤誘導し、Ticket の `rejected` semantics を満たさない。

Requested changes:
- Configured package に discovery/resolution diagnostic が attach されている場合、absent package `missing` よりも `rejected` を優先する。
- enabled but truly absent package だけを `missing` にする。
- Add tests for configured invalid manifest / incompatible API package reporting `rejected` with diagnostic in JSON/human output.
- Preserve current fixes: status vocabulary, package path/API fields, Tool schema validation, read-only/no-execution.

Reviewer validation:
- Static review only, per read-only boundary and because Orchestrator already reran validation.
- Inspected final branch clean at HEAD `982a1b75`, incremental diff `dfa966db..982a1b75`, final implementation in `crates/pod/src/feature/plugin.rs`, `crates/yoi/src/plugin_cli.rs`, and relevant Ticket acceptance text.

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T13:53:00Z -->

## Plan

Review follow-up dispatch:

Reviewer `yoi-reviewer-00001KVFD3YSV-r4` は `request_changes` を返したため、Ticket review として記録し、既存 Coder Pod `yoi-coder-00001KVFD3YSV` に修正依頼を送った。

Resolved:
- Tool schema/name static inspection blocker は解消済み。
- Prior status vocabulary、package path/location、API/schema version fixes も維持されている。

Remaining blocker:
- Configured invalid / incompatible package が `rejected` ではなく `missing` と報告され得る。
- Ticket acceptance は enabled but absent package => `missing` と、invalid manifest / incompatible API => `rejected` with diagnostic を区別している。
- `ItemBuilder::finish` の status precedence により、明示 enable された `.yoi/plugins/bad.yoi-plugin` が存在するが invalid manifest / unsupported API version の場合、diagnostic は attach されるが discovered package は作られず、`configured=true`, `discovered=false`, diagnostics present で `missing` になる可能性がある。

Required fixes sent to Coder:
- Configured package に discovery/resolution diagnostic が attach されている場合、absent package `missing` より `rejected` を優先する。
- `missing` は enabled/configured package refs が本当に absent の場合だけにする。
- configured invalid manifest / incompatible API package が JSON/human output で `rejected` + diagnostic になる tests を追加する。
- Prior fixes: status vocabulary、package path/API fields、Tool schema validation、read-only/no-execution、bounded diagnostics を維持する。

Next:
- Coder の修正 commit / validation report を待つ。
- 修正後、configured invalid/incompatible package status semantics を中心に再レビューする。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T13:54:05Z -->

## Implementation report

Coder review-fix implementation report received / Orchestrator validation:

Incremental fix commit:
- `a5f3b0b5` — `plugin: reject configured invalid packages`

Summary:
- Configured package refs with non-missing discovery/resolution diagnostics now report `rejected`, not `missing`.
- `missing` is preserved only for truly absent enabled/configured package refs.
- Added focused tests for configured invalid/incompatible packages:
  - configured invalid manifest reports `rejected`, not `missing`;
  - configured incompatible schema/API package reports `rejected`;
  - JSON and human output include rejection diagnostics while package path behavior remains pinned.
- Prior fixes are preserved:
  - status vocabulary;
  - package path/API/schema output;
  - Tool schema/name static validation;
  - read-only/no-execution inspection;
  - bounded diagnostics.

Files changed in incremental fix:
- `crates/yoi/src/plugin_cli.rs`

Coder reported validation:
- `cargo fmt` — passed
- `cargo test -p yoi plugin` — passed
- `cargo test -p pod static_inspection` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo fmt --check` — passed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed
- Additional verification:
  - `cargo test -p yoi configured_invalid_or_incompatible_package_is_rejected_not_missing` — passed
  - `cargo fmt --check` — passed
  - `git diff --check` — passed

Orchestrator evidence checked:
- Pod output を確認。
- implementation worktree は clean。
- branch `impl/00001KVFD3YSV-plugin-cli-inspection` HEAD は `a5f3b0b5`。
- incremental diff stat from previous review target `982a1b75..a5f3b0b5`: 1 file changed, 124 insertions, 2 deletions。

Validation re-run by Orchestrator in implementation worktree:
- `cargo fmt --check` — passed
- `cargo check -p yoi -p pod -p manifest` — passed
- `cargo test -p yoi plugin -- --nocapture` — passed; 10 passed, 0 failed
- `cargo test -p pod static_inspection -- --nocapture` — passed; 4 passed, 0 failed
- `cargo test -p pod plugin -- --nocapture` — passed; 31 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Routing classification: review_needed

Next action:
- previous `request_changes` blocker が解消されたか、read-only Reviewer Pod で再レビューする。

---
