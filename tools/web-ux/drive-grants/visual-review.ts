// Development-only visual evidence over the real Workers route and loopback
// HTTP fixtures. No Backend/Runtime/account/grant state is mutated.
// Run from root: deno run --config tools/web-ux/deno.json -A tools/web-ux/drive-grants/visual-review.ts after
import { chromium } from "npm:playwright@1.59.1";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";
const root = new URL("../../../", import.meta.url).pathname;
const run = Deno.args[0] ?? "after";
const before = run === "before";
const out = `${root}target/web-ux/t724-drive-grants/${run}`;
const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
const port = (listener.addr as Deno.NetAddr).port;
listener.close();
const baseUrl = `http://127.0.0.1:${port}`;
const processes = await startOwnedProcesses(
  [{
    id: "vite",
    command: "deno",
    args: ["task", "dev", "--host", "127.0.0.1", "--port", String(port)],
    cwd: `${root}web/workspace`,
    readyUrl: baseUrl,
  }],
  import.meta.url,
  `${out}/logs`,
  [],
);
const worker = {
  runtime_id: "runtime-a",
  worker_id: "worker-a",
  resource_key: "W-1",
  host_id: "host-a",
  display_name: "Documentation Worker",
  label: "Worker",
  profile: "builtin:coder",
  tags: [],
  workspace: {
    visibility: "workspace",
    identity: "space",
    workspace_id: "space",
  },
  availability: "observed",
  state: "stopped",
  pinned: false,
  retention_state: "normal",
  implementation: { kind: "runtime", display_hint: "Runtime Worker" },
  workdir_attachments: [],
  diagnostics: [],
  restore_observation_token: "observation-1",
};
const grant = {
  grant_id: "1",
  workspace_id: "space",
  runtime_id: "runtime-a",
  worker_id: "worker-a",
  access: "read_only",
  revoked: false,
  created_by: "fixture-owner",
  created_at: "2026-10-09",
  revoked_by: null,
  revoked_at: null,
};
const browser = await chromium.launch({ headless: true });
const longWorker = {
  ...worker,
  runtime_id: `runtime-${"r".repeat(140)}`,
  worker_id: `worker-${"w".repeat(140)}`,
  display_name:
    "Documentation and distributed development Worker with a long display name for international documentation review",
};
const captures: unknown[] = [];
try {
  for (const persona of ["owner", "non-owner"]) {
    for (const theme of ["light", "dark"] as const) {
      for (const width of [1440, 768, 390]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme: theme,
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          const errors: string[] = [];
          page.on("pageerror", (error) => errors.push(error.message));
          page.on("console", (message) => {
            if (message.type() === "error") errors.push(message.text());
          });
          page.on("requestfailed", (request) =>
            errors.push(
              `${request.method()} ${
                new URL(request.url()).pathname
              }: ${request.failure()?.errorText}`,
            ));
          let state = "active";
          await page.routeWebSocket("**/api/w/space/protocol/ws", () => {});
          await page.route("**/api/**", async (route) => {
            const url = new URL(route.request().url()), path = url.pathname;
            if (!path.startsWith("/api/")) {
              return await route.continue();
            }
            const owner = persona === "owner";
            let body: unknown;
            if (path === "/api/workspaces") {
              body = [{
                workspace_id: "space",
                display_name: "Drive review",
                owner_account_id: "fixture-owner",
                state: "active",
                created_at: "2026-01-01",
                updated_at: "2026-01-01",
              }];
            } else if (path.endsWith("/workspace")) {
              body = {
                workspace_id: "space",
                display_name: "Drive review",
                record_authority: "fixture",
                schema_version: 1,
                auth: {
                  Passkey: {
                    rp_id: "127.0.0.1",
                    origin: baseUrl,
                    public_base_url: baseUrl,
                    cookie_name: "fixture",
                  },
                },
                permissions: {
                  manage_repositories: owner,
                  manage_secrets: owner,
                  manage_runtimes: owner,
                  delete_workspace: owner,
                },
                extension_points: {
                  store: "fixture",
                  event_stream: {
                    status: "ready",
                    note: "fixture",
                    diagnostics: [],
                  },
                  host_worker_bridge: {
                    status: "ready",
                    note: "fixture",
                    diagnostics: [],
                  },
                  companion_console: {
                    status: "ready",
                    note: "fixture",
                    diagnostics: [],
                  },
                },
              };
            } else if (path.endsWith("/workers")) {
              body = {
                workspace_id: "space",
                limit: 100,
                source: "fixture",
                diagnostics: [],
                items: [state === "long" ? longWorker : worker],
              };
            } else if (path.endsWith("/repositories")) {
              body = {
                workspace_id: "space",
                items: [],
                source: "fixture",
                diagnostics: [],
              };
            } else if (path.endsWith("/cleanup-plan")) {
              body = {
                workspace_id: "space",
                generated_at: "2026-10-09",
                runtime_id: "runtime-a",
                revision: "plan-1",
                digest: "digest-1",
                diagnostics: [],
                workdirs: [],
                workers: [],
              };
            } else if (path.endsWith("/drive/grants")) {
              if (state === "loading") {
                await new Promise<void>((resolve) => {
                  release = resolve;
                });
              }
              if (state === "error") {
                await route.fulfill({
                  status: 503,
                  json: {
                    code: "storage_unavailable",
                    classification: "not_committed",
                    message: `Drive storage unavailable. ${
                      "A long diagnostic requiring readable wrapping and an explicit refresh action. "
                        .repeat(12)
                    }`,
                  },
                });
                return;
              }
              body = {
                grants: state === "empty" ? [] : [{
                  ...grant,
                  runtime_id: state === "long" ? longWorker.runtime_id : worker.runtime_id,
                  worker_id: state === "long" ? longWorker.worker_id : worker.worker_id,
                }],
                next_after: null,
              };
            } else if (path.endsWith("/working-directories")) {
              body = { workspace_id: "space", items: [], diagnostics: [] };
            } else {
              errors.push(`Unmatched fixture ${path}`);
              body = {};
            }
            await route.fulfill({ json: body });
          });
          let release: (() => void) | undefined;
          const response = await page.goto(`${baseUrl}/w/space/workers`);
          await page.getByRole("heading", { name: "Workers", exact: true })
            .waitFor().catch(async (cause) => {
              console.log(errors, await page.locator("body").innerText());
              throw cause;
            });
          await page.getByText("Documentation Worker", { exact: true }).first()
            .waitFor().catch(async (cause) => {
              console.log(errors, await page.locator("body").innerText());
              throw cause;
            });
          async function capture(name: string) {
            await Deno.mkdir(out, { recursive: true });
            const filename = `${persona}-${theme}-${width}-${name}.png`;
            const screenshot = await page.screenshot({
              path: `${out}/${filename}`,
              fullPage: true,
            });
            if (!before && persona === "owner") {
              const clipped = await page.locator(".drive-grants").evaluate((
                element,
              ) =>
                [...element.querySelectorAll("button, select, summary")].some(
                  (control) => {
                    const box = control.getBoundingClientRect();
                    return box.left < 0 ||
                      box.right > element.ownerDocument.defaultView!.innerWidth;
                  },
                )
              );
              if (clipped) {
                throw new Error(
                  `Grant control clipped: ${persona} ${theme} ${width} ${name}`,
                );
              }
              await page.locator(".drive-grants").screenshot({
                path: `${out}/grants-${filename}`,
              });
            }
            captures.push({
              persona,
              theme,
              width,
              state: name,
              path: filename,
              sha256: await sha256(screenshot),
              documentStatus: response?.status(),
              errors: [...errors],
              visibleErrors: await page.locator("[role=alert]")
                .allTextContents(),
              overflow: await page.evaluate("document.documentElement.scrollWidth > innerWidth"),
              accessibility: await page.locator("main").ariaSnapshot(),
            });
          }
          if (before || !ownerPersona(persona)) await capture("workers");
          else {
            await page.getByRole("combobox", { name: "Worker", exact: true })
              .selectOption(JSON.stringify(["runtime-a", "worker-a"]));
            await page.getByRole("button", { name: "Revoke Drive grant 1" })
              .waitFor();
            await capture("active");
            state = "empty";
            await page.getByRole("button", { name: "Refresh grants" }).click();
            await page.getByText("No active Drive grants for this Worker.")
              .waitFor();
            await page.getByRole("button", { name: "Add Drive grant" }).click();
            await capture("empty-form");
            state = "error";
            await page.getByRole("button", { name: "Refresh grants" }).click();
            await page.getByRole("alert").waitFor();
            await capture("error");
            state = "loading";
            await page.getByRole("button", { name: "Refresh grants" }).click();
            await page.getByText("Loading all grant pages…").waitFor();
            await capture("loading");
            release?.();
          }
          state = "long";
          await page.reload();
          await page.getByText(longWorker.display_name, { exact: true }).first()
            .waitFor();
          if (!before && persona === "owner") {
            await page.getByRole("combobox", { name: "Worker", exact: true })
              .selectOption(
                JSON.stringify([longWorker.runtime_id, longWorker.worker_id]),
              );
            await page.getByRole("button", { name: "Revoke Drive grant 1" })
              .waitFor();
            await page.getByText("Worker identity", { exact: true }).click();
          }
          await capture("long-values");
        } finally {
          await context.close();
        }
      }
    }
  }
  const pageSource = await Deno.readFile(
    `${root}web/workspace/src/routes/w/[workspaceId]/workers/+page.svelte`,
  );
  const componentSource = await Deno.readFile(
    `${root}web/workspace/src/lib/workspace/drive-grants/WorkerDriveGrants.svelte`,
  );
  const apiSource = await Deno.readFile(
    `${root}web/workspace/src/lib/workspace/drive-grants/api.ts`,
  );
  await Deno.writeTextFile(
    `${out}/review-context.json`,
    JSON.stringify(
      {
        scenario: "real Workers route, loopback authoritative API projections",
        run,
        browserVersion: browser.version(),
        workersPageSha256: await sha256(pageSource),
        componentSha256: await sha256(componentSource),
        apiSha256: await sha256(apiSource),
        captures,
      },
      null,
      2,
    ),
  );
  console.log(`Visual evidence: ${out}`);
} finally {
  await browser.close();
  await stopOwnedProcesses(processes);
}
function ownerPersona(persona: string) {
  return persona === "owner";
}
async function sha256(bytes: Uint8Array) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes).buffer))]
    .map((
      byte,
    ) => byte.toString(16).padStart(2, "0")).join("");
}
