# T-724 browser fixture validation

These tests serve the real static production Web shell against a loopback, in-memory API fixture. They are **not actual Backend/DB/LocalFileSystem authorization or round-trip evidence**. The parent owns the separate real API checks.

## Runtime prerequisites

Deno and Playwright Chromium must be available. The server binds loopback only and owns/cleans up its shared Home fixture subprocess. Tests use an ephemeral port and independent fixture state; no real workspace data, credentials, git operations, environment test switches, or UI/CSS fixture branches are used.

The parent must confirm UI readiness before the production build and browser run. Tests intentionally never build automatically.

```sh
# Narrow API-fixture compatibility checks (safe before UI readiness)
deno test --config web/workspace/test/drive-browser/deno.json --allow-read web/workspace/test/drive-browser/fixture-api.test.ts

# After explicit parent UI-ready signal and successful production build only:
deno test --config web/workspace/test/drive-browser/deno.json --allow-env --allow-net --allow-read --allow-write --allow-run --allow-sys web/workspace/test/drive-browser/drive.browser.test.ts

# Responsive audit, with optional frozen production site and artifact suffix:
deno test --config web/workspace/test/drive-browser/deno.json --allow-env --allow-net --allow-read --allow-write --allow-run --allow-sys web/workspace/test/drive-browser/responsive.browser.test.ts -- web/workspace/.svelte-kit/t724/verified-build/site final-reviewed

# Official tools/web-ux bundles; no build is performed by this runner.
deno run --config web/workspace/test/drive-browser/deno.json --allow-env --allow-net --allow-read --allow-write --allow-run --allow-sys web/workspace/test/drive-browser/capture-visual.ts before <unique-run-suffix>
deno run --config web/workspace/test/drive-browser/deno.json --allow-env --allow-net --allow-read --allow-write --allow-run --allow-sys web/workspace/test/drive-browser/capture-visual.ts after <unique-run-suffix> [workdir-relative-production-site]
```

## Fixture API

Start `tools/web-ux/drive-fixture/server.ts <port> <build-root>` with `--allow-net --allow-read --allow-run --allow-env`. It extends the shared Home/settings API fixture through a loopback proxy and implements Drive root/list/metadata/search/read-text/read-chunk/download/mutate/upload/request-status/grants APIs. Response wire types derive from generated Drive DTOs; current client parsers are exercised by the fixture compatibility tests.

- Root ID `9007199254740993` tests lossless decimal references.
- `2`: folder 資料; `3`: Markdown README; `4`: text inside folder 2.
- `5`: valid synthetic 1200×800 `landscape.png` (16,813 compressed bytes), generated as real binary PNG data by `tools/web-ux/drive-fixture/raster.ts`; `6`: empty text; `7`: undecodable PNG; `8`: SVG download-only; `9`: text exceeding 64 KiB; `10`: long Japanese name; `11`: valid 64 KiB NUL text whose escaped JSON exceeds 256 KiB but stays below the separate 512 KiB JSON bound.
- `home-member` and `*readonly*`: reads succeed, mutations are typed HTTP 403. No metadata permission flag is invented. The UI reacts to a denied operation, not speculative permission inference.
- `*denied*`: typed HTTP 403 root/read; `*offline*`/`home-error`: typed HTTP 503; `*empty*`: empty successful root list; `*paged*`: 240 additional nodes.
- `POST /__fixture/fault` JSON `{workspace, mode}`: one-shot `conflict` or committed `unknown` mutation response; persistent `image_error` binary fetch failure.
- `home-owner`/`home-member`/`home-long`: representative authoritative Worker catalogs; REST and subscription projections come from the same generated-shape records. `home-long` includes long display names and opaque Runtime/Worker IDs. `workers-error` intentionally fails catalog reads with HTTP 503.
- `GET /__fixture/log`: observable API request IDs/methods/paths/node IDs/CAS observed committed requests for identity and no-resend assertions.
- `POST /__fixture/reset`: fresh in-memory state.

Request delay/loss tests intercept the API boundary with Playwright and synchronize on gates, not guessed sleeps. They let the fixture commit before dropping/holding the response, then verify receipt query without blind upload retry.

## Visual evidence

Preserved production baseline: `.svelte-kit/t724/before-build/{site,output,build.log}`. Official bundles live below `.svelte-kit/t724/visual/`. Each contains screenshots, console/request/page diagnostics, accessibility snapshots, screenshot hashes, source commit/dirty context, and `review-context.json`.

Home/sidebar and Workers are captured under identical owner/member/error/empty/long conditions, light/dark at 1440/768/390/320. After captures add Drive owner/member/empty/error/denied/paged/Markdown/image/image-error/truncated surfaces, pinned rename/replace observed committed request forms, and owner grant selection/read-only/read-write/revoked states. Expected injected 503/403 diagnostics and superseded breadcrumb metadata aborts remain in official context; `diagnostic-disposition.json` explicitly separates unexpected diagnostics. CLI `completed-with-errors` is not relabeled a visual pass.

The production shell scrolls `main` internally, so `fullPage` does not expand that scroll panel. Capture points use ordinary keyboard focus to show the actual form or the top denial state; no CSS/DOM screenshot override is applied. The responsive audit separately records document/main/table-region geometry, visible keyboard focus, and page errors across the full theme/viewport matrix. Raster checks wait for actual decoding, assert natural dimensions 1200×800, preserve the 3:2 aspect ratio, and constrain the rendered box by both the available width and 60vh height at all four widths/both themes.

The Drive browser suite also exercises the file-browser redesign at 1440/768/390/320px in light/dark themes: menu placement, native-dialog focus and Escape restoration, folder creation, search, and destination-folder navigation. Screenshots are written to `.svelte-kit/drive-redesign/` for the list, New menu, creation dialog, Markdown preview, and move dialog. Component tests cover failed/paged destination reads, exclusion of files and the moving folder itself, and pinned mutation observed committed requests. These checks use the same fixture boundary, not a live Backend.

Parent and reviewer must open images and inspect context before asserting visual acceptance. In particular, a successful capture command is not sufficient and HTTP 200 does not excuse visible UI errors.
