Implemented, reviewed, merged, and validated.

Summary:
- Added persistent TUI composer recall history per workspace.
- Stores history under the user data dir at `composer-history/workspaces/<workspace-label>-<fnv64-key>/history.json`, not under workspace `.yoi/`.
- Stores workspace metadata and typed `Segment` vectors, not flattened strings.
- Persists only non-blank submissions, suppresses consecutive duplicates, and bounds history to 30 entries per workspace.
- Preserves TUI-local/non-destructive recall semantics: recall does not mutate Pod protocol, worker history, session transcript, model context, or conversation state until explicit submit.
- Corrupt history files fall back to empty history with a bounded generic warning.

Implementation:
- Coder commit: `64b7ff7 tui: persist composer history`
- Reviewer approved with no blocking findings.
- Merge commit: `b616420 merge: persist tui composer history`

Validation after merge:
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
