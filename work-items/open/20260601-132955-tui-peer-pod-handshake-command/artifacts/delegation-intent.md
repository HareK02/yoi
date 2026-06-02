# Delegation intent: peer Pod handshake and messaging

Intent:
- Investigate the existing Pod visibility/messaging implementation, then implement a TUI-initiated peer Pod handshake and peer messaging path if no design blockers are found.

Requirements:
- Start with investigation. Map current authority boundaries for `ListPods`, `RestorePod`, `SendToPod`, Pod metadata, spawned-child registry, protocol methods, and TUI `:` command handling.
- If there are design blockers or ambiguous authority decisions, write findings to the ticket artifacts and stop for parent decision.
- If there are no blockers, proceed with implementation in the worktree.
- Add a TUI `:` command that initiates peer handshake from the currently attached Pod to a target Pod by name.
- Make the peer relationship Pod-authoritative, durable where appropriate, and reciprocal by default.
- Ensure both Pods see each other in `ListPods` as peer/known, not spawned child.
- Add or safely broaden messaging semantics so a Pod can message a visible peer without granting spawned-child powers.
- Record delivered peer messages through an explainable durable path; do not silently mutate hidden context.
- Preserve explicit boundaries: no delegated filesystem scope, parent/child ownership, completion notification authority, or child output cursor authority for peer Pods.
- Add focused tests for protocol/runtime behavior, metadata persistence/restore, reciprocal `ListPods`, peer message authorization/delivery, and TUI command parsing/help where feasible.
- Update docs/help text for the new command and peer semantics.

Invariants:
- Do not treat peer Pods as spawned children.
- Do not reintroduce hidden context injection; model-affecting delivered messages must go through the normal committed history/event path.
- Do not add broad relationship graph semantics beyond the minimal peer relation.
- Do not edit the parent workspace; work only in the delegated worktree.
- Do not read ignored secret-like file contents.
- Do not close the ticket, merge the branch, delete worktrees, or push.

Non-goals:
- Arbitrary autonomous Pod-to-Pod background chatter.
- Replacing the multi-Pod dashboard or Pod picker.
- Delegated scope sharing between peers.
- E2E test framework design for real spawned processes.

Escalate if:
- Reciprocal handshake cannot be made atomic enough to avoid misleading partial state.
- Peer messaging requires changing history semantics in a way that could violate context/history invariants.
- Existing protocol or metadata schemas need a broad migration.
- The TUI command cannot safely deliver the runtime method while preserving current Pod authority.
- `SendToPod` broadening would confuse spawned-child and peer semantics.

Validation:
- Before implementation: write a short investigation summary in the implementation report or a separate artifact.
- Focused tests for touched crates.
- `./tickets.sh doctor` and `git diff --check`.
- `nix build .#yoi` if feasible; record if skipped and why.
- Commit the implementation in the worktree when reviewable.
