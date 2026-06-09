Implemented, reviewed, merged, and validated.

Summary:
- Strengthened Orchestrator routing guidance so `ready` / `queued` Tickets are returned to `planning` only when a concrete missing decision/information remains after bounded project-context checks.
- Clarified risk flags, risky domains, and authority-adjacent work as context lookup / reviewer-focus / escalation signals rather than automatic stop gates.
- Added risky-but-specified guidance and an `allow-spawnpod-child-workspace-cwd`-style example for proceeding with reviewer focus when work is specified.
- Aligned `ticket-preflight-workflow`, `multi-agent-workflow`, and work-item docs with the same policy.
- Preserved workflow_state semantics and did not reintroduce preflight/readiness as a separate state or lane.

Implementation:
- Coder commit: `8576615 workflow: tighten orchestrator planning return policy`
- Reviewer approved with no blocking findings.
- Merge commit: `5af58b5 merge: tighten orchestrator planning return policy`

Validation after merge:
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

`cargo check --workspace` was intentionally omitted because the merged implementation is workflow/docs text only and no Rust code/tests changed; Nix build covered resource/package integrity.