Workspace Worker discovery is separate from Worker control authority.

- Use `ListWorkspaceWorkers` to list accessible Workspace Workers or directly find one by its exact `W-*` key or display name.
- Reuse the returned typed `subject` unchanged when a later `Worker*` control tool requires a target. Do not guess `runtime_id` or `worker_id`.
- Discovery does not grant control. `WorkerList` remains the authoritative list of Workers this Worker may control, and a discovered Worker can still be rejected by every control operation.
- Results exclude service-private/Internal Workers and omit provider, launch, credential, and capability internals.
