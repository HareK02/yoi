<!-- event: create author: "yoi ticket" at: 2026-06-13T17:34:41Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-13T17:39:19Z -->

## Decision

修正:
- 初回作成時は git log の確認が不足しており、既存/別 branch の E2E 実装済み範囲を一部重複して書いていた。
- git log を確認し、既存の Panel E2E harness、latest-binary build、late quit latency E2E、mouse click selection E2E、orchestration branch の tmp runtime/data isolation を前提として反映した。
- この Ticket は残差に絞る: wheel regression、rewind UI real-process regression、必要最小限の critical path 維持/補強。


---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T17:51:04Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: implementation_report author: hare at: 2026-06-14T05:24:37Z -->

## Implementation report

Implemented critical-path TUI E2E coverage in commit b9f49eee.

Files changed:
- crates/tui/src/multi_pod.rs: panel mouse wheel events now move selection and emit e2e observer diagnostics without enabling full drag-motion capture.
- crates/tui/src/single_pod.rs: e2e-test-only canned single-Pod rewind fixture path for real TUI/PTTY driving; preserves production/non-production boundary.
- tests/e2e/Cargo.toml: disables auto-discovered integration tests and registers panel/rewind only behind the e2e feature.
- tests/e2e/src/lib.rs: harness support for rewind fixture env isolation, wheel input, event waiting, and forbidden ?1002h/?1003h capture detection.
- tests/e2e/tests/panel.rs: adds PTY wheel regression coverage and drag-capture failure assertion while keeping existing panel cases.
- tests/e2e/tests/rewind.rs: adds single-Pod rewind picker/apply/duplicate-Enter regression.

Validation:
- cargo fmt --check
- git diff --check
- cargo test -p yoi-e2e --no-run
- cargo build -p yoi
- cargo test -p yoi-e2e --features e2e
- cargo test -p tui --lib single_pod::tests::rewind
- cargo test -p tui --lib mouse
- nix build .#yoi

Additional note:
- Full cargo test -p tui was also attempted and failed in pre-existing/unrelated tests: multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace and spawn::tests::{profile_choices_include_builtin_and_project_default_marker, profile_choices_use_project_registry_default}. Focused changed-area tui tests passed.

Residual risks:
- Rewind E2E uses an e2e-test-only canned in-process fixture instead of a provider/socket-backed Pod, intentionally avoiding credentials/network while still exercising the real single-Pod TUI/PTTY path.
- nix build emitted the expected dirty-tree warning before succeeding.


---
