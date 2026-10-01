# Workspace web UI

SvelteKit static SPA for the Yoi workspace control plane.

The frontend is intentionally static. Workspace authority, validation, and API behavior live in the Rust `yoi-server` backend.

## Toolchain

Dependencies are pinned in `deno.json` and `package.json`, with resolutions in `deno.lock`. Run `deno install --frozen` to reproduce the installed versions. The current toolchain is SvelteKit 3, Svelte 5, Vite 8, and Vitest 5. TypeScript is kept on 6.0.3 because SvelteKit 3.0.0 and svelte-check 4.7.6 do not yet declare support for TypeScript 7.

SvelteKit and adapter settings live in `vite.config.ts`. Internal library imports use the `#lib/*` package subpath mapping with explicit file extensions, shared by Vite, TypeScript, and Deno. `tsconfig.json` extends `$app/tsconfig`; generated WASM JavaScript is consumed through its generated `.d.ts`, not checked as handwritten JavaScript. Deno-only tests under `test/` are type-checked by `deno task test`.

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
cargo run -p yoi-workspace-server --bin yoi-server -- serve --listen 127.0.0.1:8787
```

The backend reads Workspace records from the Yoi server DB at `<data_dir>/server/server.db`. Run `cargo run -p yoi-workspace-server --bin yoi-server -- init --workspace .` first when the server DB has not been initialized.

## Static build

Build the SPA:

```bash
deno task build
```

Static asset packaging is a deployment concern. Local development normally uses the Vite dev server proxy plus the Rust backend command above.

## Checks

```bash
deno task check
deno task test
deno task test:component
deno task build
```

## Design and architecture authority

- [`../../docs/development/ui-ux/design-language.md`](../../docs/development/ui-ux/design-language.md): visual and interaction language.
- [`../../docs/development/ui-ux/product-ux.md`](../../docs/development/ui-ux/product-ux.md): resource and navigation IA.
- [`../../docs/development/ui-ux/application-architecture.md`](../../docs/development/ui-ux/application-architecture.md): shell and nested override architecture.
- [`../../docs/development/ui-ux/visual-review.md`](../../docs/development/ui-ux/visual-review.md): implementer-owned visual validation.

This README owns frontend implementation guidance and concrete source paths. It does not redefine product or visual rules from those documents.

## CSS ownership

`src/app.css` is the global foundation and owns only:

- font imports;
- cascade layer order;
- semantic tokens and light/dark theme;
- reset and base element typography;
- global focus and text-selection behavior.

It must not own page layout, feature class selectors, table or definition-list layout, button variants, status presentation, helper spacing, or anchor color. Navigation, inline links, and action links own their color in the relevant component stylesheet.

Feature styles are owned by their implementation area:

- `src/lib/workspace/styles/workspace-pages.css`: Workspace main-content primitives.
- `src/lib/workspace/styles/tickets.css`: Ticket surfaces.
- `src/lib/workspace/styles/workers.css`: Worker surfaces.
- `src/lib/workspace/styles/settings.css`: Settings, Account, and form surfaces.
- `src/lib/workspace/styles/workspace-catalog.css`: Workspace catalog.
- `src/lib/workspace/sidebar/sidebar.css`: Sidebar components.
- Svelte-local styles: behavior-specific presentation that is not reused outside that component.

Use semantic tokens from `app.css`. Do not introduce component-local color, spacing, font, or z-index systems.

## Source map

- `src/routes/+layout.svelte`: root application shell and override contexts.
- `src/routes/w/[workspaceId]/+layout.svelte`: Workspace Header and Sidebar registration.
- `src/routes/w/[workspaceId]/settings/+layout.svelte`: Settings Sidebar registration.
- `src/lib/workspace/home/`: bounded, read-only work dashboard feeds and their presentation.
- `src/lib/workspace/settings/WorkspaceIdentitySettings.svelte`: Workspace-scoped name editing, public identity disclosure, and deletion confirmation.
- `src/lib/workspace/header/`: Header frame, context, and overrides.
- `src/lib/workspace/sidebar/`: Sidebar frame, scoped content, contexts, and override stack.
- `src/lib/workspace/markdown/`: shared safe Markdown parsing plus presentation-specific document renderers.
- `src/lib/workspace/ui/`: generic presentation components such as `Tooltip`, `Bevel`, and `BevelLine`.
- `src/routes/design-lab/workspace-web-ux/`: static Design Language showroom.

When one of these paths changes, update this source map in the same change. Stable architecture and UX authority remain in `docs/`; this map follows the current implementation.
