# Merge Request native WIP projection

## Purpose and authority

When a Worker explicitly runs in `wip` mode and its Merge Request Feature has at least one enabled operation, the Worker Host projects that exact operation set as native Worldspace objects. The projection is an adapter over the existing scoped Workspace Merge Request tools and Backend routes. It does not create a second Merge Request store, provider integration path, review grant, approval rule, or completion authority.

The Host allocates `/features/merge-request` and the Feature mounts:

```text
/features/merge-request/merge-requests
/features/merge-request/merge-requests/<merge-request-id>
```

An item identity is a single canonical path segment. Nested paths and percent-encoded target substitution do not resolve. Item operations remove `merge_request_id` from their descriptor and reconstruct it only from the selected object route. Discovery does not mint authority: every call still uses the Worker's scoped `WorkspaceClient`, the enabled Feature flags, ordinary Tool permission rules, and the Backend's Workspace, repository, assignment, provider-ref, review, and integration checks.

## Current Feature inventory

The native surface is assembled from `MergeRequestFeatureConfig`, not from a broader Backend catalog. Only an ordinary Merge Request tool that would be registered for the Worker is projected and claimed from `/tools`. The current Feature has no list/query Tool, so the native adapter does not invent one: callers can resolve an existing known MR ID as a direct item route, while a successful `open` result includes the newly created item path.

Collection interface `yoi.merge-request/collection/v1`:

| Native operation | Existing Tool | Preserved authority inputs |
| --- | --- | --- |
| `open` | `OpenMergeRequest` | Ticket, repository key, source selector, target selector, summary |
| `complete_ticket` | `CompleteTicket` | Ticket, operation ID, current item revision, exact linked-MR result set, requirement approval event |

Item interface `yoi.merge-request/item/v1`:

| Native operation | Existing Tool | Preserved non-identity inputs or authority |
| --- | --- | --- |
| `read` | `ShowMergeRequest` | bounded thread `after` and `limit`; Backend resolves current source and target refs |
| `check_readiness` | `CheckMergeRequestReadiness` | Backend resolves the current source subject and derives current approval/blockers |
| `review` | `ReviewMergeRequest` | decision/body/findings plus the injected Reviewer capability, captured request event, Ticket revision, linked result snapshot, and exact candidate |
| `complete` | `CompleteMergeRequest` | operation ID, approval event, target-before/after refs, merge strategy and conflict resolution; Backend requires Orchestrator authority |

Typical role surfaces are therefore:

- Coder: collection `open`; item `read`.
- Reviewer: item `read` and `review`.
- Orchestrator: collection `complete_ticket`; item `read`, `check_readiness`, and `complete`.

`complete_ticket` is deliberately collection-scoped because its subject is a Ticket and an exact set of linked Merge Request results rather than one route-bound MR. It keeps those identities explicit. The review descriptor never contains the injected capability token, and a review item route must exactly match the Merge Request captured by that capability before the ordinary review Tool can run.

## Preserved review and integration gates

The native projection delegates to the same ordinary tools and REST endpoints. In particular, it does not replace or weaken:

- Workspace and configured-repository isolation;
- current Coder assignment and assigned-Workdir source-selector proof for opening/review requests;
- provider resolution of the exact current source candidate;
- ReviewRequested binding to the Ticket item revision and complete linked-MR/source snapshot;
- injected Reviewer capability consumption and exact MR/candidate matching;
- source movement invalidating source approval while target-only movement preserves unchanged-source review;
- unresolved request-change and revoked/stale approval rejection;
- current target observation, target-before/after proof, strategy, and conflict-resolution checks;
- Orchestrator-only Merge Request and Ticket completion;
- immutable integration/result evidence and exact linked-result-set Ticket completion.

The Worldspace adapter performs no Git or provider integration. A visible `complete` operation remains unusable without the ordinary Backend's authenticated online Orchestrator source and integration evidence.

## Bounded reads, staleness, and outcomes

`read` exposes the existing bounded thread page with optional `after` and a `1..=200` limit. Responses keep linked Tickets as references/data, so Ticket or Objective native projections are not prerequisites. Descriptors never expand repository diffs, unbounded thread history, credentials, or capability material.

A successful item read records a response fingerprint. A successful item mutation advances a local mutation generation. Opening records the returned MR and adds its native path to the result. Ticket completion invalidates every MR route named in its exact result set because releasing the assignment can change later ref observations. The common WIP Host rejects calls made with stale object/interface validators; callers must rediscover before deciding whether to call again.

Optional fields preserve ordinary JSON-input semantics: omission remains omission and explicit WIP unit/null remains JSON `null` for schema and permission matching. The reconstructed object is validated against the original Tool JSON Schema before the Tool executes.

Backend 401/403 responses map to permission denial, 404 to missing objects, and deterministic 400/409/422 authority or workflow failures to invalid arguments. Cancellation and interruption remain distinct. Transport/server failures after dispatch, malformed successful mutation responses, and identity-mismatched mutation responses are reported as unknown outcomes and must not be retried automatically.

Normal Tool mode is unchanged. In WIP mode, only the enabled Merge Request compatibility tools represented by these native objects are suppressed, preventing duplicate native and `/tools` publication.
