<!-- event: create author: tickets.sh at: 2026-05-31T12:40:40Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: hare at: 2026-05-31T20:41:28Z status: approve -->

## Review: approve

External reviewer: `dev-pod-runtime-env-reviewer-20260531`
Reviewed implementation commit: `0031953ed352ba7fae9e798b6aeee1e8ea080816` (`dev: add pod runtime command override`)
Verdict: approve

Summary:
- Added `INSOMNIA_POD_RUNTIME_COMMAND` as a narrow development-only executable override for the standard `insomnia pod ...` runtime command.
- Unset/empty values preserve `current_exe() + ["pod"]`.
- Non-empty values replace only the executable path; the `pod` prefix argument is still automatically added.
- The value is passed as a single `PathBuf`/program and is not shell-parsed or argument-split.
- Diagnostics for failed client spawn/restore include the resolved runtime command.

Requirements mapping:
- Override implementation lives in `client::PodRuntimeCommand`, preserving the post-CLI-owner dependency boundary.
- Spawn/restore paths continue to use typed command construction with `Command::new(program)` and prefix args.
- `docs/environment.md` documents the env var as a development-only escape hatch, not normal configuration.
- Old `INSOMNIA_POD_COMMAND` was not reintroduced and has no active non-work-item refs.
- No Pod runtime flag/profile/manifest/protocol semantic changes were found.

Blockers: none.

Non-blocking follow-up:
- `PodRuntimeCommand::Display` joins argv-like pieces with spaces, so diagnostics can be visually ambiguous when an executable path contains spaces. It is sufficient for this ticket, but a future quoted/structured argv display would be clearer.

Validation adequacy:
- Coder validation covered fmt, focused/full client tests, affected crate check, doctor, diff-check, and old env-name grep.
- Reviewer performed read-only diff/source/docs/grep review and did not rerun Cargo tests.


---
