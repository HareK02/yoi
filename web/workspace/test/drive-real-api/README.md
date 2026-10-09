# Real Drive API/Web roundtrip (not the browser fixture)

This harness uses a fresh RAII temporary directory, real Server SQLite records, FeatureStorage Drive nodes and `object_store::local::LocalFileSystem` blobs. It creates a Workspace through T-717's actual repositoryless `/api/workspaces` endpoint. No repositories, user's data, live Server, environment-variable test configuration, Tools or WIP are involved.

The ordinary Rust test exercises loopback HTTP signed Worker ingress: explicit read-only/read-write grants, rejection of read-only writes, Worker-created bytes visible through Web/user API, and immediate grant revocation (even when the request also carries an owner's bearer token). Only the Runtime worker-spawn boundary uses the existing deterministic workdirless fixture runtime.

The explicit process test starts a loopback Axum Server and launches Deno against it. `roundtrip.ts` imports the production `src/lib/workspace/drive/api.ts` adapter, not a copy or mock. Its typed `fetchFn` resolves relative paths only to the bound loopback origin and adds fixture bearer authorization. Tokens travel over stdin, not command-line arguments or logs; Rust failure diagnostics defensively redact both fixture bearer tokens. Deno has network permission only for the exact loopback port, uses no npm dependencies, and runs with `--cached-only`. Each listener is abort-on-drop, child processes are kill-on-drop, and the child has a 60-second upper timeout, without sleeps or readiness polling.

Coverage: Worker file list/read, Japanese folder/Markdown names, create/read/edit/download, empty files, actual PNG upload/authenticated image/download and binary update, publication receipt lookup, paged list, two authenticated accounts racing a common revision (one commit/one typed conflict, no hidden retry), stable latest URL through rename and move, deleted URL and same-name recreation, and unauthorized latest URLs. The exact Workers-page `drive-grants/api.ts` adapter also lists persisted grants, creates both access levels, revokes them and re-reads durable state, and rejects member administration. Its global browser fetch is temporarily origin/auth-bound in this isolated child; network responses are never mocked. This tests real authenticated HTTP with test bearer credentials, not a Passkey/cookie browser login or a full Runtime process. UI dirty-draft retention, delayed response handling, visual review and browser cancellation belong to the **separate browser fixture**, not this API test.

## Registration

The inline `mod tests` block in `crates/workspace-server/src/server.rs` registers the module alongside the existing Drive API tests:

```rust
mod drive_web_roundtrip_tests;
```

No product routing modification is needed. The scoped production router is cached by the harness without starting unrelated Orchestrator hook tasks; production server ingress, authorization and API/storage handlers still run.

## Run

From the repository root:

```sh
cargo test -p yoi-workspace-server --lib drive_web_roundtrip_tests
cargo test -p yoi-workspace-server --lib drive_web_roundtrip_tests -- --ignored
```

The second command requires Deno on PATH. Its ignored marker is an explicit process/E2E prerequisite, not an environment-variable gate or silent fallback. An absent Deno fails the explicit test. The ordinary real HTTP test does not require Deno.

The script is harness-only: it receives fresh credentials and Workspace identity from the parent over stdin; do not invoke it against a live Server. Type-check it without starting the Server:

```sh
deno check --config web/workspace/test/drive-real-api/deno.json \
  web/workspace/test/drive-real-api/roundtrip.ts
```

## Validation evidence

Validated against the production Server endpoints and Web adapter, not the browser mock routes:

- `cargo test -p yoi-workspace-server --lib drive_web_roundtrip_tests -- --test-threads=4`: **1 passed, 1 explicitly ignored** (the process prerequisite).
- `cargo test -p yoi-workspace-server --lib drive_web_roundtrip_tests -- --ignored --test-threads=4`: **1 passed**; the actual Deno adapter process completed against real SQLite/LocalFileSystem HTTP.
- `cargo test -p yoi-workspace-server -- --test-threads=4`: **693 library tests passed, 1 ignored; 11 binary tests passed; doc-tests passed**.

The repositoryless fixture was adjusted to retain the production runtime trust gate's exact normalized binding snapshot and to scope the reusable Runtime observation to the newly bootstrapped Workspace. Neither fix changes the API, Drive authorization policy, Tools, WIP, or browser fixtures.
