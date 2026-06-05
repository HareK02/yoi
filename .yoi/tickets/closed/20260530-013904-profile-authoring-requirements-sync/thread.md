<!-- event: create author: tickets.sh at: 2026-05-30T01:39:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-30T02:03:58Z -->

## Decision

Updated the requirements to reflect the current interpretation: Profile is a reusable manifest-like recipe template, not necessarily a separate semantic-only projection. The boundary is that runtime-bound and authority-bearing fields (`pod.name`, concrete `scope.allow`, resolved paths, secret material, runtime state) stay out of Profile, while reusable recipe fields such as model, worker/reasoning, compaction, memory, web, tool policy, and scope intent may remain close to Manifest shape.

Lua-specific notes now record controlled `require` and a public `profile` constructor as likely core primitives, while keeping the exact language/API/return contract open.


---

<!-- event: close author: hare at: 2026-06-01T07:02:44Z status: closed -->

## Closed

Closed as completed. The requirements sync served its purpose: it captured the Profile boundary, rejected proceeding with semantic-nix-profiles as the assumed direction, and led to the follow-up Lua implementation ticket. The concrete authoring direction was implemented and closed in lua-profile-authoring; remaining refinements should be tracked as separate follow-up tickets.


---
