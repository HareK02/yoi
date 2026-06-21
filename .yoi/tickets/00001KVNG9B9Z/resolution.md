Workspace web UI に sidebar navigation panel を追加し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- `web/workspace/src/lib/workspace-sidebar/` に sidebar components を追加:
  - `WorkspaceSidebar.svelte`
  - `RepositoriesNavSection.svelte`
  - `ObjectivesNavSection.svelte`
  - `WorkersNavSection.svelte`
  - `types.ts`
- `web/workspace/src/routes/+page.svelte` を sidebar + main content の responsive two-column layout に更新。
- Sidebar header に workspace label/name と disabled settings placeholder を表示。
- `repositories`, `objectives`, `workers` sections を追加。
- Objectives section は `/api/objectives` を読み、title/state と empty/error states を section-local に表示。
- Workers section は `/api/workers` を読み、Worker label/state/status/role を表示し、Pod を primary UI naming として露出しない。
- Repository section は future Repository API に繋げやすい placeholder seam として実装。
- Host/Worker API/UI merge 後の `+page.svelte` conflict を解消し、main Host/Worker content と sidebar skeleton を両方維持。
- Backend/API authority, SSR, mutation/business logic は追加していない。

統合・検証:
- Merge commit: `613f4126 merge: workspace sidebar navigation`
- Implementation commits: `d3b8bdfd`, `4ab696b4`
- Reviewer final verdict: approve
- Validation passed: `git diff --check HEAD^1..HEAD`, `deno task check`, `deno task build`, `cargo test -p yoi-workspace-server`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Settings page, Repository CRUD/API, Objective edit/detail UI, Worker start/stop/attach controls, drag/drop/collapsible tree, auth, multi-workspace switcher は実装していない。