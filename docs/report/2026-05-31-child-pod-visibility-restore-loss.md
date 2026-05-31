# Child Pod visibility/restore loss during review flow

Date: 2026-05-31

## Summary

During the `workspace-memory-lint-cli` review flow, a spawned reviewer Pod appeared to stop producing notifications/output and then became impossible to attach/restore from the parent Pod. The parent later saw no spawned Pods at all, while a restore/prune notification reported that missing or unreachable delegated child Pods had been reclaimed.

This looks like a control-plane visibility/restore issue rather than an implementation-review issue. The lost Pod was read-only and the review was safely re-run in a new reviewer Pod, but the incident is worth recording because it undermines long-running multi-agent workflows.

## Observed sequence

1. `workspace-memory-lint-coder-20260531` completed implementation and reported commit `7a717f2 cli: add workspace memory lint`.
2. A read-only reviewer Pod was spawned:
   - `workspace-memory-lint-reviewer-20260531`
   - read scope: main workspace and `.worktree/workspace-memory-lint-cli`
3. Repeated `ReadPodOutput` calls returned:
   - `running; no new assistant text`
4. `InspectPod` still saw the reviewer as live/reachable/running at one point:
   - socket: `/run/user/1000/insomnia/workspace-memory-lint-reviewer-20260531/sock`
   - restore impossible only because the segment was locked by that live Pod
5. Later, after the user asked to restore it, `AttachOrRestorePod` failed:
   - `pod workspace-memory-lint-reviewer-20260531 is not visible to this Pod`
6. `ListPods` then reported no spawned Pods, and `ListVisiblePods` only showed the self Pod `insomnia`.
7. A notification appeared:
   - `Restored Pod state contained missing or unreachable delegated child Pods; their delegated write scopes were reclaimed before resume.`
8. The review had to be re-run by spawning a new read-only reviewer:
   - `workspace-memory-lint-reviewer-rerun-20260531`

## Impact

- Parent-side orchestration lost track of a child reviewer Pod that had previously been visible.
- The parent could not attach/restore by name because the child was no longer visible to the parent Pod.
- Any review result already produced by the lost child would have been hard to recover through normal parent tools.
- Multi-agent workflows that rely on long-running reviewer/coder Pods become less reliable if spawned-child visibility can disappear during parent resume/restore/prune.

In this instance the practical impact was low because the reviewer had read-only scope and the review could be re-run. The incident would be more serious for implementation Pods with unmerged write-scope work or for expensive/long review tasks.

## Why this matters

The current design intent is that Pod metadata is durable current state and spawned child registry persistence reuses Pod metadata. Parent-side tools should be able to inspect/attach/restore visible spawned children where durable state still records them, and pruning should be conservative enough not to erase reachable or recoverable child work prematurely.

This incident suggests at least one of these paths needs inspection:

- parent spawned-child registry persistence/restoration;
- pruning of unreachable children during parent restore;
- visibility rules for previously spawned child Pods after parent resume;
- distinction between live socket reachability, durable pod-store metadata, and parent-visible child registry;
- notification/read-output cursor behavior when a child is still running but no output arrives.

## Notes for follow-up

- The failure mode was not simply “child stopped”; the parent tool reported “not visible to this Pod,” which is different from stopped/unreachable.
- `InspectPod` had previously seen the child as live and locked; later `ListPods` returned no spawned Pods.
- The prune/reclaim notification may have happened after parent restore and may have removed child visibility state.
- A useful regression test would simulate parent restore with a child that is pending/running/unreachable at different phases and assert whether it remains visible, attachable, or intentionally pruned with a recoverable diagnostic.
- A workflow-level mitigation is to write important reviewer/coder outputs into ticket threads/artifacts promptly after reading them, and to re-run read-only reviewers if child visibility is lost.

## Current workaround

For `workspace-memory-lint-cli`, a replacement reviewer Pod was spawned with the same read-only task:

- `workspace-memory-lint-reviewer-rerun-20260531`

The original reviewer Pod was treated as lost/unrecoverable from the parent after `AttachOrRestorePod` reported it was not visible.
