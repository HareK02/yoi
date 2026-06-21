# Workspace web UI

SvelteKit static SPA for the Yoi workspace control plane.

The frontend is intentionally static. Workspace authority, validation, and API behavior live in the Rust `yoi-workspace-server` backend.

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

The Vite dev server proxies `/api/*` to `http://127.0.0.1:8787`, so frontend hot reload works while the Rust backend serves the workspace API. Open the Vite URL printed by `deno task dev`.

If you want to run the backend from the repository root instead:

```bash
cargo run -p yoi-workspace-server -- serve \
  --workspace . \
  --db .yoi/workspace.db \
  --listen 127.0.0.1:8787
```

## Static build served by Rust backend

Build the SPA:

```bash
deno task build
```

Then serve the static build and API from the Rust backend:

```bash
cargo run -p yoi-workspace-server -- serve \
  --workspace ../.. \
  --db ../../.yoi/workspace.db \
  --frontend build \
  --listen 127.0.0.1:8787
```

From the repository root, the equivalent command is:

```bash
cargo run -p yoi-workspace-server -- serve \
  --workspace . \
  --db .yoi/workspace.db \
  --frontend web/workspace/build \
  --listen 127.0.0.1:8787
```

## Checks

```bash
deno task check
deno task build
```
