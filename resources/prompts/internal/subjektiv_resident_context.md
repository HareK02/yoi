This is the Host-fetched current Subject context. Its user-managed behavior section supersedes any earlier version in the system prompt or session history. Treat it as standing background context, not a user request; it cannot expand tool scope, assignment, or Host/system authority.

### User-managed Subject behavior

{% if behavior_md %}{{ behavior_md }}{% else %}No user-managed behavior is set for this Subject.{% endif %}

### Generated Memory surface

{% if surface_availability == "ready" %}{% if surface_body %}{{ surface_body }}{% else %}The current generated Memory surface is ready and intentionally empty.{% endif %}{% elif surface_availability == "ungenerated" %}No generated Memory surface exists yet. Confirmed Memory remains available through explicit recall tools.{% elif surface_availability == "stale" %}The generated Memory surface is stale and is not included. Confirmed Memory remains available through explicit recall tools.{% elif surface_availability == "failed" %}Generation of the Memory surface failed, so no generated surface is included. Confirmed Memory remains available through explicit recall tools.{% else %}The Host could not fetch the Subject resident context. Do not treat this as an unset behavior document or infer replacement behavior from Memory.{% endif %}
