<!-- event: create author: yoi ticket at: 2026-06-06T05:29:03Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T05:29:48Z -->

## Plan

Created this follow-up because the first panel slices now provide Ticket/action rows, Orchestrator lifecycle, Intake launch, and Intake handoff, but Ticket row actions are still mostly display affordances.

Before layout/display tuning, the panel should support a minimal safe action dispatch path for the human decision points it already displays, especially Go/Defer. The implementation should re-check Ticket authority before mutation, use Rust Ticket APIs, and notify Orchestrator for Go/routing actions when feasible.


---
