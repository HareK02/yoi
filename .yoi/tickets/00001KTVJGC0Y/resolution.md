Ticket tool users 全体に `ticket.language` guidance が届くようにした。

実装概要:
- `crates/ticket/src/tool.rs` に shared `ticket_tool_description(name, record_language)` を追加し、configured `ticket.language` がある場合は Ticket tool description に durable Ticket record / Ticket tool body language guidance を追加するようにした。
- `crates/pod/src/feature/builtin/ticket.rs` の builtin Ticket feature `ToolDeclaration` descriptions でも同じ helper を使い、read-only Companion-style context と lifecycle/Ticket-role-style context の両方に guidance が届くようにした。
- guidance は `worker.language` / `memory.language` / `ticket.language` を区別し、protocol literals、file paths、commands、logs、identifiers、quoted external text は fidelity 優先で保持することを明記した。
- guidance は Ticket tool / capability surface で model-visible であり、hidden context injection や `ticket_role` launch prompt fragment ではない。
- Companion/read-only authority や mutating tool exposure は拡大していない。

Review / integration:
- Implementation commit: `92c4dee7 ticket: guide Ticket tool language universally`
- Reviewer: `yoi-reviewer-ticket-language-guidance` が approve。
- Orchestrator merge commit: `ec66cad8 merge: ticket language guidance for tool users`
- Ticket completion commit: `2ba97b67 ticket: mark language guidance done`

Validation:
- `cargo test -p ticket ticket_record_language_guidance`: pass
- `cargo test -p pod ticket_language_guidance`: pass
- `cargo test -p client ticket_record_language_stays_out_of_first_run_text`: pass
- `cargo test -p ticket`: pass, 68 tests
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Known unrelated validation failure:
- `cargo test -p pod` still includes pre-existing failures in Pod orchestration guidance prompt assertion tests. These were verified on the Orchestrator branch before merge and reviewed as unrelated to this Ticket.

Cleanup:
- coder/reviewer Pods stopped。
- child worktree `/home/hare/Projects/yoi/.worktree/ticket-language-guidance-all-tools` removed。
- branch `ticket/ticket-language-guidance-all-tools` deleted。

Non-blocking risk:
- configured language guidance is appended to every Ticket tool description, including read-only tools. This repeats some prompt text, but it is the accepted universal capability-surface tradeoff for this Ticket.