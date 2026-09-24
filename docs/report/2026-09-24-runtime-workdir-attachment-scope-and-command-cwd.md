# Runtime Workdir attachment scope and implicit command cwd

## Symptom

A Runtime Worker with two local Workdir attachments retained the correct attachment identities and capabilities, but `Read`, `Write`, and Internal SubWorker scope delegation rejected every path below either Workdir root:

```text
Workdir path `crates` exceeds the provider attachment scope
```

The same Worker could still modify the checkout through Bash.

## Cause

The multi-attachment launch and restore paths intentionally removed the legacy single local filesystem authority, but `runtime_local_workdir_router` continued to give every local attachment session the Worker's shared global scope. Profiles used by Workspace Workers can have an empty global filesystem scope because Workdir attachments are their filesystem authority. Consequently, each provider session had the advertised attachment capabilities but no readable provider path.

Bash masked this defect. `CommandRequest.cwd` was optional, Bash always sent `None`, and `LocalWorkdirSession::start_command` used its stored cwd without applying the scope check used for an explicit cwd. Typed filesystem operations failed closed while command execution bypassed the missing attachment scope.

## Correction

Runtime-local attachment sessions now receive independent filesystem scopes derived from their own materialized root and capabilities:

- sessions with write, edit, or command authority receive recursive write permission for that attachment root;
- read-only sessions receive recursive read permission;
- an empty capability set receives an empty scope.

The provider-owned Bash output scope remains separate from attachment filesystem authority. This preserves dynamic authorization of the Worker-local output directory without merging attachment roots into a Worker-global scope.

`CommandRequest.cwd` is now required. The Bash tool schema also requires a Workdir-relative `cwd`, and the local provider validates that explicit path before every process start. Scoped sessions translate the explicit child-relative cwd before forwarding the request; they no longer synthesize one when absent.

## Regression coverage

Tests verify that:

- a restored Runtime-local read-only attachment can read its own root even when the Worker-global scope is empty;
- its provider scope authorizes delegated read rules;
- attachment scope permission follows session capabilities;
- Bash rejects input without `cwd` before command start;
- local command execution rejects an explicit cwd outside the attachment scope;
- transported command requests preserve the required cwd while stripping provider-local spill paths.
