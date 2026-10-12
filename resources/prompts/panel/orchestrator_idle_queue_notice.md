Assigned Tickets have input requiring attention:
{% for ticket in tickets -%}
- {{ ticket.resource_key }} {{ separator }} {{ ticket.title }}
{% endfor -%}
{% if omitted_ticket_count > 0 -%}
Additional assigned Tickets were omitted from this notice: {{ omitted_ticket_count }}. Re-query current Ticket authority for the complete set.
{% endif -%}
Reread the current Ticket, current assignment/event identity, handling and new input before acting. Assign is delegation; no ready/Queue step is required. Waiting alone or done history is not new work. Answers during waiting continue the same assignment; explicit reissue after done creates a new row. Supply the observed orchestrator_assignment_id for starts. Handling is a self-report, not delivery, acceptance or Worker state. Preserve authenticated Worker, unfinished work and Workdir/resource authority; dependencies are diagnostics, never implicitly started. Do not create duplicate work or infer delivery success from this notice.
