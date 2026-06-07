<!-- event: create author: "yoi ticket" at: 2026-06-07T00:16:51Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-07T00:18:58Z -->

## Decision

Decision from user discussion:

The panel should not provide direct messaging to arbitrary selected Pods. The existing `Companion` composer target is currently a misleading label for selected-Pod direct send and should be replaced by a real workspace Companion Pod.

Target model:
- default panel composer talks to a workspace-named Companion Pod;
- Companion is a foreground management chat for the human;
- Ticket Intake remains a separate target for new requests;
- selected Pod direct send is removed;
- Pod attach/open remains available for inspection;
- Companion should be status-aware but not directly write/mutate project state;
- Companion prompt/tool policy should focus on situational awareness and human support, with direct writes/Ticket mutations/implementation spawning prohibited by default.

Split child tickets were created for direct-send removal, Companion lifecycle, and Companion prompt/tool policy.


---
