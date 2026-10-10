# Recovery session: SubWorker scope delegation is rejected

## Observed boundary

During state-contract cleanup after restoring the worktree, `WorkerList` returned no children. The attached External Workdir `yoi-origin` advertised READ/WRITE/COMMAND, and the parent could read files, write files, and execute build commands through that attachment.

A new `SubWorkerSpawn` requesting only recursive READ of `.` on `yoi-origin` was rejected before the child started:

```text
Invalid argument: scope parent-owned Workdir tools: Workdir operation denied: Workdir operation was denied
```

An empty scope is not supported (`scope must not be empty`), so a context-only child was also unavailable. No child was created, and no delegated work is running.

The response does not identify which delegation condition failed. Successful direct parent access does not establish delegation authority, so these observations alone do not establish whether the refusal is a policy restriction or a Provider implementation defect. No running process was stopped or reloaded to investigate this.

## Additional observed tool failure

`Edit` of `crates/workspace-server/src/store.rs`, after reading it through the same attachment, returned:

```text
Invalid argument: invalid argument: Workdir operation request is invalid
```

The parent could still modify that file using its granted command capability. This is an input-request failure observation, not evidence that `Edit` has the same root cause as the SubWorker denial.

## Diagnostic need

Return bounded, non-sensitive reason categories that distinguish direct access from delegation denial, and distinguish invalid tool requests from scope checks. Avoid reporting generic denial as proof that a child is running or that the parent lost its direct attachment authority.
