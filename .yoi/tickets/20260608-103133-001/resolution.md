Implemented, reviewed, merged, and validated.

Summary:
- Enabled Markdown table parsing in the existing TUI chat/conversation Markdown renderer.
- Rendered ordinary pipe tables as readable monospace rows with separators and alignment-aware padding.
- Kept the change in display/rendering code only; no Pod history, session logs, worker history, prompt context, or stored message text mutation.
- Added focused tests for readable table rendering, ragged/wide table safety, and non-table pipe text regression.

Implementation:
- Coder commit: `f767ec7 tui: render markdown pipe tables in chat`
- Reviewer approved with no blocking findings.
- Merge commit: `e6bae04 merge: render tui markdown tables`

Validation after merge:
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
