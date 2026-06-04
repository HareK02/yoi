# Decision: narrow plugin permissions to host authorities

Plugin/Feature permission should mean user-approved host authority, not every contribution the feature declares.

Purpose:

- Explain to the user what dangerous host authority a plugin receives.
- Ensure sandboxed/external plugin code is not given APIs or handles for unapproved actions.
- Keep registry contribution integrity separate from authority grants.

Revised model:

- Contributions are descriptor/digest-locked declarations:
  - Tools
  - Hooks
  - BackgroundTasks
  - Service providers
- Host authorities are user-approved sandbox/object-capability grants:
  - filesystem access
  - network access
  - secret refs
  - model-visible durable notification/history append
  - Pod management façade access where exposed
  - store/state access such as Memory/WorkItem or persistent plugin state where applicable

Tool/Hook/BackgroundTask/ServiceProvider declarations should be shown during install and locked by plugin descriptor/package digest. If they change, the plugin requires re-approval. They do not need separate `ContributeTool`, `ContributeHook`, `RunBackgroundTask`, or `ProvideService` authority variants.

Service consumption is also not a blanket authority by itself. A service requirement is resolved by the host; the authority question depends on what host authority the service handle exposes. Service handles should be narrowed or authority-bound so acquiring one broad service handle cannot become an authority escalation path.

Tool execution remains governed by the existing per-call tool permission / PreToolCall path. Feature authority grants do not replace manifest/tool permissions.
