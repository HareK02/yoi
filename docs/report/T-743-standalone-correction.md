# T-743 standalone source correction

## Accepted-plan correction and source boundary

The user withdrew the instruction to compose/wait for T-748. T-743 owns its
public UI/API/Tools/Prompt work and its own independent review/fix loop against
the existing develop contracts. Platform composition and target integration are
Orchestrator work. T-748 implementation or approval is **not** a T-743 return
gate. This does not reduce any T-743 product requirement or authorize ignoring
T-743-specific authorization, concurrency or public-operation defects.

Observed develop is `56795fb92cb9def9bcd701bd51d92e99526f8eff`; a normal fetch
confirmed no movement from the original T-743 base. Standalone implementation
`9b0d42fdbc2920736c3418bb894f26140e962bbc` adds the required public contracts and
semantic tests on that base. Published merge `5c5727e3f8ad85bddebda2672fd245a24317fc5f`
had added unrelated T-748 platform and dependent adapters at the withdrawn
instruction. Corrective commit `c6d73e82` uses a **normal mainline revert** of that
merge, not reset, rebase, force push or a replacement MR.

Before retaining historical report bodies, the corrective tree was compared and
matched `9b0d42fd` exactly. After retaining those reports, `git diff --quiet
9b0d42fd -- crates web tools resources openapi` still passed. Thus all source,
wire/generated contracts, UI/Tools/Prompt and semantic tests from the initial
standalone result survive without changes or weakened assertions.

Removed only the composition's shared DB/Host/cleanup execution owners, broad
async-platform and lifecycle caller changes, Workdir signing adapter, and the
new assignment/query execution adapters/test accessors and fixtures requiring
that removed owner. No incomplete executor facade or platform API was copied.
The baseline's synchronous execution placement is distinguished from T-743
product defects; it is not an excuse to overlook a newly introduced defect.
The implementation is separable: no T-743 product API, author/source fence or
receipt requires a T-748-only type.

Kept T-743's authenticated source attribution, current singleton/live binding
fence including replay, whole IMMEDIATE answer source/CAS/body/audit/receipt/
commit transaction, committed-input delivery scan/deduplication, sanitized
assignment diagnostics, retained history, typed SQL-before-page filters,
canonical/Browser routes and Normal Tools/WIP transport. Kept case/handling/
Worker/MR separation, same-assignment answer versus new-row reissue, stable tags
and closure history, effective Prompt/Profile/Flow and their original tests.

## Evidence and review ownership

Historical report bodies (`T-743-public-surface-validation.md`,
`T-743-T748-callers.md`, `T-743-T748-composition.md`), published commits and MR/
Ticket events remain evidence of what was done then. Their old composition-wait
instructions are superseded, not retroactively rewritten. T-748-specific reports
and tests remain readable in the published merge's Git history, but are not
current T-743 source contracts. This document carries the current correction and
new validation evidence.

Fresh trusted independent review on MR
`01a12d19-b61b-79a2-baca-89241fd3c3d3` judges the actual T-743 Ticket requirements,
authorization, concurrency and public operations against the exact normally
published corrected source. The review thread should include this correction
as an append-only event, not replace its original summary. The Coder tool set
has no MR-comment mutation tool; `OpenMergeRequest` on this repository would
conflict with the existing MR, not append a comment. The correction is therefore
supplied for the trusted review event body, without bypassing typed authority.
No target integration, Ticket/MR conclusion, live update or retained Worker/
Workdir cleanup is authorized or performed.

## Revalidation

These are fresh results on the corrected standalone implementation, not the
historical T-748-composed suite. Only report notices/evidence changed afterward;
implementation equality with `9b0d42fd` was checked again before publication.

- Root `cargo check` passed, including normal TUI/Server compile closure; final
  repeat passed. Logs: `target/t743-standalone-{check,final-check}.log`.
- Public assignment/source/replay/filter focused tests: **28 passed**.
- `cargo test --no-fail-fast -p ticket -p worker-runtime -p worker -p yoi-workspace-server -p server-api`
  passed all selected targets with normal default parallelism. Library counts:
  server-api **98**, ticket **99**, worker **994**, worker-runtime **417**,
  Server **946** with **1 pre-existing ignored** (176.76s); Server binary **11**,
  other integration/binary/doc targets passed. Log: `target/t743-standalone-crates.log`.
- The full Worker suite includes public Tool/WIP forwarding, source/target
  distinction, exact-retry and effective research-return/confirmation-wait/
  separate-close Prompt fixtures. These are scripted semantic tests, not a
  measurement of real model judgment. Extra `manifest --lib builtin_profile`
  **6 passed** and `worker --lib flow` **17 passed**. Initial mistyped filters
  matched zero tests and are not counted as proof.
- Fresh Web check **0 errors/0 warnings** and production build passed; existing
  chunk-size advisory remains. Web **613**, component **318** and Chromium
  Ticket/principal **7** tests passed. Logs: `target/t743-standalone-{web-check,web-build,web-tests,components,browser}.log`.
- All six TypeScript generator `--check` commands passed: ticket, repository
  OpenAPI, runtime, workdir, Worker launch and companion.
- `cargo fmt --all -- --check`, `git diff --check HEAD` and the required
  `env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts`
  passed: **558 matching lines, 0 unapproved, 0 stale exceptions**.

## Recovered visual evidence and parent review

The visual child was stopped by an interrupted turn, not a successful final
handoff. The parent inspected its committed session and recovered six completed
capture bundles under `target/t743-standalone-visual`, then personally used
`ViewImage` on actual images. No missing child verdict was invented, and no
child was restarted just to wait for a report.

Fresh bundles at source **`c6d73e8286d743e0329154d49b52d4bee53e8d60`**:

- `c6d73e82-tickets` / `tickets-dark`: **63 captures each**, covering detail,
  list/create/current closure filtering, Orchestrator, open+done, waiting,
  running/stopped Worker, repository/MR/no-MR, read-only/eligibility, closed,
  long, empty, error and unknown states at **1440/768/390**, light/dark.
- `c6d73e82-ticket-principals` / `ticket-principals-dark`: **9 each**, explicit
  agent/account/Worker selection at all three widths.
- `c6d73e82-related-light` / `related-dark`: **21 each**, explicitly scrolled
  capture points for retained result/release history, case history, returned
  handling, stopped/running Worker, action eligibility and MR separation.
  This closes the inner-scroll gap: browser full-page capture alone does not
  expose lower detail sections.

Total **186** captures. Metadata records the actual source SHA and `dirty:true`
for the then-uncommitted report notices, not hidden implementation changes.
Parent rechecked the implementation equality, source/viewport/route matrix,
per-capture errors and runner-owned fixture shutdown records. Parent actual image
review included list desktop/mobile and Orchestrator narrow/mobile, principal
light/mobile and dark/desktop sheets, waiting and returned handling, retained
result/release mobile and case history narrow, stopped Worker, MR separation,
mobile error and long list. Labels/actions remain readable; case state, handling,
actual Worker and MR sections remain distinct; no new overflow or visual fix was
needed. Original before/after visual evidence remains retained for comparison.

Both Ticket scenario bundles deliberately exit **2** and are
`completed-with-errors`: only the `error` route at the three widths records its
fixture query **HTTP503**, console/request-abort and visible error/Retry evidence.
The other four bundles exit **0**, and no other route has capture errors. The
parent saved the exact error audit in
`target/t743-standalone-visual/parent-errors-audit.txt`. Do not describe the whole
scenario as error-free or the intentional 503 as a product regression. The
synthetic member/anonymous transport is not proof of live anonymous permission;
actual cookie Origin/signed Runtime/source denial is exercised by Server tests.

All reported fixture processes had owner-scoped shutdown before the child was
stopped; this is fixture teardown, not live Yoi/retained Worker cleanup. This
publication is ready for its own fresh independent T-743 review, not an approval,
model-quality attestation, completion of T-744 live paths or target integration.
