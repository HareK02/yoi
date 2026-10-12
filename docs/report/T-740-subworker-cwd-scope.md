# T-740: delegated cwd and attachment coordinates

During T-740, two implementation children were spawned with a read rule for `.` and narrow write rules, plus `cwd: crates/ticket` or `cwd: crates/server-api`. Their committed sessions reported only the narrow crate attachment accessible; `docs/development/rust-testing-strategy.md` and, for the API child, the delegated client/Worker files were inaccessible. The API child correctly stopped rather than trying to bypass the denial. Its partial API changes were inspected and retained.

The parent had already read the test-design policy and reviewed the delegated tests against it. Remaining client/Worker work was assigned to a new child with `cwd: .`; that child could read the policy and implement its delegated targets. Parent integration and tests verified the actual checkout, not only child summaries.

This observation does not establish the provider's internal cause. The public `SubWorkerSpawn` description says cwd selects tool working directory, not Workdir/Profile/memory/Ticket roots or additional authority. Narrow cwd plus wider read/delegation rules should have predictable attachment coordinates, or the tool should report that only a restricted attachment can be provided before work starts. An early capability/attachment summary would avoid partial implementations and make mandatory policy-document access verifiable.

No authority workaround, process stop of the running host, or change to the control-plane tooling is included in T-740.
