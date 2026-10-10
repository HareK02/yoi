// Official tools/web-ux capture bundles (console, request, accessibility, context).
// Does not build. Before always uses isolated saved site; after requires explicit parent readiness.
import { dirname, fromFileUrl, join } from "@std/path";
import type { RouteScenario } from "../../../../tools/web-ux/src/types.ts";
const root = join(dirname(fromFileUrl(import.meta.url)), "../../../..");
const stage = Deno.args[0];
if (stage !== "before" && stage !== "after") {
  throw new Error("usage: capture-visual.ts before|after");
}
const output = join(root, "web/workspace/.svelte-kit/t724/visual");
await Deno.mkdir(output, { recursive: true });
const runSuffix = Deno.args[1] ? `-${Deno.args[1]}` : "";
for (const scheme of ["light", "dark"]) {
  const runId = `t724-${stage}-${scheme}${runSuffix}`;
  const routes = ["owner", "member", "empty", "error", "long"].map((
    state,
  ): RouteScenario => ({
    id: `home-${state}`,
    label: `Home/sidebar Workers ${state}`,
    path: `/w/home-${state}`,
    goal:
      "Compare unchanged Home/sidebar/Workers composition under identical API fixture conditions.",
    dataState: state === "error"
      ? "Intentional 503 for Home merge-requests; visible scoped error expected."
      : `Shared Home ${state}; representative authoritative Workers (empty for new workspace).`,
    ready: {
      kind: "selector",
      selector:
        ".workspace-home:has([data-feed-ready='home-active']):has([data-feed-ready='home-recent'])",
    },
    capturePoints: [{
      id: "initial",
      label: "Home and Workers sidebar",
      fullPage: true,
    }],
  }));
  for (const state of ["owner", "member", "empty", "error", "long"]) {
    routes.push({
      id: `workers-${state}`,
      label: `Workers ${state}`,
      path: state === "error" ? "/w/workers-error/workers" : `/w/home-${state}/workers`,
      goal: "Compare the existing Worker management surface with identical authoritative catalogs.",
      dataState:
        `Shared Home ${state} identity; representative Worker list (empty or 503 when requested). Owner controls follow workspace permission projection.`,
      ready: {
        kind: "selector",
        selector: state === "empty"
          ? ".workers-page:has-text('No Workers are visible.')"
          : state === "error"
          ? ".workers-page .section-state.error"
          : ".workers-page .workers-table tbody tr",
      },
      capturePoints: [{ id: "initial", label: "Workers", fullPage: true }],
    });
  }
  if (stage === "after") {
    routes.push({
      id: "drive-form-observation",
      label: "Pinned rename and replace form observations",
      path: "/w/home-owner/drive/3",
      goal: "Show the form's locked read observation and explicit destination/file controls.",
      dataState:
        "Initial committed request read from fixture metadata; no mutation is sent by these capture interactions.",
      ready: { kind: "selector", selector: '.drive-page[data-drive-ready="true"]' },
      capturePoints: [
        {
          id: "rename",
          label: "Rename / move pinned observation",
          fullPage: true,
          interaction: [
            { action: "click", selector: '.drive-menu > summary[aria-label="More actions"]' },
            { action: "click", selector: '.drive-toolbar button:has-text("Rename / move")' },
            {
              action: "wait",
              ready: { kind: "selector", selector: '.drive-form:has-text("Changes by another writer will not be overwritten.")' },
            },
            { action: "press", selector: ".drive-form input:not([inputmode])", key: "Shift" },
          ],
        },
        {
          id: "replace",
          label: "Replace file pinned observation",
          fullPage: true,
          interaction: [
            { action: "click", selector: '.drive-form button:has-text("Cancel form")' },
            { action: "click", selector: '.drive-menu > summary[aria-label="More actions"]' },
            { action: "click", selector: '.drive-toolbar button:has-text("Replace file")' },
            {
              action: "wait",
              ready: { kind: "selector", selector: '.drive-form:has-text("Changes by another writer will not be overwritten.")' },
            },
            { action: "press", selector: ".drive-form input[type=file]", key: "Shift" },
          ],
        },
      ],
    });
    routes.push({
      id: "worker-grants-owner",
      label: "Owner Worker Drive grant lifecycle",
      path: "/w/home-owner/workers",
      goal:
        "Use the authoritative Worker catalog to grant read-only/read-write access and revoke it without conflating Profile enablement.",
      dataState:
        "Representative REST catalog and matching protocol snapshot; in-memory grant API, not actual Backend.",
      ready: { kind: "selector", selector: ".drive-grants select" },
      capturePoints: [
        {
          id: "selected",
          label: "Selected Worker without grants",
          fullPage: true,
          interaction: [
            {
              action: "press",
              selector: ".drive-grants .worker-select select",
              key: "ArrowDown",
            },
            {
              action: "press",
              selector: ".drive-grants .worker-select select",
              key: "Enter",
            },
            {
              action: "wait",
              ready: {
                kind: "selector",
                selector: '.drive-grants button:has-text("Add Drive grant")',
              },
            },
          ],
        },
        {
          id: "read-only",
          label: "Read-only grant",
          fullPage: true,
          interaction: [
            {
              action: "click",
              selector: '.drive-grants button:has-text("Add Drive grant")',
            },
            { action: "click", selector: ".drive-grants button[type=submit]" },
            {
              action: "wait",
              ready: {
                kind: "selector",
                selector: '.drive-grants strong:has-text("Read only")',
              },
            },
          ],
        },
        {
          id: "read-write",
          label: "Read-write grant after explicit revoke",
          fullPage: true,
          interaction: [
            {
              action: "click",
              selector: '.drive-grants button[aria-label^="Revoke Drive grant"]',
            },
            {
              action: "wait",
              ready: {
                kind: "selector",
                selector: '.drive-grants:has-text("No active Drive grants for this Worker.")',
              },
            },
            {
              action: "click",
              selector: '.drive-grants button:has-text("Add Drive grant")',
            },
            {
              action: "press",
              selector: ".drive-grants form select",
              key: "ArrowDown",
            },
            {
              action: "press",
              selector: ".drive-grants form select",
              key: "Enter",
            },
            { action: "click", selector: ".drive-grants button[type=submit]" },
            {
              action: "wait",
              ready: {
                kind: "selector",
                selector: '.drive-grants strong:has-text("Read and write")',
              },
            },
          ],
        },
        {
          id: "revoked",
          label: "Grant revoked",
          fullPage: true,
          interaction: [
            {
              action: "click",
              selector: '.drive-grants button[aria-label^="Revoke Drive grant"]',
            },
            {
              action: "wait",
              ready: {
                kind: "selector",
                selector: '.drive-grants:has-text("No active Drive grants for this Worker.")',
              },
            },
          ],
        },
      ],
    });
    for (
      const [state, workspace, nodeId] of [
        ["owner", "home-owner", ""],
        ["member", "home-member", ""],
        ["empty", "home-empty", ""],
        ["error", "home-error", ""],
        ["denied", "drive-denied", ""],
        ["paged", "drive-paged", ""],
        ["markdown", "home-owner", "/3"],
        ["image", "home-owner", "/5"],
        ["image-error", "home-owner", "/7"],
        ["truncated", "home-owner", "/9"],
      ]
    ) {
      routes.push({
        id: `drive-${state}`,
        label: `Drive ${state}`,
        path: `/w/${workspace}/drive${nodeId}`,
        goal:
          "Review real production Drive layout, bounded safe preview and distinct failure states.",
        dataState: `In-memory Drive API ${state}, explicitly NOT actual Backend.`,
        ready: {
          kind: "selector",
          selector: state === "image-error"
            ? "main .drive-page [role=alert]"
            : 'main [data-drive-ready="true"]',
        },
        capturePoints: state === "member"
          ? [
            {
              id: "initial",
              label: "Member before authority denial",
              fullPage: true,
            },
            {
              id: "denied-write",
              label: "Member after typed write denial",
              fullPage: true,
              interaction: [
                { action: "click", selector: '.drive-create-menu > summary' },
                {
                  action: "click",
                  selector: '.drive-page button:has-text("New folder")',
                },
                {
                  action: "fill",
                  selector: ".drive-form input:not([type=file])",
                  value: "member-denied-folder",
                },
                {
                  action: "click",
                  selector: ".drive-form button[type=submit]",
                },
                {
                  action: "wait",
                  ready: {
                    kind: "selector",
                    selector: '.drive-dialog:has-text("Read only / access denied by Backend")',
                  },
                },
                {
                  action: "press",
                  selector: '.drive-dialog button[aria-label="Close dialog"]',
                  key: "Shift",
                },
              ],
            },
          ]
          : [{ id: "initial", label: `Drive ${state}`, fullPage: true }],
      });
    }
  }
  const port = scheme === "light" ? 17241 : 17242;
  const scenario = {
    schemaVersion: 1,
    id: runId,
    title: `T-724 fixture-only ${stage} ${scheme}`,
    baseUrl: `http://127.0.0.1:${port}`,
    colorScheme: scheme,
    reducedMotion: "reduce",
    personas: [{
      id: "fixture",
      label: "Fixture account; Backend not connected",
      auth: { kind: "anonymous" },
    }],
    viewports: [1440, 768, 390, 320].map((width) => ({
      label: String(width),
      width,
      height: 900,
    })),
    routes,
    processes: [{
      id: "drive-fixture",
      command: Deno.execPath(),
      args: [
        "run",
        "--allow-net",
        "--allow-read",
        "--allow-run",
        "--allow-env",
        join(root, "tools/web-ux/drive-fixture/server.ts"),
        String(port),
        Deno.args[2]
          ? join(root, Deno.args[2])
          : stage === "before"
          ? join(root, "web/workspace/.svelte-kit/t724/before-build/site")
          : join(root, "web/workspace/build"),
      ],
      readyUrl: `http://127.0.0.1:${port}/health`,
    }],
  };
  const path = join(output, `${stage}-${scheme}.json`);
  await Deno.writeTextFile(path, JSON.stringify(scenario, null, 2));
  const command = new Deno.Command(Deno.execPath(), {
    cwd: root,
    args: [
      "run",
      "--config",
      "tools/web-ux/deno.json",
      "--allow-env",
      "--allow-net",
      "--allow-read",
      "--allow-write",
      "--allow-run",
      "--allow-sys",
      "tools/web-ux/cli.ts",
      "capture",
      "--scenario",
      path,
      "--output",
      output,
      "--run-id",
      runId,
    ],
    stdout: "inherit",
    stderr: "inherit",
  });
  const result = await command.output();
  if (!result.success && result.code !== 2) {
    throw new Error(
      `Official ${stage}/${scheme} capture failed (${result.code}); inspect review-context.`,
    );
  }
  // CLI exit 2 retains intentional HTTP error-state diagnostics rather than hiding them.
  // Parent must inspect the contexts; report unexpected errors separately.
  const context = JSON.parse(
    await Deno.readTextFile(join(output, runId, "review-context.json")),
  );
  const unexpected = context.captures.flatMap((
    capture: {
      route: { id: string };
      errors: { kind: string; message: string; url?: string }[];
    },
  ) =>
    capture.errors.filter((error) => {
      // Selection/read-text republishing entry metadata supersedes the in-flight
      // breadcrumb lookup. Its abort is intentional and must not become a UI error.
      if (
        [
          "drive-markdown",
          "drive-form-observation",
          "drive-image",
          "drive-image-error",
          "drive-truncated",
        ].includes(capture.route.id) && error.kind === "request" &&
        error.message === "net::ERR_ABORTED" &&
        error.url?.includes("/home-owner/drive/metadata?")
      ) return false;
      if (
        ["drive-member", "drive-denied"].includes(capture.route.id) &&
        error.kind === "document" &&
        error.message ===
          "visible UI error: Drive request failed: denied. No empty content or success is inferred from this failure."
      ) return false;
      if (
        capture.route.id === "drive-error" && error.kind === "document" &&
        error.message ===
          "visible UI error: Drive request failed: storage_unavailable. No empty content or success is inferred from this failure."
      ) return false;
      if (capture.route.id === "home-error") {
        return !(error.kind === "console" && error.message.includes("503")) &&
          !error.url?.includes("/home-error/merge-requests?");
      }
      if (capture.route.id === "workers-error") {
        return !(error.kind === "console" && error.message.includes("503")) &&
          !(error.url?.includes("/workers-error/workers") &&
            /HTTP 503|net::ERR_ABORTED/.test(error.message));
      }
      if (capture.route.id === "drive-member") {
        return !(error.kind === "console" && error.message.includes("403")) &&
          !(error.url?.includes("/home-member/drive/mutate") &&
            /HTTP 403|net::ERR_ABORTED/.test(error.message));
      }
      if (
        capture.route.id === "drive-error" ||
        capture.route.id === "drive-denied"
      ) {
        return !(error.kind === "console" && /403|503/.test(error.message)) &&
          !error.url?.includes("/drive/");
      }
      return true;
    }).map((error) => ({ route: capture.route.id, ...error }))
  );
  await Deno.writeTextFile(
    join(output, runId, "diagnostic-disposition.json"),
    JSON.stringify(
      {
        expected:
          "Home reviews/Worker catalog error: injected 503. Drive denied/member/error: injected typed 403/503 and their precise visible failure messages. File metadata ERR_ABORTED is a superseded breadcrumb read, fenced by the UI epoch with no visible breadcrumb failure. All retained in review-context; no diagnostics are removed from official evidence.",
        unexpected,
      },
      null,
      2,
    ),
  );
  if (unexpected.length) {
    throw new Error(
      `${unexpected.length} unexpected visual diagnostics; inspect disposition.json`,
    );
  }
}
