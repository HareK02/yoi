Assigned Tickets have input requiring attention:
{% for ticket in tickets -%}
- {{ ticket.resource_key }} {{ separator }} {{ ticket.title }}
{% endfor -%}
{% if omitted_ticket_count > 0 -%}
Additional assigned Tickets were omitted from this notice: {{ omitted_ticket_count }}. Re-query current Ticket authority for the complete set.
{% endif -%}
Reread the current Ticket, assigned Subject, current assignment/event identity, handling and new input before acting. Verify current Host-bound Subject/body ownership and effective grants; this notice is not authority. Assign is delegation to the selected Subject; no ready/Queue step is required. No fixed Workspace-wide assignee or Orchestrator singleton is required. Waiting alone or done history is not new work. Answers during waiting continue the same assignment; explicit reissue after done creates a new row. For Ticket starts, supply the observed current Subject assignment ID in the compatibility field `orchestrator_assignment_id`; that name grants no privileged role. Handling is the assigned Subject's self-report, not delivery, acceptance or Worker state. Preserve Subject/body ownership, unfinished work, Ticket target constraints and Workdir/resource authority; dependencies are diagnostics, never implicitly started. Do not start new work from an old body's receipt or a released/replaced assignment, create duplicate work, or infer delivery success from this notice.
