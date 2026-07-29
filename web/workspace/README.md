# Workspace web UI

SvelteKit static SPA for the Yoi workspace control plane.

The frontend is intentionally static. Workspace authority, validation, and API
behavior live in the Rust `yoi-server` backend.

## Development

Use two terminals from the repository checkout.

Backend terminal:

```bash
cd web/workspace
deno task dev:backend
```

Frontend terminal:

```bash
cd web/workspace
deno task dev
```

The Vite dev server proxies `/api/*` to `http://127.0.0.1:8787`, so frontend hot
reload works while the Rust backend serves the workspace API. Open the Vite URL
printed by `deno task dev`.

If you want to run the backend from the repository root instead:

```bash
cargo run -p yoi-workspace-server --bin yoi-server -- serve --listen 127.0.0.1:8787
```

The backend reads Workspace records from the Yoi server DB at
`<data_dir>/server/server.db`. Run
`cargo run -p yoi-workspace-server --bin yoi-server -- init --workspace .` first when the server
DB has not been initialized.

## Static build

Build the SPA:

```bash
deno task build
```

Static asset packaging is a deployment concern. Local development normally uses
the Vite dev server proxy plus the Rust backend command above.

## Checks

```bash
deno task check
deno task build
```
