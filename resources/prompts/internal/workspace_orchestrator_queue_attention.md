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
Reread the current Ticket state before acting, including current assignment tracking and new input. Assign is delegation; no ready/Queue step is required. Handling status is your self-report, not delivery or Worker state. Waiting alone and done history are not new work. Dependencies are diagnostics for partial work, not a blanket start gate; never implicitly start dependencies. Preserve current assignment identity, authenticated Worker, unfinished work, and Workdir/resource authority; do not create duplicate work.
