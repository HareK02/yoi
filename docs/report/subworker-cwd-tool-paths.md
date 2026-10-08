# SubWorker cwd changes logical tool path interpretation

During T-716 delegation, a child spawned with `cwd: crates/merge-request` and explicit read grants for `docs/development/rust-testing-strategy.md` could not read that path. `Read` was presented as Workdir-root-relative, but the observed error resolved it as `crates/merge-request/docs/development/rust-testing-strategy.md`. The same prefixing occurred for `crates/merge-request/src/lib.rs`; parent traversal was rejected.

The child made no edits. Rebinding a child at the checkout root with read-only `.` plus writable `crates/merge-request` let it read the required strategy and edit only the delegated crate. No denied write was bypassed.

Suggested improvement: make logical path resolution and child cwd semantics consistent in the tool description, capability checks, and execution. If cwd affects filesystem tools, expose the effective logical root explicitly and retain an unambiguous way to address other granted read paths. Scope validation should reject unusable grants at spawn rather than leave the child discovering path-prefix behavior.
