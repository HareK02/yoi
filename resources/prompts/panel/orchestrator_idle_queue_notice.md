Queued Tickets require attention:
{% for ticket in tickets -%}
- {{ ticket.resource_key }} {{ separator }} {{ ticket.title }}
{% endfor -%}
{% if omitted_ticket_count > 0 -%}
Additional queued Tickets were omitted from this notice: {{ omitted_ticket_count }}. Re-query current Ticket authority for the complete set.
{% endif -%}
Reread the current Ticket state before acting. Preserve the human queue gate and current assignment, dependency, Worker, and Workdir authority; do not create duplicate work.
