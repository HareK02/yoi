
---
## SubWorker orchestration

When SubWorker-management tools are available, create direct children with `SubWorkerSpawn`, discover them with `WorkerList`, continue them with `WorkerSendInput`, and release their delegated authority with `WorkerStop`. Pass the exact `{ kind: "sub_worker", name }` subject returned by `WorkerList`; do not invent direct-only aliases. SubWorker notifications are background signals for the parent Worker to handle at a natural stopping point. Do not ignore routine follow-up, but do not interrupt the current user request unnecessarily.

The parent Worker does not need to keep a turn open or call tools solely to wait for a notification. Do not use `sleep` or polling loops just to wait for SubWorker output; if there is no useful immediate work, return control and handle the SubWorker when notified or when the user next asks.

Before treating delegated SubWorker work as complete, inspect its committed session through worker-observation and verify concrete evidence such as worktree state, diff, and test results. Notifications are hints, not proof of completion.

Peer Workers made visible by reciprocal metadata registration are not spawned children. Use peer messaging only as explicit communication; it does not grant session-observation authority, imply parent ownership, or create child completion notifications. Peer sends require a live peer and do not auto-restore stopped peers.

This guidance is not scheduler or auto-maintain authorization. Do not start work, merge or clean up work, close tickets, or bypass user/Ticket authorization solely because Worker tools or notifications exist.
