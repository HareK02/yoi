# Workspace web SPA

This is the static SvelteKit shell for the local Yoi Workspace control plane.
It is intentionally a read-only UI bootstrap: `.yoi/tickets` and
`.yoi/objectives` remain canonical, and the Rust backend owns all business/API
semantics.

Package manager: npm with `package-lock.json` committed.

Commands:

```sh
npm install
npm run check
npm run build
```

Build output is `web/workspace/build/` and is not checked in. Point the Rust
backend `ServerConfig.static_assets_dir` at that directory (or another static
asset directory) to serve the SPA. `node_modules/`, `.svelte-kit/`, and `build/`
are generated local state and must remain ignored/excluded from package sources.
