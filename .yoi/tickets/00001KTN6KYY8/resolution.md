Implemented, reviewed, merged, and validated.

Summary:
- Added reusable `crates/session-analytics` library with `analyze_session(path) -> SessionReport`.
- Added thin product CLI `yoi session analyze <SESSION_JSONL_PATH> --json` that delegates to the library.
- Implemented tolerant streaming JSONL parsing with bounded malformed/unknown-entry diagnostics.
- Report covers tool usage counts, calls per turn, failed tool counts, repeated reads with mutation/context-lifecycle observations, edit/write churn, `replace_all`, large edit/result observations, saved/truncated Bash outputs, and compaction/prune/context correlations.
- Default report avoids raw user input, raw tool arguments, raw file contents, raw session snippets, and raw tool output content.
- Tests use synthetic/minimal fixtures rather than private local sessions.
- No Worker pruning/compaction, Pod protocol/history/session writer, TUI dashboard, prompt auto-fixer, or LLM summarization behavior was changed.

Implementation:
- Coder commit: `c1809b3 feat: add session analytics tooling`
- Reviewer approved with no blocking findings.
- Merge commit: `0d2a6a7 merge: add session analytics tooling`

Validation after merge:
- `cargo test -p session-analytics`
- `cargo test -p yoi session_cli`
- `cargo test -p yoi parse_session_analyze_uses_session_mode`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
