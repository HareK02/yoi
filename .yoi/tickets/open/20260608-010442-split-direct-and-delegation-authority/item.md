---
id: '20260608-010442-split-direct-and-delegation-authority'
slug: 'split-direct-and-delegation-authority'
title: 'Split direct and delegation authority for Pods'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['pod', 'scope', 'delegation', 'orchestrator', 'security', 'profile']
workflow_state: 'inprogress'
created_at: '2026-06-08T01:04:42Z'
updated_at: '2026-06-08T06:38:27Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T05:45:43Z'
---

## Background

Orchestrator currently needs enough authority to delegate write scope to Coder Pods, because `SpawnPod` requires child scope to be a subset of the parent Pod's effective write scope. After project profiles were adjusted to make non-coder roles read-only by default, `project:orchestrator` became read-only and could no longer spawn Coder Pods with write scope.

Observed active metadata for `yoi-orchestrator` showed only read scope:

```json
"scope": {
  "allow": [
    { "permission": "read", "target": "/home/hare/Projects/yoi", "recursive": true }
  ]
}
```

The immediate workaround was to edit the Pod metadata under the user data `pods/` directory to grant the Orchestrator write authority. That is not a good long-term boundary.

The design issue is that Orchestrator may need *delegation authority* to give a child Coder a bounded write scope, while not necessarily needing broad direct write authority for its own tool calls. Today those two concepts are conflated in the same effective write scope.

## Goal

Separate a Pod's direct tool/write authority from its authority to delegate scoped write permissions to child Pods.

## Requirements

- Model direct filesystem/tool authority separately from delegation authority.
  - Direct authority: what the Pod itself may do with tools such as Bash/Edit/Write/Ticket tools.
  - Delegation authority: what scope the Pod may grant to child Pods through `SpawnPod`.
- Allow Orchestrator-like roles to delegate bounded child write scopes without necessarily granting themselves arbitrary direct workspace write tools.
- Preserve safety invariant that delegated child scope must be authorized by an explicit parent grant; do not allow arbitrary escalation.
- Ensure explicit denies still apply correctly to both direct and delegated permissions.
- Make the relationship between Profile scope, resolved Manifest scope, and delegation scope explicit and inspectable.
- Update `SpawnPod` preflight/policy to validate against delegation grants rather than only direct effective write scope if the split is implemented.
- Update profile authoring guidance so role profiles can express patterns such as:
  - Companion: direct read, no write delegation.
  - Intake/Reviewer: direct read, no write delegation by default.
  - Orchestrator: limited direct writes for Ticket/project records if needed, plus delegation authority for child worktrees.
  - Coder: direct write only within delegated worktree, no further delegation unless explicitly granted.
- Avoid requiring users to hand-edit `~/.yoi/pods/<name>/metadata.json` to repair Orchestrator delegation authority.
- Provide diagnostics when a Pod cannot spawn a child due to missing delegation authority, distinguishing it from missing direct write authority.

## Design questions

- Should delegation authority be represented as a separate `scope.delegate` / `scope.grant` section in Manifest/Profile resolution?
- Should delegation grants be path-scoped independently from direct grants?
- Should Ticket role launcher supply Orchestrator delegation grants based on workspace/worktree policy rather than broad workspace write?
- How should restored older Pod metadata without delegation grants behave?
- Should direct Ticket tool authority be separated from filesystem write authority in the same change, or remain a separate host-authority concern?

## Acceptance criteria

- Orchestrator can spawn a Coder with write scope to a child worktree without necessarily having broad direct write access to the whole workspace.
- A Pod without delegation authority cannot grant write scope even if it has read access.
- A Pod with direct write but no delegation grant cannot spawn write-capable children unless explicitly allowed.
- Diagnostics distinguish direct permission denial from delegation-authority denial.
- Profiles/manifests can express at least the Companion/Intake/Orchestrator/Coder/Reviewer patterns above.
- Tests cover delegation validation, deny handling, restoration/metadata behavior, and SpawnPod error messages.
- Relevant docs/profile examples are updated.
- Focused tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
