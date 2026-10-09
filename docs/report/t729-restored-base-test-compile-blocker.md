# T-729: restored base test-only compile blocker

The assigned checkout starts at `f9d4c97f` (WIP 0.2 integration). On 2026-10-08,
`cargo test -p yoi-workspace-server --lib auth_logging_tests` could not compile
because `server_workspace_config_wip_tests.rs` still calls `WipRuntime::discover`
(lines 152 and 192), which no longer exists, and external `WipRuntime::call`
(lines 170 and 200), which is now crate-private. At the initial failure those
files were unchanged by the T-729 diagnostic work.

A narrow test filter still builds all lib test modules, so it cannot isolate the
new authentication boundary tests from this unrelated test API mismatch. This
was escalated on T-729 instead of silently disabling those tests or widening the
Ticket to migrate unrelated configuration tests. Future API migration work should
check affected test-only consumers as well as the normal compilation closure.

The additional no-default-features check also fails in unchanged Runtime modules:
`resource.rs`, `worker_backend.rs` and `worker_source.rs` import feature-gated
`workspace_request`, `reqwest` and `futures` unconditionally. The no-default lib
test build additionally calls feature-gated filesystem constructors. This is not
an auth diagnostic regression and was not repaired by broadening T-729. Default
Runtime crate tests pass (345 library tests, 17 tests in each of the two binaries);
root `cargo check` passes. After correcting the T-729 SHA formatting, the Server
lib test build reports only the four existing WIP API errors above.

## Resolution of the Server prerequisite

Ticket T-729 seq7 explicitly authorized a narrow test-only migration. Commit
`717ff255` replaces the helpers with public Tree, path-centered Inspect and
structured InterfaceReference Invoke. All existing Backend authority, attachment,
mutation, stale observation and unknown-outcome assertions are retained. All 12
`workspace_config_integration` tests pass, and the authentication logging tests
now run (3 passed). No production API was exposed or legacy API revived. The
no-default-features issue remains separate and unresolved.
