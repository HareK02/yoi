Orchestrator progress context (read-only weak notification; no auto-run).
Reason: workspace Panel refreshed bounded orchestration progress for Companion explanation.
Roles: Companion {{ companion.pod_name }} is {{ companion.status }}; Orchestrator {{ orchestrator.pod_name }} is {{ orchestrator.status }}.

{% if tickets %}Tickets (first {{ tickets | length }} visible, bounded):
{% for ticket in tickets %}- {{ ticket.id }} [{{ ticket.state }}] {{ ticket.title }} (ref: {{ ticket.reference }})
{% endfor %}{% if omitted_ticket_count > 0 %}- … {{ omitted_ticket_count }} more ticket(s) omitted from this bounded notice.
{% endif %}{% else %}Tickets: none visible in the current Panel snapshot.
{% endif %}{% if role_pods %}
Role pod status snapshot:
{% for role_pod in role_pods %}- {{ role_pod.name }}: {{ role_pod.status }}
{% endfor %}{% endif %}
