Workspace Dashboard observed that this Orchestrator Worker is idle while queued Ticket work is present.

This is bounded attention only, not scheduler authority. Do not drain the queue automatically. Verify the Ticket is still `queued`, then use the guarded `SpawnTicketCoder` operation without a separate state transition; that operation records `queued -> inprogress` only after Worker creation, initial input, assignment, and Workdir finalization are durably accepted.

Workspace: {{ workspace }}

Actionable queued Tickets:
{% for ticket in actionable_tickets -%}
- {{ ticket.id }} — {{ ticket.title }} [{{ ticket.classification }}]
{% endfor -%}

{% if waiting_tickets | length > 0 -%}
Queued Tickets retained in the session work set but currently waiting:
{% for ticket in waiting_tickets -%}
- {{ ticket.id }} — {{ ticket.title }} [{{ ticket.classification }}]: {{ ticket.waiting_reason }}
{% endfor -%}
{% endif -%}
{% if omitted_ticket_count > 0 -%}
Additional queued Tickets omitted from this bounded notice: {{ omitted_ticket_count }}
{% endif -%}

Preserve the existing human gate, dependency/conflict/capacity/dirty-workspace checks, and duplicate-start checks using actual Ticket state, role/session claims, visible Workers, and worktrees.
