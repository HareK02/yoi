# Svelte validation rewrites the live dev server route manifest

While investigating shared sidebar leakage, running `svelte-check --tsconfig ./tsconfig.json` in `web/workspace` rewrote every file under `.svelte-kit/generated`, even though no routes had been added or removed. The live Vite server watches the same directory and reacted with repeated full-page reloads for all generated node modules.

The failure is more severe than reload noise. During the rewrite, Vite retained module transforms keyed by the previous generated node numbers. Subsequent hard reloads hydrated routes with unrelated layouts; for example, `/design-lab/workspace-web-ux` attempted to hydrate `src/routes/w/[workspaceId]/settings/+layout.svelte`, issued requests containing `undefined` route parameters, and failed with a DOM hierarchy error. The generated files on disk were internally consistent after the command completed, but the running dev server remained inconsistent until restarted.

This is reproducible in an isolated copy: after `svelte-kit sync`, recording `.svelte-kit/generated` mtimes and then running `svelte-check` shows that the checker rewrites the generated route tree. `vite build` also uses the same `.svelte-kit` directory.

For this investigation, validation was moved to `target/yoi-web-validation`, with the current source copied there and the existing `node_modules` linked in. `svelte-kit sync`, `svelte-check`, production build, and browser route-transition checks then ran without mutating the live server's output.

Suggested improvement: make repository Web validation use an isolated SvelteKit `outDir` or a disposable copied workspace by default. Validation commands must not share `.svelte-kit` with a running dev server. Vite could also ignore generated route churn that it owns, but isolation is the stronger contract because build and check rewrite the files non-atomically.
