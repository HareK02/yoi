# Web UX inspection workbench

`tools/web-ux` is a development-only Playwright workbench for repeatable visual inspection of the
real Web Workspace. It does not add a Yoi product Skill, Flow, Runtime capability, or browser
automation route.

The workbench produces a **review context bundle** rather than treating a screenshot as evidence by
itself. Every capture records the persona, route, viewport, theme, intended user goal, expected data
state, sanitized document URL/status, console/page/request failures, screenshot hashes, an
accessibility snapshot, source revision, and browser version.

## Environment

Enter the repository dev shell. The shell supplies the Nix-pinned Chromium build and sets
`PLAYWRIGHT_BROWSERS_PATH`; Playwright does not download a browser at runtime.

```sh
nix develop
cd tools/web-ux
deno task check
deno task test
deno task test:browser
```

`test:browser` starts a deterministic fixture server owned by the test, captures distinct owner and
non-owner contexts, verifies the review bundle, and proves server/browser cleanup. It must run
inside `nix develop` so it uses the pinned browser.

The npm Playwright version in `deno.json` must match `pkgs.playwright-driver.version` in the pinned
Nixpkgs input. Update both as one toolchain change.

## Scenario contract

Scenarios are reviewed JSON files under `scenarios/`. A scenario fixes:

- personas and whether each uses an isolated anonymous context or a local Playwright storage-state
  file;
- explicit routes and user goals;
- expected data state, viewports, theme, locale, timezone, and reduced-motion mode;
- an explicit readiness condition for every route and optional interaction/capture-point conditions;
- selectors and exact environment-derived text that must be redacted;
- optional processes owned by the capture command, including an HTTP readiness URL.

`${UPPER_CASE_ENV}` values are expanded at runtime. URLs with embedded credentials are rejected.
Route readiness is bounded and retried twice; it never relies on a fixed sleep. `network-idle` is
available but should be used only for screens whose contract actually reaches idle. Prefer a stable
screen-owned selector.

`workspace-control-plane.json` expects:

```sh
export WEB_UX_BASE_URL='http://127.0.0.1:5173'
export WORKSPACE_ID='<workspace-id>'
export XDG_STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
```

## Authentication fixtures

Authentication state is local sensitive material stored under `$XDG_STATE_HOME/yoi/web-ux/auth/`,
outside the Repository and Workdir. Files are written with mode `0600`, state contents are never
copied into a review bundle, and the CLI never prints cookies or credentials. Each profile has a
sidecar binding it to the exact persona and base URL origin with a 12-hour default expiry. Capture
fails explicitly when metadata is missing, the origin differs, or the profile has expired; it never
silently reuses or refreshes that state.

For an interactive Passkey/browser login:

```sh
deno task web-ux auth \
  --scenario scenarios/workspace-control-plane.json \
  --persona owner
```

The command opens Chromium at the configured login route, waits up to five minutes for the
scenario's success URL, saves `storageState`, and closes the browser in `finally`. Repeat for
`non-owner` using a real account with that permission projection.

A test fixture may already provide Playwright-compatible `{ cookies, origins }` state. Import it
without putting its value on the command line:

```sh
deno task web-ux auth \
  --scenario scenarios/workspace-control-plane.json \
  --persona owner \
  --import-state /private/path/owner-state.json \
  --expires-in-hours 8
```

Delete both the profile and its metadata when it is no longer needed:

```sh
deno task web-ux auth \
  --scenario scenarios/workspace-control-plane.json \
  --persona owner \
  --delete
```

Do not place passwords, bearer tokens, private keys, WebAuthn material, or inline cookies in a
scenario, process arguments, a Repository URL, or `redact.text`. `redact.text` is only a final
defense for a secret already supplied through an environment-owned fixture; it is not a credential
transport.

## Capture and inspect

Capture a stable multi-persona bundle:

```sh
deno task web-ux capture \
  --scenario scenarios/workspace-control-plane.json \
  --output ../../target/web-ux \
  --run-id before-change
```

Use filters for a bounded feedback loop:

```sh
deno task web-ux capture \
  --scenario scenarios/workspace-control-plane.json \
  --output ../../target/web-ux \
  --run-id ticket-list-after \
  --personas owner,non-owner \
  --routes tickets \
  --viewports desktop
```

The command exits `2` when it produced evidence but observed UI/tool errors, and exits `1` when
capture itself failed. It continues other route/persona captures after a bounded route failure.
Inspect:

- `review-context.json` for the exact context, hashes, HTTP status, retained/truncated diagnostic
  counts, route and capture-point readiness, and the redacted interaction sequence;
- `contact-sheet.png` through its manifest `workdirPath` with an image-capable reviewer for
  composition, hierarchy, density, clipping, empty/error states, and permission-specific
  affordances;
- each `accessibility.md` through its manifest `workdirPath` for landmark/name/state evidence that a
  screenshot cannot prove;
- `process-logs/` when the scenario owns a server process. Each stdout/stderr stream is redacted,
  capped at 1 MiB, and paired with truncation metadata.

The implementing agent must inspect the actual contact sheet (for example with `ViewImage`), record
concrete findings, fix them, recapture under the same persona/route/viewport filters, and inspect
the new evidence. Playwright success alone is not visual acceptance.

## Compare before and after

```sh
deno task web-ux compare \
  --before ../../target/web-ux/before-change/review-context.json \
  --after ../../target/web-ux/after-change/review-context.json \
  --output ../../target/web-ux/before-vs-after
```

`comparison.html` and `comparison.png` show before, after, and pixel diff side by side.
`comparison.json` records changed-pixel counts, dimension mismatches, unmatched capture keys, and
diff hashes. Pixel differences are orientation evidence, not a correctness verdict; explain expected
animation/font/data changes and inspect the actual UI.

Capture keys are stable across runs: `persona / route / viewport / capture-point`. Keep those
identities unchanged when comparing the same user task.

## Process and artifact cleanup

The capture command owns only processes declared in its scenario. It starts them without a shell,
records bounded/redacted output, and terminates the process and descendants on success, capture
failure, or interruption observed by the command. It never stops an existing Yoi Server or Runtime
that it did not start.

Old complete review bundles can be removed without touching auth state or arbitrary directories:

```sh
deno task web-ux cleanup --output ../../target/web-ux --keep 5 --older-than-days 14 --dry-run
deno task web-ux cleanup --output ../../target/web-ux --keep 5 --older-than-days 14
```

Cleanup recognizes only directories containing `review-context.json`. The repository `target/` tree
is ignored by Git, while authentication state remains outside the repository. `capture` defaults to
`target/web-ux` when `--output` is omitted. Keep a bundle outside Git or publish it through the
approved immutable artifact channel when durable review evidence is required.

## Adding a scenario

1. Name the concrete user task and expected data state; do not write “looks correct”.
2. Use the smallest persona/route/viewport matrix that proves the intended contract, including
   owner/non-owner/anonymous boundaries when permissions affect composition.
3. Choose a screen-owned readiness selector or response. Avoid arbitrary sleeps.
4. Add capture points only for meaningful visual states (initial, expanded detail, error, empty, and
   so on).
5. Mark sensitive DOM regions with `[data-web-ux-redact]` or scenario selectors; never use review
   artifacts to transport secrets.
6. Run `deno task check`, `deno task test`, one real capture, and inspect `contact-sheet.png` plus
   `review-context.json`.
