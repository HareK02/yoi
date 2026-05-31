<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:19Z -->

## Migrated

Migrated from TODO.md entry without a legacy ticket file. No legacy review file was present at migration time.

---

<!-- event: plan author: hare at: 2026-05-31T00:51:55Z -->

## Plan

Planning note:

- Keep this in the existing user-facing `insomnia` binary implemented by the `tui` crate. Do not add another installed command for this ticket.
- The command should be headless: parse args, lint files, print report, exit. It must not initialize terminal UI or connect to a Pod.
- `insomnia memory lint` is preferred, but `insomnia memory` alone should continue to be a valid Pod-name attach/create path if practical with the current parser.
- Use `memory::Linter` directly so CLI behavior tracks tool pre-write validation. Existing files should be linted with `WriteMode::Update`.
- Keep crate rename / single-binary architecture as future cleanup, not part of this ticket.


---

<!-- event: implementation_report author: hare at: 2026-05-31T02:14:28Z -->

## Implementation report

Implementation report from coder Pod `workspace-memory-lint-coder-20260531`:

- Branch: `workspace-memory-lint-cli`
- Commit: `7a717f2d259563df562913e0c3ceb388b094b697` (`cli: add workspace memory lint`)
- Added `insomnia memory lint [--workspace <PATH>] [--json] [--warnings-as-errors]` as a headless mode in the existing `tui` crate/user-facing `insomnia` binary.
- `insomnia memory` alone remains a positional Pod name.
- The lint command resolves workspace root, collects existing summary/decisions/requests/knowledge records through `memory::WorkspaceLayout`, and lints with existing `memory::Linter` using `WriteMode::Update`.
- The command prints deterministic human output by default and stable JSON with workspace/files/errors/warnings/counts when `--json` is requested.
- Exit codes follow the ticket: 0 clean, 1 lint failures or warnings-as-errors, 2 usage/I/O/output/runtime failures.
- The headless path returns before raw terminal setup or Pod connection/spawn logic.

Validation reported by coder:

- `cargo fmt --check` passed
- `cargo test -p tui memory_lint -- --nocapture` passed
- `cargo test -p tui` passed
- `cargo check -p tui` passed
- `./tickets.sh doctor` passed
- `git diff --check` passed

Unresolved issues: none.


---

<!-- event: review author: hare at: 2026-05-31T02:14:28Z status: approve -->

## Review: approve

External review by reviewer Pod `workspace-memory-lint-reviewer-rerun-20260531`: approve.

The original reviewer Pod `workspace-memory-lint-reviewer-20260531` became non-visible to the parent before output could be recovered; this review was rerun with a replacement read-only reviewer Pod.

Reviewer summary:

- The implementation adds `insomnia memory lint` as a headless mode in the existing user-facing `insomnia` binary.
- The memory lint path branches before raw terminal setup and Pod connection/spawn logic.
- Parser tests preserve `insomnia memory` as positional Pod name behavior.
- The collector targets summary, decisions, requests, and knowledge records while ignoring opaque memory subsystem directories and workflow files.
- Existing `memory::Linter` and `WriteMode::Update` are used, and the code only reads files / writes reports.
- Human and JSON outputs are deterministic enough for the ticket, and exit code mapping matches requirements.

Blockers: none.

Non-blocking follow-ups:

- Add broader fixture coverage for `_staging`, `_usage`, knowledge, and decisions if desired.
- Add process-level exit-code integration tests if a CLI test harness is introduced later.

Validation adequacy: coder-reported validation is sufficient for this ticket. Reviewer additionally checked `git diff --check develop...HEAD` read-only.


---
