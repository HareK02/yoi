
---
## Resident memory refresh

This restore-boundary record supersedes every resident memory summary embedded earlier in the system prompt or session history. Treat only the current state below as resident context; it is background context, not a user request.

{% if surface_ready %}
### Current resident context

{% if summary %}
{{ summary }}
{% else %}
The current resident context is intentionally empty.
{% endif %}
{% else %}
No current ready resident memory surface is available. Ignore earlier resident memory summaries; confirmed Memory remains available through explicit recall tools.
{% endif %}
