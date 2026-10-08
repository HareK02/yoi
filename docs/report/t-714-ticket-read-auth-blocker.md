# T-714: Ticket authority read rejected after implementation validation

## Observed boundary

On the continuation turn, `ShowTicket({ id: "T-714", event_limit: 5 })`
returned `Workspace Ticket API request failed with HTTP status 401`.
The same assigned Coder session had successfully read the Ticket and recorded
one shared-seam coordination decision earlier. This report does not diagnose
whether the rejection is authentication expiry, capability invalidation, or
another Backend condition.

## Consequence

The local checkout implementation and validation were complete. Git retained the
assigned `work/t-714-checkout-discovery` branch and all changes; coherent local
commits were authorized by the user. Publication/MR creation and review were
paused because a fresh authoritative Ticket/MR read could not be obtained.
No alternate actor, CLI, Backend storage access or replacement control-plane
path was attempted. No target integration or live dogfood update occurred.

## Improvement / recovery

Provide an actionable typed error distinction between authentication failure and
capability rejection, and a host-owned way to restore valid transport authority
without changing actor identity or bypassing the rejected capability. After that,
reread the Ticket and any linked MR, normally publish only its original source
selector, verify exact provider HEAD, and request an independently bound review.
The local implementation/validation evidence is in commits and
`docs/development/wip-integration-validation.md`; no approval is claimed here.
