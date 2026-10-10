# T-724 validation evidence

Editorial note (2026-10-10): obsolete state terminology is summarized by purpose below; cited IDs, commits and validation results still describe the original investigation, not new executions.

Scope: `T-724`, authoritative item `00001M4D5ZN6P:4`, resume instruction thread
sequence 7. Baseline: `c79506b60f1abcb3d8698d01491f2b6e644170d0` (clean detached
checkout). Source branch: `work/T-724-workspace-drive`. T-717/T-722 are integrated;
T-723 Tools/WIP and T-714 implementation are not modified.

## Requirements → observable proof

| Requirement | Implementation / proof |
| --- | --- |
| Shared Workspace asset, no Repository/Git prerequisite | Workspace sidebar Drive; optional-node route; real `/api/workspaces` repositoryless bootstrap, zero repository records/config, folder/Markdown/PNG/empty-file and Worker artifact roundtrip. |
| DB hierarchy, stable string node URL | `driveHref`, strict decimal/ref/metadata parsers; root/list/parent-chain APIs only; rename/move/delete/recreate lifecycle in real adapter and production browser. No blob path or second persisted tree. |
| Paged folder list, metadata and mutations | Service cursors, bounded pages, breadcrumb, search, creation, native binary download, replace, relocate, empty-folder delete; real and fixture roundtrips. |
| CAS and draft retention | Per-Workspace/node draft with observed node state at read time; 2-account real concurrent CAS (one success/one conflict), 2-tab browser drafts, explicit refresh vs discard; component regression proves open rename form does not silently rebase on refresh. |
| Identity/content-condition fences | Controller selection/write/page epochs; saved tuple, scoped receipts and separate cancel generation; delayed old read/save/status/workspace browser gates; keyed image decode events and URL revocation component regression. |
| Safe Markdown/raster/binary | Existing raw-HTML-rejecting renderer; same-Workspace explicit resource navigation and credential-free external HTTP(S), no automatic inline image requests; real node-content-bound PNG transfer, 256 KiB raster bound, HTML/SVG download-only, failed image/truncated/empty browser states. |
| Upload publication / errors / OutcomeUnknown | Raw Blob ≤16 MiB + SHA-256; transfer receipt vs DB-published acknowledgment; cancellation/loss preserve request ID and query current-authorized result, no unconditional reupload; typed name/CAS/limit/denied/storage failures. |
| Backend current permission and Worker grants | Existing authoritative Worker catalog; owner setting flag; explicit read_only/read_write, separate Profile enablement, complete paged grants, revoke; real signed read-only Worker denial/revoke and exact Web grant-adapter owner/member HTTP boundaries; component and browser grant interactions. |
| Latest only / no added workflow | Explicit replacement/history warning; no versions, approval/publication workflow, public share links or recursive folder delete. |

## Real API/DB/LocalFileSystem evidence (not fixture responses)

`crates/workspace-server/src/server/tests/drive_web_roundtrip_tests.rs` creates a
fresh temporary SQLite/FeatureStorage/LocalFileSystem Workspace over real loopback
HTTP. It starts no live dogfood Server or Runtime; the Runtime Worker-spawn process
boundary is fake, while Workspace/Worker registry/grants/nodes/bytes/receipts and
request authentication are production authorities.

The explicit Deno child imports the **production** content and grant adapters.
Fresh credentials arrive via stdin; origin/auth resolution is loopback-bound and
responses are never mocked. This proves authenticated HTTP adapters, **not** a
Passkey/cookie browser login or complete live Runtime process E2E.

- Ordinary real HTTP test: **1 passed / 1 explicitly ignored** process test.
- Explicit process test (`--ignored`): **1 passed**, including owner grant
  read_only/read_write create/list/revoke/re-read and non-owner administration
  rejection after parent integration.
- Full changed Server crate, replayed by the parent before commit: **693 library +
  11 binary passed**, 1 explicitly ignored; doc-tests passed
  (`target/t724-server-final.log`).
- Parent real adapter replay: `target/t724-real-adapter-final.log` (**1 passed**).
  Ordinary HTTP coverage is also part of the full Server crate run.

Reproduce using `web/workspace/test/drive-real-api/README.md` or
`deno task test:drive:real-api` from `web/workspace`.

## Compile, generated contracts and Web semantic validation

- Workspace root `cargo check`: pass (`target/t724-root-check-final.log`).
- `cargo test -p server-api --features typescript`: **91 passed**, doc-tests pass.
- Canonical OpenAPI `export_openapi -- --check openapi/server-api.json`: current.
- Regenerated Drive TypeScript, Deno-formatted to the checked-in style: byte equal
  to `web/workspace/src/lib/generated/drive-api.ts`; no generated contract edits.
- `generate_repository_openapi_types`, `generate_runtime_api_types`,
  `generate_ticket_api_types`, `generate_companion_api_types`,
  `generate_worker_launch_api_types` with `--check`: all current.
- `cargo fmt --all -- --check`, Deno format/lint of new typed boundaries, and
  `git diff --check HEAD`: pass.
- Web `deno task check`: **0 errors / 0 warnings**.
- Web `deno task test`: **566 passed**, including 36 content/controller boundary
  tests and 2 preview/link-policy tests.
- Web `deno task test:component`: **302 passed / 32 files**, including actual
  Markdown rendering/alt labels, late decode fences, pinned observed form condition, grant
  administration, and dependent existing Worker/Console/Markdown surfaces.
- Separate fixture-client compatibility tests: **4 passed**. These prove mock
  wire compatibility, not real storage or authorization.

Web logs: `.svelte-kit/t724-{web-check-final,unit-final,components-final}.log`,
plus narrow `.svelte-kit/t724-{drive-state,ui-fence,markdown-tests}.log`.

## Production browser and visual review

Production rebuild and implementer-owned visual acceptance completed on
2026-10-09 before the implementation commit:

- `deno task build`: pass (`.svelte-kit/t724/final-build.log`).
- Production browser scenarios: **9 passed** (`.svelte-kit/t724/browser-final.log`).
- Responsive/keyboard/raster audit: **1 passed**, all 1440/768/390/320 widths in
  both themes (`.svelte-kit/t724/responsive-final.{log,json}`).
- Official after bundles: `.svelte-kit/t724/visual/t724-after-{light,dark}-committed-candidate/`,
  **108 captures per theme**. The same baseline Home/Workers personas and widths
  are compared, with additional Drive/grant states.
- Parent opened before and after screenshot overview sheets through `ViewImage`
  (`target/t724-final-visual/{before,after}-{light,dark}-*.png`), plus original
  320px grant and 390px denied-write captures. Reviewed grouping, separators,
  spacing, focus, wrapping, page vs table scroll, and permission/recovery copy.
  Home/Workers composition is preserved except the intended shared Drive entry,
  grant section and contained Worker-table scrolling. New states remain distinct
  and narrow layouts retain accessible controls without document overflow.
- Parent inspected official `review-context.json`: HTTP document status 200,
  accessibility snapshots and screenshot hashes present; diagnostic dispositions
  retain intentional 403/503 fixtures and superseded metadata aborts. Both
  `diagnostic-disposition.json` files have **no unexpected diagnostics**. The
  runner correctly reports `completed-with-errors` for injected failures; visual
  acceptance is the implementer's review, not that exit/status label.
- Visual result: **pass**, no unresolved new visual regression. Artifacts use
  baseline source commit plus dirty context; production source is frozen by
  the subsequent commit (only test formatting/documentation changed afterward).

A final grant regression also proves catalog readiness toggles cannot abandon an
already transmitted mutation. Readiness gates new actions but is not Worker
identity. All **22 grant component tests** pass; pending/unknown operations remain
fenced until their result is reconciled. Worst-case 64 KiB NUL text additionally
proves the separate 512 KiB JSON response bound without relaxing the raster bound.
These changes are included in the final Web check/test/build/captures above.

The production static Web shell is exercised against synthetic loopback APIs;
this is separate from the real authority tests above. The fixture has a single
Worker catalog reused by REST/subscription projections, not a second UI registry.
See `web/workspace/test/drive-browser/README.md` for reproducible scenarios, gates,
owned-process cleanup and the browser/visual commands.

Before sources are preserved under `.svelte-kit/t724/before-build/`; baseline JS
has no Drive/grant controls. Official before bundles use
`.svelte-kit/t724/visual/t724-before-{light,dark}-representative/`. The current
implementation was never overwritten to produce the final static before build.

Intermediate visual review found and fixed:
- Narrow Worker table widening the page grid and clipping grant controls: contain
  horizontal table scrolling and constrain the page grid; full identities remain
  available in a disclosure.
- Markdown image placeholders losing alternate text: use the renderer library's
  actual `text` prop; retain no automatic image loading. Component and production
  browser regressions protect this.
- Parent semantic review additionally fixed late image-decode event identity and
  open rename/replacement observed-condition pinning, with focused component tests.

Injected permission/storage/list errors and superseded aborted breadcrumb reads
remain in official context with explicit dispositions. A successful capture exit
or HTTP200 alone is not visual acceptance. No screenshots, credential state or
real response secrets are committed.
