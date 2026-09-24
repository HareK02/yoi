# Repository secret delivery expiry deletes a live Workdir helper and blocks restore

## Symptom

The `workspace:wip` T-3 Coder completed a clean local commit but could not publish it:

```text
branch=work/T-3-wip-protocol
head=742442f17417bfe0828291addd0249bb8a754ae9
origin=git@gitea.hareworks.net:Hare/wip-rs.git
```

The non-force push failed before contacting the Git remote:

```text
fatal: cannot exec '/home/hare/.local/share/yoi/workdirs/.repository-access/f536517181e31ae9/ssh-command': No such file or directory
fatal: unable to fork
```

Stopping and restoring the Coder to reconstruct Repository access then failed twice with:

```text
worker_workdir_attachments_replace_failed: Remote Runtime 'arcadia' rejected request
(HTTP 401 Unauthorized): invalid Workspace capability token:
Workspace capability does not authorize this operation
```

No branch was pushed and no Merge Request was opened.

## Preserved work

The implementation was not lost:

- Runtime Worker: `arcadia:01a0cfe9-2763-7070-8098-2088696ec9d5`
- implementation Workdir: `001a0cfe90e4e000003`
- read-only reference Workdir: `001a0cfe90c9a000002`
- branch: `work/T-3-wip-protocol`
- commit: `742442f17417bfe0828291addd0249bb8a754ae9`
- observed checkout: clean

The Worker is stopped, but its persisted restore request, logical attachments, execution metadata, and checkout remain. Cleanup, recreation, manual credential injection, and direct repository mutation are unnecessary.

## Cause 1: a delivery deadline was reused as the bound helper lifetime

The Workspace Server makes Repository SSH secret delivery available for 300 seconds. It stores the serialized private-key candidates and pinned host entry behind a Runtime-bound `FetchOnce` resource handle. Runtime consumes that resource once before binding the Workdir; successful fetch removes the Server-side resource.

This deadline has a meaningful role only while the one-shot secret has not been consumed. It limits how long an unused delivery handle remains valid. It is not a lifetime intrinsic to the SSH key, Repository permission, or Git operation.

The Runtime nevertheless copied the same absolute deadline into an active expiry scheduler after Workdir bind. At expiry the scheduler stopped and removed the already-bound Repository command access while the Workdir session remained live.

The observed timing matches that behavior:

| Event                         |               Timestamp |
| ----------------------------- | ----------------------: |
| T-3 Workdir linked to Coder   | 2026-09-23 20:15:59 UTC |
| push failed on missing helper | 2026-09-23 20:29:42 UTC |
| elapsed                       |   13 minutes 43 seconds |

The push happened well after the five-minute delivery deadline.

### What the missing helper was

`GIT_SSH_COMMAND` did not point to a private key. It pointed to a Runtime-generated executable script:

```text
.repository-access/f536517181e31ae9/ssh-command
```

A bound access consists of:

```text
.repository-access/<access-id>/
├── ssh-command   # invokes yoi-runtime __repository-ssh <b.sock> "$@"
├── known_hosts   # pinned host key
└── b.sock        # policy-enforcing Runtime broker socket

.repository-agents/<agent-id>/
└── agent.sock    # Runtime-internal ssh-agent socket
```

Runtime loads the fetched key into its internal `ssh-agent` through `ssh-add -`. The Worker receives neither the raw key nor the agent socket: its ordinary `SSH_AUTH_SOCK` is `/dev/null`. A Git SSH invocation reaches only `ssh-command`, whose broker validates the destination, port, Repository path, Git service, and read/write ceiling before starting `ssh` with the internal agent.

Thus the lending boundary is the brokered Git operation, not a five-minute key lifetime.

At the erroneous deadline, `RepositoryCommandAccess::stop`:

1. stopped the broker and removed `b.sock`;
2. killed the internal `ssh-agent` and removed its directory;
3. removed the complete `.repository-access/<access-id>` directory, including `ssh-command` and `known_hosts`.

The live `LocalWorkdirSession` retained the original `GIT_SSH_COMMAND` string, leaving a stale path. Git therefore failed at `exec`; it never reached Gitea. This is not evidence of a rejected key or missing remote write permission.

## Cause 2: Server and Runtime disagreed on attachment-replacement permission

Restore reacquires Repository SSH authority before rebinding persisted Workdirs. In `WorkspaceApi::restore_workspace_worker`, the relevant order is:

1. send fresh Repository access for each SSH-backed Workdir;
2. replace the Worker's Workspace API binding;
3. replace the Worker's logical Workdir attachments;
4. restore the Runtime Worker and bind the Workdirs.

The observed diagnostic is from step 3, at:

```text
POST /v1/workers/{worker_id}/workdir-attachments
```

The Server capability classifier recognized paths containing `/attachments`, but `/workdir-attachments` does not contain that literal substring. Server therefore fell through to `runtime:read`. Runtime explicitly classified `/workdir-attachments` as `workers:input`.

```text
Server-issued operation: runtime:read
Runtime expectation:      workers:input
```

The exact 401 text is `WorkspaceCapabilityVerificationError::WrongOperation`. Verification had already accepted token shape, Workspace trust/signature, Workspace, Runtime, binding revision, and Worker identity. Method, path, body-digest, and expiry checks occur later and were not reached.

Retrying the same restore without correcting the Server classifier deterministically returns the same 401.

## Failure chain

1. Runtime consumed the one-shot Repository secret and bound a policy broker to the Workdir session.
2. Five minutes later, the delivery deadline incorrectly stopped that live broker and deleted its helper files.
3. The Worker retained a stale `GIT_SSH_COMMAND`.
4. A later push failed before remote contact.
5. The Orchestrator stopped the Worker to reconstruct the binding.
6. Restore reacquired Repository access but signed attachment replacement as `runtime:read`.
7. Runtime required `workers:input` and rejected restore before Workdir bind.

The two defects are distinct. The first breaks long-running Workdir sessions; the second blocks the normal recovery path.

## Implemented correction

The correction preserves the existing security boundary without inventing a credential lifetime:

- the 300-second expiry remains on unconsumed one-shot secret delivery and pending pre-bind authority;
- binding validates and consumes that authority before its deadline;
- after bind, `RepositoryCommandAccess` is retained only as a Workdir session resource;
- every Worker Git SSH invocation still passes through the Runtime broker's host, Repository, service, and read/write policy checks;
- closing the Workdir session drops the resource, stops the broker and agent, and removes all helper files;
- expiry scheduler state and tests for active bound access are removed;
- a regression test waits beyond the consumed delivery deadline and proves the live helper remains until binding drop.

The restore correction adds `/workdir-attachments` to the Server's `workers:input` classifier and locks both Server issuance and Runtime verification with focused POST-route assertions.

## Safe recovery boundary

After deployment through the normal operator-controlled process:

1. restore the same stopped Worker;
2. confirm that the existing implementation Workdir remains clean at `742442f17417bfe0828291addd0249bb8a754ae9`;
3. let the assigned Coder perform the normal non-force push, remote-ref verification, Merge Request, and review flow.

No Runtime trust reprovisioning, credential rotation, manual helper construction, force push, Workdir copy, or Worker deletion is supported by the evidence.
