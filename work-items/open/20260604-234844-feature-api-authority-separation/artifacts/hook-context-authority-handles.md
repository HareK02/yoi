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
