<!-- event: create author: tickets.sh at: 2026-06-04T23:48:44Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-04T23:50:15Z -->

## Decision

# Decision: separate internal feature modules from external-plugin authority

Internal modules extracted from Pod implementation files should not be treated as if they require the external-plugin permission model.

For an internal built-in module such as Task tools:

- the feature registry is an API/registration boundary;
- descriptor-declared contributions are reconciled at install time;
- normal ToolRegistry and PreToolCall permission behavior remains authoritative;
- host state such as `TaskStore` can be passed by the Pod host constructor;
- requested host authorities should normally be empty.

The external-plugin authority model remains necessary for sandbox/object-capability grants when plugin code receives dangerous host APIs such as filesystem, network, secrets, model-visible durable notification/history append, Pod-management façade, persistent state, or authority-bearing service access.

This split should be implemented separately from the Task tools extraction. The Task tools extraction should validate the contribution-only built-in module path without solving external plugin approval.


---

<!-- event: decision author: hare at: 2026-06-05T00:49:53Z -->

## Decision

# Decision: authority handles live in Hook contexts, not Hook return effects

The internal-module and external-plugin authority split should treat host-authority APIs as handles supplied by the host, including inside Hooks.

Implications:

- Hook return values remain per-hook-point flow-control actions.
- Side effects such as durable model-visible SystemItem append are performed through typed host handles on event-specific Hook contexts.
- Built-in internal modules may receive handles according to host policy without user-facing external-plugin approval.
- Future external plugins receive only the handles allowed by their approved host authorities.
- The main API should not be “return an effect and let the host reject it at runtime.” Rejection remains defense-in-depth for malformed calls, missing handles, bounds, and policy violations.
- Do not model every authority combination as a distinct Hook context type. Use event-specific context types with authority-specific handles whose constructors are host-owned.

This preserves the clean distinction: contribution declarations are descriptor-locked; dangerous host APIs are represented by host-created handles; normal tool permission remains the per-call execution gate.


---

<!-- event: plan author: hare at: 2026-06-05T04:54:33Z -->

## Plan

Preflight result: `implementation-ready`.

`ticket-built-in-feature-tools` should not be implemented until this boundary is clarified, because Ticket tools need to be internal built-in feature contributions while Ticket backend operations remain typed host authority, not arbitrary filesystem scope or external plugin package approval.

Implementation intent:

- Clarify `pod::feature` naming around host authority grants.
- Keep contribution declarations and descriptor reconciliation separate from host authority requests/grants.
- Preserve built-in Task feature behavior as the current contribution-only example.
- Avoid external plugin loading, real approval protocol, Ticket tool implementation, Hook behavior changes, or broader crate moves.

Detailed delegation intent is in `artifacts/delegation-intent.md`.

Note: main workspace currently has an unrelated dirty `README.md` change. This feature work must not touch or commit that file.


---
