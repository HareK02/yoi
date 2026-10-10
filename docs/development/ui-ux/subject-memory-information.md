# Subject / Memory information audit (T-692)

This document records T-692's UI decisions and validation at the commits cited below. Its counter labels and fixture values describe that historical implementation, not the current API. Current Memory uses opaque `change_id` references, `memory_fingerprint` and `built_from_memory_fingerprint`; see [the current store contract](../../design/subjektiv-store.md). Historical observations and test results below are retained unchanged.

## Decision and scope

The previous Subjects index exposed `Store revision` alongside identity, state and
update time; Subject detail led with technical metadata and raw surface states.
These numbers require implementation knowledge before they help a user choose a
Subject or understand what is available. The revised hierarchy starts with
identity, connection, committed records and generated-context availability.

This is a presentation change, not a change to storage, freshness comparisons,
permissions, generation, or Worker history. T-688's ordinary Worker launch and
T-689's independent Subject creation remain the actual operation paths. T-693
owns editable user-managed behavior: that content must remain distinct from this
page's context generated from committed Memories. A successful read or a current
surface does not attest what is already in any running Worker's history.

## Field classification and before/after priorities

| Information | Classification and reason | Presentation decision |
| --- | --- | --- |
| Subject role and ID | Normal identification. Similar or identical roles need a stable identifier for selection and comparison. | Keep role as heading and the complete wrapping ID visible in index and detail. |
| Subject active/retired state | Normal operation decision: whether a new Worker connection can use the Subject. | Replace raw `active` with `Available`; keep `Retired`. Detail explains connection eligibility, not an assertion that a Worker is running. |
| Current Worker | Normal connection decision, derived from the existing Backend singleton projection. | Promote `Connected` / `Not connected` and display name. Do not infer ownership from store revision or claim all Workers received current Memory. Worker start remains the ordinary launch form. |
| Committed-Memory presence | Normal reading decision, independent from surface generation or candidates. | Show `Available` / `None` only on the successful first bounded page. On later pages say `N on this page`; errors say `Unavailable`. Never infer counts from revision or scan all pages. |
| List counts | Normal navigation context, not a store-wide statistic. | Label `N shown` and `more available`, preserving existing bounded cursor pagination. |
| Resident availability | Normal context decision. A missing surface is not missing committed Memory. | `Ready to use`, `Ready, no context`, `Not generated`, `Needs refresh`, `Generation failed`, and request `Unavailable` are distinct. These describe the current generated store artifact, not content truth or Worker-history propagation. |
| Resident body and generated time | Normal content/read freshness context. | Keep the shared Markdown renderer, generated date and exact snapshot reference count. Ready-empty explains that a valid snapshot has no body; stale/failed do not display a partial or old snapshot. |
| Subject store revision | Diagnostic only: internal committed-Memory change number, not record count, schema version or quality score. | Remove it from the index's normal columns. Retain the exact value under detail's `Technical details`, with its meaning explained. Backend comparisons are untouched. |
| Subject update time | Secondary metadata; not a context-injection or generation completion timestamp. | Label `Last changed`; on detail put it with Subject diagnostics rather than competing with the availability summary. |
| Snapshot ID and built-from revision | Diagnostic correlation, distinct from per-Memory revisions. | Retain under `Surface sources and diagnostics`, labelled `Generated from Subject revision` and explained. |
| Surface Memory references | Detailed audit/reachability. | Preserve reference IDs, immutable revision values and Memory links in the disclosure. |
| Memory claim, kind, lifecycle state, excerpt and body | Normal content decision. Active, resolved and retracted are not interchangeable. | Retain list scanning and body reading. Keep the Markdown renderer and current/historical distinction. |
| Individual Memory revision and history | Detailed content comparison, also useful for identifying a selected version. | Explicit `Memory revision` labels, separate from Subject counters. Preserve immutable history, selected revision links and revision pagination. |
| Candidate provenance, evidence, origin, source refs and derivations | Detailed audit, required to understand why a record exists. | Retain IDs, ranges, summaries, excerpts, origins and derived-revision links. Counts say `shown` except where the API provides exact per-candidate totals. Preserve body and evidence continuation links. |
| Raw field names / duplicated general revision terminology | Unnecessary in the normal hierarchy; multiple unrelated counters are misleading. | Replace bare revision labels and raw surface enums with purpose-specific labels. Do not delete API fields, stored information, diagnostics or their detailed access. |

No diagnostic datum was removed from Subject detail, surface detail or Memory
provenance/history. The index no longer shows a store revision, but the exact
value remains one click away in Subject detail. No invented regeneration,
editing or recovery action was added; `Needs refresh` does not mean an update job
is running. Creation still creates only a Subject, and selection still uses the
existing ordinary launch contract.

## Candidate-only verification

Candidates are not committed records and are not an input to this page. The
Subject loader performs just three bounded reads: Subject, resident surface and
`/memories?limit=100[&cursor=...]`. It does not read or count staging records.

The existing real-store regression
`subjektiv::tests::candidate_decisions_reject_conflicts_and_roll_back_partial_writes`
stages a candidate and attempts an invalid apply. It asserts the candidate has no
resolution and the committed Memory list remains empty. The component regression
`candidate-only Subjects show no committed Memories, not an inferred candidate count`
uses that read projection (store revision 0, ungenerated surface, empty successful
committed list) and checks `None`, `No committed Memories yet.`, `Not connected`
and `Not generated`, with no resident body. Thus a staged candidate does not turn
into a committed-Memory count or a claim that context has been generated.

The store test proves the persistence boundary; the component test proves the
rendering of its public projection. These are separate boundary tests, **not** a
live browser-to-Backend candidate-creation E2E. The existing `ungenerated` capture
represents the same visible projection, but does not itself stage a candidate.

## Visual review and validation evidence

The restored implementation is in `fbb09e65` and `4cf6c130`; this report/test
supplement does not change rendered markup, styling, copy or data contracts.

- Scenarios: `tools/web-ux/scenarios/memory-subjects-{light,dark}.json`.
- Before: `target/web-ux/t-692-before-{light,dark}` at `cc979874`.
- After: `target/web-ux/t-692-final2-{light,dark}` at clean `4cf6c130`.
- Comparisons: `target/web-ux/t-692-final2-{light,dark}-comparison`.
- Browser interactions: `target/web-ux/t-692-browser-interactions`.
- Persona: synthetic Workspace fixture; permitted creation and intentional
  permission-rejection responses. No real account or private store data used.
- Viewports: 1440, 768, 390 and 320px; light and dark themes.
- States: connected/unconnected, active/retired, successful empty lists, ready
  long Markdown, ready-empty, ungenerated, stale, failed, unavailable HTTP503,
  create form and create permission error; expanded technical disclosures.
- The takeover implementer personally inspected individual before/after images
  for index at 768px, representative detail at desktop/mobile and Memory detail,
  then ready-empty mobile, ungenerated narrow, stale desktop, failed mobile,
  endpoint error at 320px, mobile surface sources, compact Subject diagnostics
  and mobile permission error. Oversized contact sheets were rejected by the
  image provider; individual viewport PNGs were inspected instead.
- Findings/result: primary state groups replace counters without losing IDs or
  document access; long IDs wrap, status is textual rather than color-only,
  mobile groups stack, diagnostics remain readable, and error state regions stay
  separate. No new visual defect identified. **Pass for the audited scope.**
- Final bundles contain 64 captures each. `completed-with-errors` is expected:
  intentional creation HTTP403, endpoint HTTP503/associated aborted requests,
  and the deliberately failed surface's alert are retained as evidence. Healthy
  capture states have empty error arrays; this is not a blanket claim that all
  captures were error-free. Review-context diagnostics are empty.
- The focused production-shell browser test proves main-owned vertical scrolling
  with wheel/keyboard/touch, 320px/mobile shell behavior, no page-wide overflow,
  local table/code scroll, Markdown safety, creation/launch separation, bounded
  pagination and Memory/source/revision reachability.

Independent review at source `4cf6c130` ran root `cargo check`,
`cargo fmt --all -- --check`, diff hygiene, Web check (0 errors/warnings), 12
Memory API tests, 452 full Web unit tests, 134 component tests (8 Memory tests),
production build, the focused production-shell browser test (1), and Web-UX unit
tests (16), all passing.

The takeover implementer then validated this report/test supplement with:

- `cargo test -p yoi-workspace-server --lib candidate_decisions_reject_conflicts_and_roll_back_partial_writes`: 1 passed.
- Root `cargo check`, `cargo fmt --all -- --check`, and `git diff --check HEAD`: passed.
- Targeted Memory component tests: 9 passed, including the added candidate-only projection test.
- Web `deno task check`: 0 errors/warnings; `deno task test`: 452 passed;
  `deno task test:component`: 135 passed.
- Scoped `deno fmt --check` on the changed TypeScript test: passed.
- JSON audit of both final2 bundles: 64 captures each, exact clean source
  `4cf6c130`, and no errors outside the deliberate permission/error/failed routes.

The supplement changes only a document and a test, so it does not require new
rendered-UI captures. Rereview records and the final approved Ticket handoff link
this report to the new exact source.

Known non-blocking validation diagnostics: component runs emitted incidental
localhost:3000 connection-refused messages while passing; the production build
warned about a large chunk. A broader Web-UX typecheck fails in unchanged
`tools/web-ux/src/processes.ts:44` on a `TextDecoderStream` generic incompatibility;
this report does not claim that broad check passed.

No live-Backend browser interactions, real Worker model-request propagation,
regeneration, or server/runtime process E2E were performed. No Rust production
crate changed in T-692; the supplemental store test exercises an existing
contract without changing that contract. Backend counters, comparison logic,
storage schema and feature behavior remain intact.
