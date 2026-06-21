# Workspace web SPA

This is the static SvelteKit shell for the local Yoi Workspace control plane.
It is intentionally a read-only UI bootstrap: `.yoi/tickets` and
`.yoi/objectives` remain canonical, and the Rust backend owns all business/API
semantics.

Canonical frontend tooling: Deno. Dependency versions, tasks, and the committed
lockfile are managed by `deno.json` and `deno.lock`.

`package.json` is intentionally kept only as minimal SvelteKit/Vite ecosystem
metadata (`type: module`, private package identity). It does not define scripts
or dependencies and is not the package-manager source of truth.

Commands:

```sh
deno install
deno task check
deno task build
```

`deno task dev` and `deno task preview` are available for local frontend work.
Deno uses npm compatibility for the SvelteKit/Vite toolchain, so `node_modules/`
may be created as generated local state; do not check it in.

Build output is `web/workspace/build/` and is not checked in. Point the Rust
backend `ServerConfig.static_assets_dir` at that directory (or another static
asset directory) to serve the SPA. `node_modules/`, `.svelte-kit/`, and `build/`
are generated local state and must remain ignored/excluded from package sources.
