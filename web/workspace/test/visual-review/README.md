# Optional initial repository validation (T-717 Web portion)

These scenarios use the production static Workspace build and the existing
`tools/web-ux` capture/process harness. The loopback proxy only extends the shared
Home fixture's API responses for empty Worker lists and launch options; it does
not replace the application shell, styles, or components. No real Workspace,
account, repository, Runtime, or Worker is created.

## Run

From `web/workspace`, with the repository's pinned Chromium available:

```sh
deno task build
deno task test:browser:empty-workspace
```

The browser test covers light/dark at 1440, 768, 390, and 320px: optional Git
native validation, opting out after entering repository values, null creation
response/navigation, zero-repository catalog reopening, normal embedded Worker
submission without Workdir or Ticket assignment, and remote Runtime Workdir
requirements. It uses dynamically allocated loopback ports and the existing
owned-process cleanup harness. Screenshots and logs are ignored artifacts under
`.svelte-kit/t717/browser`.

From `tools/web-ux`:

```sh
deno task web-ux capture \
  --scenario ../../web/workspace/test/visual-review/optional-repository.json \
  --output ../../web/workspace/.svelte-kit/t717/visual --run-id empty-workspace

deno task web-ux capture \
  --scenario ../../web/workspace/test/visual-review/optional-repository-interactions.json \
  --output ../../web/workspace/.svelte-kit/t717/visual --run-id creation-interactions
```

The interaction scenario intentionally rejects creation with HTTP400. Capture
exit status 2 and the recorded request/console/visible validation errors are
expected for `validation-error`, not evidence of a successful creation. No other
capture error is expected. To repeat dark captures, use a scenario copy with
`colorScheme: "dark"`, preserving the process cwd paths.

## Implementer-owned visual review

- Baseline commit: `e939cd4e4b1f6b63b81411fd7766062f2d69ad0d`.
- Before: `.svelte-kit/t717/visual/before-final-light` and `before-dark`.
  These use an isolated build under `.svelte-kit/t717/baseline-source`, with the
  changed production form, catalog API, model, and CSS restored from baseline
  Git source. The original pre-edit light capture is also retained as
  `.svelte-kit/t717/visual/before-valid`.
- After: `.svelte-kit/t717/visual/final-light`, `final-dark`,
  `final-interactions-light`, and `final-interactions-dark`.
- Comparisons: `.svelte-kit/t717/visual/final-comparison-light` and
  `final-comparison-dark`.
- Personas: synthetic owner and non-owner permission projections; catalog
  creation/reopening uses the same synthetic account.
- Viewports: 1440, 768, 390px; additional browser screenshots/checks at 320px.
- Themes: light and dark.
- States: zero repositories, empty Home/Sidebar, representative member work
  records, long catalog names, unchecked/checked optional repository form,
  populated inputs, and intentional Backend validation failure.
- Reviewed by the implementer using `ViewImage`: before/after contact sheets,
  individual narrow/mobile populated-form screenshots, and the final mobile
  dark validation-error screenshot. Reviewed context bundles and accessibility
  snapshots for document status, control names/state, screenshot hashes,
  console/page/request errors, and capture points.
- Fixed: catalog/form clipping at narrow content widths; repository field
  disclosure/required-state handling. Chromium tests also found and fixed the
  existing repository-key HTML pattern's invalid modern-regexp character class.
- Result: pass. All baseline and final main captures have document status 200
  and no retained browser/UI errors. Only the intentional HTTP400 interaction
  errors remain. Home, member Sidebar, and Worker launch comparisons show zero
  changed pixels; catalog changes are confined to the intended surface.

## Validation and limitations

- `deno task check`: 0 errors, 0 warnings.
- Targeted model/catalog Deno tests: 18 passed.
- `deno task test:component`: 29 files, 268 tests passed.
- `deno task test:browser:empty-workspace`: passed (8 theme/width combinations).
- `deno task build`: passed.
- Web-scoped `git diff --check HEAD -- .`: passed.
- `deno task test`: 522 passed, 1 pre-existing failure in
  `src/lib/workspace/console/worker-console.ui.test.ts`,
  “workspace Tickets surface provides Kanban and lifecycle controls”. It fails
  the unchanged Ticket detail source assertion at line 429. The same failure
  was reproduced against the isolated baseline source. Evidence:
  `.svelte-kit/t717/unit.log` and `baseline-ticket-failure.log`.
- Canonical OpenAPI and generated TypeScript were regenerated together.
  The Web parser uses the generated nullable creation-response type directly.
  Backend persistence/idempotency is covered separately by Store/catalog and
  real HTTP boundary tests in `yoi-workspace-server`.
