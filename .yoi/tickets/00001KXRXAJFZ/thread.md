<!-- event: create author: "yoi ticket" at: 2026-07-17T20:47:11Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T20:47:48Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T20:47:48Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-17T20:58:47Z -->

## Implementation report

Implemented reusable Worker-internal runner and migrated memory extract to use it.

Changes:
- Added `crates/worker/src/internal_worker.rs` with `InternalWorkerSpec`, `InternalWorkerRunResult`, `InternalWorkerRunError`, and `run_internal_worker`.
- The runner owns isolated Engine creation, limited tool registration, cache key / max turns, usage capture, and foreground-history isolation by construction.
- Updated memory extract path to run through the internal runner with only the extract tool surface supplied by the caller.
- Kept existing extract behavior: post-run threshold trigger, transitional `write_extracted` path, and flat staging output.
- Left persistence / Protocol exposure / session-explore feature for later Tickets.

Validation:
- `cargo fmt --check`
- `cargo test -p memory`
- `cargo test -p worker`
- `nix build .#yoi`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T20:58:47Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-17T20:58:47Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-17T20:58:47Z status: closed -->

## 完了

Implemented reusable Worker-internal runner and migrated memory extract to use it.

The new runner provides a Worker-owned path for isolated internal LLM jobs with caller-supplied prompt, input, limited tools, cache key, max turns, and usage capture. Memory extract now uses this runner while preserving the existing trigger and staging behavior.

Validation passed:
- `cargo fmt --check`
- `cargo test -p memory`
- `cargo test -p worker`
- `nix build .#yoi`


---
