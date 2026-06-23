Workspace web Repository Ticket Kanban の grouping / lazy rows 改善を統合した。

主な成果:
- Repository Ticket Kanban を `RepositoryTicketKanban.svelte` component に分離。
- `planning` + `ready` を display-only group とし、`ready` を `planning` より上に表示。
- `queued` + `inprogress` を display-only group とし、`inprogress` を `queued` より上に表示。
- `done`, `closed`, `other` は独立 group として維持。
- 各 row に original Ticket state を表示。
- 各 group の初期表示行数を 30 に cap。
- 各 group に独立 scroll area と independent lazy visible count を実装。
- High-volume `closed` group が page height を無制限に伸ばさないようにした。
- `WorkspacePage.svelte` から inline Kanban logic/markup を削減。
- Styling は existing design tokens を使い、backend/API/Ticket lifecycle semantics は変更していない。

統合・検証:
- Merge commit: `eea26f91 merge: kanban lazy rows`
- Implementation commit: `6f68bb8d web: group repository ticket kanban rows`
- Reviewer final verdict: approve
- Validation passed: `git diff --check HEAD^1..HEAD`, `deno task check`, `deno task build`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Backend pagination、Ticket state mutation UI、drag/drop Kanban、browser/manual scroll E2E tests は追加していない。