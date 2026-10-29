Assigned Tickets have unreceived input:
{% for ticket in tickets -%}
- {{ ticket.resource_key }} {{ separator }} {{ ticket.title }}
{% if ticket.assignment_id -%}
  Delegation: {{ ticket.assignment_id }}; input: {{ ticket.input_event_id }}.
{% endif -%}
{% if ticket.waiting_reason -%}
  Waiting reason: {{ ticket.waiting_reason }}
{% endif -%}
{% if ticket.next_action -%}
  Next action: {{ ticket.next_action }}
{% endif -%}
{% endfor -%}
{% if omitted_ticket_count > 0 -%}
Additional assigned Tickets were omitted from this notice: {{ omitted_ticket_count }}. Re-query current Ticket authority for the complete set.
{% endif -%}
Reread the current Ticket state before acting, including the assigned Subject, current assignment/event identity and new input. Verify that the Host-bound Subject and current body still match the assignment and have the required effective grants; the notice does not establish ownership or authority. Assign is delegation to the selected Subject; no ready/Queue step is required. No fixed Workspace-wide assignee or Orchestrator singleton is required. Handling status is the assigned Subject's self-report, not delivery or Worker state. Waiting alone and done history are not new work. Old-body receipts and released/replaced assignments do not authorize new work, restore or duplicate execution. Dependencies are diagnostics for partial work, not a blanket start gate; never implicitly start dependencies. Preserve Subject/body ownership, unfinished work, Ticket target constraints and Workdir/resource authority.
