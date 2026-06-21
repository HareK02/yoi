Workspace web control plane bootstrap を実装し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- New backend library crate `yoi-workspace-server` / `crates/workspace-server` を追加。
- Axum-based read-only HTTP API と static/SPA serving surface を追加。
- `/api/...` と static/SPA fallback を分離し、API route miss を SPA fallback に飲ませない設計にした。
- `ControlPlaneStore` trait と SQLite implementation `SqliteWorkspaceStore` を追加。
- SQLite migration/version table、WAL、foreign keys、busy timeout を設定。
- `.yoi/tickets` と `.yoi/objectives` を canonical read sources として扱う local project-record bridge を追加し、既存 record workflow を移行・変更しない。
- Read APIs: `/api/workspace`, `/api/tickets`, `/api/tickets/{id}`, `/api/objectives`, `/api/objectives/{id}`, `/api/runs`, `/api/runners`。
- Future runner/event-stream extension seams を response/state shape に用意しつつ、scheduler/runner dispatch/write API は実装しない。
- SvelteKit static SPA skeleton を `web/workspace` に追加し、npm lockfile、static adapter、README、generated artifact ignore/source-filter handling を追加。
- `Cargo.lock` と `package.nix` cargo hash / source filtering を更新。

統合・検証:
- Merge commit: `3e03e536 merge: workspace web control plane`
- Implementation commit: `ab7658c1 feat: bootstrap workspace web control plane`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p yoi-workspace-server`, `cargo check -p yoi-workspace-server`, `cargo check -p yoi`, `cd web/workspace && npm ci && npm run check && npm run build`, `cargo run -p yoi -- ticket doctor`, `cargo run -p yoi -- objective doctor`, and `nix build .#yoi --no-link`。

範囲外 / deferrals:
- Product CLI/server launch command は未追加。backend library exposes `serve(...)`; launch surface は future Ticket で設計する。
- Write API、runner job dispatch、scheduler、hosted/multi-tenant auth、billing/quota、memory migration は実装していない。
- Event stream implementation は未実装で、extension seam のみ。
- Generated SPA assets are not committed; configured static directory such as `web/workspace/build` can be served after frontend build。