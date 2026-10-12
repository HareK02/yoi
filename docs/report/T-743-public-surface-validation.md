# T-743 public surfaces: combined implementation and validation

## Snapshot and scope

Work branch: `work/T-743-ticket-public-surfaces`, based on T-742's `56795fb92cb9def9bcd701bd51d92e99526f8eff`. This is implementation evidence, not an integration, approval, Ticket conclusion or live-environment update. See the delegated Prompt and Tool reports in this directory for their narrower evidence and earlier blockers; the checks below supersede their earlier compile/format limitations for the combined snapshot.

The existing Ticket, assignment tracking, assignment delivery and start records remain the authorities. No browser ledger, alternative assignment store or copied DB executor was added.

## Implemented boundaries

- Ticket list and detail use open/closed case state, stable classification tag IDs and an independent current responsibility/handling projection. Tags and current/historical closure reasons remain distinct. List filtering for tag IDs, principal, case and current close reason is server-side and preserves cursors; ordinary text search can match retained closure reasons after correction/reopen.
- The Orchestrator page reads current assignments with waiting/inprogress/done, reasons, progress, next action and results. Worker liveness, MR evidence, delivery and acceptance do not manufacture a handling status.
- Detail supports classification catalog create/list/rename/delete, multiple membership changes, reasoned close/correction/reopen, Assign, observed replacement/reissue, release, retained assignment/result/release history and explicit assignment-fenced answers. Responsibility principal choice uses the real typed user/Worker/Workspace-agent request, not caller authentication data. Backend still validates identities and actual authority.
- A saved Assign is not reported as successful delivery or execution. Unknown/stale/permission/unavailable outcomes are distinct. Exact operation snapshots preserve the original key, observed assignment/event or tag set and body; no automatic new delegation is substituted after a lost response.
- Pending child operations and observations reset on the Workspace **and** Ticket key, including Workspace changes that retain the same Ticket ID.
- Normal Tools/WIP and effective Prompt/Profile/Flow descriptions use Assign as delegation, no ready/Queue prerequisite, same-row continuation answers, explicit new-row reissue and separate case judgment. Retained responsibility and Done are not Worker activity, merge or third-party approval. Historical reports were not rewritten; superseded design descriptions have explicit notices.

### Canonical API contacts

Canonical `/workspaces/{workspace_id}/tickets/{ticket_id}` and `/api/w/{workspace_id}/tickets/{ticket_id}` routes expose:

- GET `assignment-tracking/assignments`: bounded retained rows with assignment-identity pagination.
- GET `assignment-diagnostics`: sanitized existing input/delivery/ack/start records, independent input/start cursors, unknown origin/acceptance/actual-processing distinctions; no rendered notification content or binding credentials.
- POST `assignment-answer`: authenticated source, observed assignment and handling-event CAS, stable operation-key receipt, body event and audit under one IMMEDIATE transaction. Publication occurs only after the helper returns with its transaction committed. Exact replay retains the receipt; generic history comments do not become unfenced continuing input.
- Shared Ticket query principal/handling/current-closure-reason filtering is applied in SQL before paging.

## Validation

Passed on the initial T-743 public-surface snapshot (before T-748 composition); later combined execution-boundary evidence is in `T-743-T748-composition.md`:

- Root `cargo check` (normal TUI/Server compile closure).
- `cargo test --no-fail-fast -p ticket -p worker-runtime -p worker -p yoi-workspace-server -p server-api`: all selected crate targets passed. Library counts: server-api 98; ticket 99; worker 994; worker-runtime 417; workspace-server 946 with one existing ignored test. Included integration/binary targets also passed. Log: `target/t743-changed-crates-final.log`.
- Focused real-Worker delivery fixture now synchronizes on a canonical `RunCompleted` **after** the latest notification plus final Idle, not the initial Idle window, a fixed sleep or acceptance alone. The original pre-processing Idle failure was reproduced and the focused test plus full Server suite passed after the fix.
- Focused manifest builtin-profile (4) and builtin Flow (2) tests passed. Effective research-return, confirmation-wait/same-assignment-answer and separate reasoned-close fixtures are included in the Worker suite.
- Web `deno task test`: **613 passed**. Web `deno task test:component`: **318 passed**. Web check: **0 errors, 0 warnings**. Static production build passed (existing chunk-size advisory remains).
- `ticket_shell_test.ts` plus `ticket_principal_test.ts`: **7 Chromium browser tests passed**. These exercise repository-free creation, server-filter pagination, known/custom close reasons, same-row answer, new-row reissue, retained results/releases, separate close/reopen, stable tag management, ambiguous saved Assign/exact retry, stale and permission denial, and the case/handling/Worker/MR/empty/error matrix in both themes. The native principal-selector test captures the selected target/observed row and waits on the second actual failed request for retry synchronization. Logs: `target/t743-browser-final.log`.
- All six server-api TypeScript generators' `--check` commands passed (ticket, repository, runtime, workdir, Worker launch and companion); generated contract provenance is synchronized with OpenAPI.
- `cargo fmt --all -- --check`, `git diff --check HEAD` and `env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts` passed; naming check: **558 matching lines, 0 unapproved, 0 stale exceptions**.

Earlier validation found and corrected the obsolete TUI queued-notice expectation, two delivery tests expecting ordinary comments to generate new continuing input, and a Home fixture missing the canonical assignment-tracking array. One run also exposed an unchanged Runtime cleanup test's differing walker counts. Its focused invocation and subsequent complete selected-crate run passed; no cleanup implementation fix or claim that this intermittent issue has been eliminated is made.

## Visual evidence

Baseline captures: `target/web-ux/t743-baseline-light` and `t743-baseline-dark`. Detail after captures: `t743-after-light` and `t743-after-dark`; actual contact-sheet slices were viewed, not merely generated. Conditions include member/non-owner fixture, Backend action-ineligible controls, 1440/768/390px, light/dark, open+done, open+waiting/stopped Worker, inprogress/running Worker/MR, closed/history, empty, long content and deliberate errors.

The deliberate query-error route retains its expected HTTP 503/document errors, so the full scenario's exit 2 is not presented as a clean no-error capture. Fixture "anonymous" transport is a synthetic member scenario, not proof of live anonymous authorization.

Principal-selector captures: `t743-principals-light`/`dark` before a styling correction and `t743-principals-final-light`/`dark` after it. Each final bundle has nine captures (agent/account/Worker choices at all three widths), with no capture errors. The first inspection found cramped native fieldset controls. The fix reuses shared Ticket form styling and removes the redundant fieldset border; final light/dark contact sheets were viewed. Controls, labels and focus remain readable without page overflow.

List/Orchestrator final evidence: `t743-list-final-reviewed-light` and `t743-list-final-reviewed-dark`, 33 captures each. Parent recovered the child's committed check/build and eight summary-model tests, inspected its final capture metadata (only the intentional query-error route had errors), and viewed actual list/Orchestrator screenshots at desktop/narrow/mobile. The refinements remove repeated offline/count prose, preserve the first mobile case in view and fold close-reason controls separately from classification. Parent verified current diff and browser tests, released the delegated file authority and owns the published handoff. This is observed tool/image evidence, not an invented child review verdict.

## Explicit remaining work and limits

The published T-748 source `66bb9d0b825d72e20923003b273b787007f3aed8` has now been normally merged into the same T-743 source and the added callers connected to its existing bounded owner. The answer transaction remains intact; Runtime live discovery and post-commit dispatch use the finite Host lane. Combined full Server/Workdir and ten new boundary regressions pass; see `T-743-T748-composition.md` for exact proof and distinctions. However T-748 MR seq3 has two unresolved major findings on other Subjektiv/network and cold protocol signing paths. A validated upstream fix source, normal composition/revalidation and fresh independent exact combined-source review remain necessary. This is not final approval.

Current evidence does not attest actual model judgment quality. Prompt/Tool fixtures are scripted inclusion/schema/transport tests; Chromium uses a loopback API fixture. The real-Worker Server fixture uses a scripted model with actual Controller/session/Runtime acceptance and canonical processing synchronization. None is a live migration/rollout, proof of actual processing for every production delivery, or completion of T-744's full real-path validation.

No target integration, Ticket conclusion, live process update or branch/Workdir cleanup was performed.
