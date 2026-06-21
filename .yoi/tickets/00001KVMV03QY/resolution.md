Workspace web SPA の frontend tooling を npm/Node-primary から Deno-primary に移行し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- `web/workspace/deno.json` を追加し、Deno task を canonical workflow にした。
- `web/workspace/deno.lock` を追加。
- `web/workspace/package-lock.json` を削除。
- `web/workspace/package.json` は SvelteKit/Vite ecosystem compatibility metadata のみに縮小し、scripts/dependencies を削除。
- README を Deno workflow (`deno install`, `deno task check`, `deno task build`, `deno task preview`) と source-of-truth 説明に更新。
- Static SPA output path `web/workspace/build/` と `@sveltejs/adapter-static` assumptions を維持。
- Rust backend/static serving code、Workspace API authority、Ticket/Objectives authority、`.yoi` canonical record workflows には変更を加えていない。
- Generated artifacts (`node_modules`, `.svelte-kit`, `build`) は ignored/source-filtered のまま。

統合・検証:
- Merge commit: `6dc78e3f merge: workspace spa deno tooling`
- Implementation commit: `66f04e04 feat: migrate workspace spa tooling to deno`
- Reviewer final verdict: approve
- Validation passed: `git diff --check HEAD^1..HEAD`, `deno task check`, `deno task build`, `deno task install`, `cargo check -p yoi-workspace-server`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Backend API / static serving implementation は変更していない。
- Protocol reconnect work (`00001KVMT2J25`) には触れていない。
- SSR / Deno runtime server / Deno Deploy assumptions は追加していない。