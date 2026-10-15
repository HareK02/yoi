# Tickets browse: component performance comparison

## Change

The Web Tickets list uses `POST /api/w/{workspace_id}/tickets/browse`, a deliberately narrow display contract. The existing `/tickets/query` and Ticket detail paths retain authoritative MR evidence observation; the browse result contains no evidence/approval fields and is not an integration-readiness attestation.

A nonempty browse page uses one SQLite read transaction with four SELECT statements: filtered page selection, active tags, current closure judgments, and current Orchestrator assignment tracking. No full Ticket/event reconstruction, dependency graph, provider Git observation, or per-row enrichment query is performed. All filters precede LIMIT; cursors bind Workspace, canonical filters, sort and the last row, including NULL timestamps and ID ties.

The Web loader waits only for the list (and tag-name resolution when requested). Orchestrator status loads independently after mount; repositories load on opening Add Ticket. Late responses are fenced against route/Workspace changes.

## Backend measurement

Run the retained benchmark:

```sh
cargo test -p yoi-workspace-server ticket_browse_component_performance --lib -- --ignored --nocapture
```

Optional fixture sizes: `BROWSE_BENCH_TICKETS` and `BROWSE_BENCH_EVENTS`.

- Real `SqliteWorkspaceAuthority` methods and file-backed disposable SQLite; no production DB, HTTP server, Runtime, provider network, or live Git.
- 1,000 generated open Tickets, each with 100 discussion events plus the initial post, roughly 1 KiB text per event, event attributes/references and one relation. One additional seed Ticket is used for valid references.
- Compare the former browser path (`query_tickets`, preserved for authoritative queries) with `query_ticket_browse` on the same fixture.
- Fixture construction excluded; both paths warmed, then three old/new pairs measured through JSON serialization. Assert identical displayed items/order and has-more before accepting results.
- Debug test build on a shared development host; these are component measurements, not production response-time estimates. Run-to-run variation is substantial; no flaky time threshold is asserted.

Adopted Unicode-aware implementation with borrowed scalar-function inputs, median milliseconds:

| Case | Former browser path | Browse path | Response bytes, old → new |
|---|---:|---:|---:|
| Initial 30-row list | 23,265.888 | 0.769 | 29,744 → 5,647 |
| Dense discussion-text match | 23,121.110 | 16.130 | 52,163 → 5,646 |
| Sparse title match (`Ticket 00099`, 10 rows) | 7,314.318 | 496.367 | 10,076 → 1,968 |
| No match (`absent-needle`) | 299.065 | 409.180 | 184 → 186 |

The ordinary list removes the unnecessary work rather than caching evidence. Dense matching stops after enough rows; sparse/no-match searches still scan text across candidates. **No-match search regresses by about 110 ms in this fixture.** The browse path fixes the old SQL/Rust Unicode case-fold mismatch using the same Rust lowercase operation for the needle and stored text, at callback cost. An ASCII SQL length-check fast path was measured and removed because it increased scan cost. A persistent search index is not introduced in this change; large sparse/no-match searches remain a limitation, not a claimed improvement.

## Web component measurement

```sh
cd web/workspace
deno task test:component 'src/routes/w/[workspaceId]/tickets/ticket-list.browser.spec.ts'
```

The real Svelte component and new loader run against controlled promises, with 20 ms list latency and 100 ms auxiliary-request latency. The comparison restores the former `Promise.all` auxiliary gate for the baseline; it does **not** simulate the old backend SQL. One focused run measured 103.17 ms with the gate versus 23.48 ms through row rendering without it. There are no live requests or hard timing thresholds. Separate assertions verify visible rows while status is unresolved and no repository fetch before Add Ticket.

## Regression evidence

- Workspace-server library: 1,088 passed, 2 ignored (including the separately run timing benchmark).
- Backend browse tests: keyset pagination, filter-before-limit, tag/current-assignment/closure parity, Workspace isolation, literal `%`/`_`, Unicode title/body/thread/reason search and embedded NUL.
- Actual HTTP route: response shape, unauthenticated rejection, invalid filters and foreign Workspace rejection.
- Counting source observer: browse makes zero calls; authoritative QueryTicket and show still observe open MR sources.
- Web unit tests: 644 passed. Svelte components: 401 passed. Svelte check: no errors or warnings.

No production migration or restart is needed for the schema itself (no schema change); serving the new endpoint and Web code still requires the normal software update.
