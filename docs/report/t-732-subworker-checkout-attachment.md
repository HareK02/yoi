# T-732: read-only SubWorker did not receive its delegated checkout

During T-732, the parent spawned `concurrency-audit` with read-only scope `.` for a source-boundary audit. Spawn succeeded, but the child's committed session reported no Workdir attachment. Its Glob/Read/Grep calls could not inspect the checkout; WorkerList also exposed no alternative source-observation subject. The parent inspected the child's committed final entry and stopped it without treating the audit as completed.

The parent could read and modify the assigned checkout through its own tools. This is an attachment/delegation surface problem, not a Rust-source finding. The parent continued the investigation itself; no child audit or review verdict is claimed.

Expected behavior: a delegated read scope should be bound to a concrete parent Workdir alias, or spawn should reject it with an actionable error rather than start an inaccessible child. The UI/tool response should expose the attachment binding clearly. A trusted Reviewer also needs a usable source observation surface; a successful Spawn alone must not be mistaken for independent source review.

No grant was broadened, no hidden paths were explored by the parent to repair the child's authority, and no live process/environment update or T-731 resume was attempted.
