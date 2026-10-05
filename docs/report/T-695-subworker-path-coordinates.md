# SubWorker cwd and logical paths during T-695

Two writable child sessions initially reported missing Workdir/Ticket access.
The concrete observations did not justify that conclusion:

- A default-profile child had no `ShowTicket` tool, while the `inherit` child
  could read the Ticket. Product requirements therefore had to be forwarded or
  the effective child profile chosen explicitly.
- A child spawned with `cwd: crates/workspace-server` received
  `file not found: crates/workspace-server/crates/workspace-server/src/config_source.rs`
  for a root-relative `Read` path. `Read("src/config_source.rs")` succeeded.
- `Read("../server-api/src/lib.rs")` was rejected as an invalid logical path,
  despite both crate directories being in the declared child scope.
- The read-only investigation child successfully inspected repository sources;
  successful spawning was not evidence that every child had the expected tools,
  but one invalid absolute output-directory lookup was not evidence of an
  inaccessible repository either.

The parent inspected committed child sessions and preserved their changes.
The server child was released at Idle, then continued under an explicitly
`inherit`, root-cwd child with the same per-crate write grants and root read
scope. No host-path or parent-traversal workaround was used to bypass the
rejected path. The original prerequisite diff and test evidence were retained.

Improvement: tool descriptions and generated child startup context should agree
on whether relative file paths are attachment-root-relative or cwd-relative.
Expose effective attachment aliases, root/cwd coordinates and granted tool
surface directly to the child, so a profile/tool mismatch is not misdiagnosed
as a repository authority failure. Scope validation should distinguish invalid
path syntax, missing file, missing attachment and actual permission denial.
