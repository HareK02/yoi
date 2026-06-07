<!-- event: create author: "yoi ticket" at: 2026-06-07T00:16:51Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: comment author: hare at: 2026-06-07T01:21:43Z -->

## Comment

## Dependency note: local role/session registry

Companion lifecycle should remain separate from Ticket/role Pod claim authority. Local role session and Ticket claim data belongs in the user-data workspace overlay planned by `workspace-panel-local-role-session-registry`, not in git-tracked Ticket metadata.

The Companion may eventually read/display this derived status, but it should not own the registry or gain mutation authority by default.

---
