# SubWorker cwd attachment-root ambiguity during T-697

## Observed barrier

A delegated backend investigation was spawned with `cwd=crates/workspace-server`
and explicit writable rules for both `crates/workspace-server` and
`crates/server-api`, plus repository-root read access. The child reported that
its available attachment exposed only the workspace-server crate; its observed
file reads used `src/server.rs`/`src/lib.rs` and it could not access server-api.
It also had no `ShowTicket` or parent-notification tool, despite the parent
having typed Ticket authority. No implementation changes were made by that child.

The parent inspected the committed child session, released the idle child, and
respawned at the repository root with the same delegated write boundaries and
an explicit summary of the authoritative Ticket requirements. That child could
then implement the cross-crate API boundary. No filesystem capability rejection
was worked around through another write path.

## Suggested improvement

The advertised SubWorker `cwd` contract says that it changes only the tool
working directory, not attachment roots. Show the effective attachment aliases,
logical roots and readable/writable scopes explicitly to the child and parent,
and test cross-crate reads/writes for a non-root cwd. If cwd intentionally
narrows or rebases an attachment, document that instead of presenting it as
only a command default. A denied/inaccessible sibling diagnostic should identify
the effective logical attachment root without disclosing unauthorized paths.

Also expose which typed domain and parent-messaging tools will be available
before spawning. Delegated implementation requires authoritative requirements;
where Ticket reads are unavailable, the parent must deliberately provide the
bounded contract rather than assume the child can recover it by ID. This report
records tool ergonomics only; it is not a new T-697 product requirement.
