Spawn a parent-owned Internal SubWorker session to split context for a delegated task. The parent Worker's write scope is reduced by the scope passed here; the Internal SubWorker starts running `task` immediately without creating a Runtime Worker record, OS process, PID, or Unix socket. It remains available for follow-up turns until explicitly stopped or its parent exits.

Optional `cwd`: when provided, the spawned SubWorker's tool default working directory only. It must be an absolute existing directory covered by the child's delegated readable scope, and it does not change workspace/Profile/memory/Ticket roots or grant authority. `name` must be unique among this Worker's direct children.

Profile selection: `profile` may be omitted or set to `default` to use the effective child default profile, set to `inherit` to derive reusable child configuration from this Worker, or set to one of the registry selectors below. Raw/path profile selectors are not accepted by SubWorkerSpawn. `scope` is always the only delegated filesystem capability; profile scope is replaced by the explicit SubWorkerSpawn scope.

Default profile: {{ default_profile }}
Special selector: inherit — derive reusable model/worker/tool policy from the spawner while replacing worker.name and scope.
Available registry profiles:
{{ available_profiles }}{% if profile_diagnostic %}

Profile discovery diagnostic: {{ profile_diagnostic }}{% endif %}