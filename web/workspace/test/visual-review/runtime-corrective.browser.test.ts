import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";

// Production static build plus loopback fixtures only; no live accounts or keys.
// --before retains the superseded wire shape solely for before/after visual evidence.
Deno.test("Runtime settings keep list, editing and testing accessible after signed-ping authentication failure", async () => {
  const before = Deno.args.includes("--before");
  const root = new URL("../../../../", import.meta.url).pathname;
  const output = new URL(
    `../../.svelte-kit/runtime-corrective/${before ? "before" : "after"}/`,
    import.meta.url,
  ).pathname;
  await Deno.mkdir(output, { recursive: true });
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  const server = new Deno.Command(Deno.execPath(), {
    args: [
      "run",
      "--allow-net",
      "--allow-read",
      `${root}tools/web-ux/browser-tests/workspace_home_fixture_server.ts`,
      String(port),
      `${root}web/workspace/build`,
    ],
    stdout: "null",
    stderr: "null",
  }).spawn();
  try {
    // Wait for an observable ready response, bounded by timeout.
    await new Promise<void>((resolve, reject) => {
      let stopped = false;
      const deadline = setTimeout(() => {
        stopped = true;
        reject(new Error("fixture readiness timed out"));
      }, 10000);
      const probe = async () => {
        if (stopped) return;
        try {
          const response = await fetch(`${base}/health`);
          await response.body?.cancel();
          if (response.ok) {
            clearTimeout(deadline);
            resolve();
            return;
          }
        } catch { /* owned server is starting */ }
        setTimeout(probe, 25);
      };
      void probe();
    });
    const browser = await chromium.launch({ headless: true });
    const evidence: unknown[] = [];
    try {
      for (const colorScheme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390]) {
          for (const owner of [true, false]) {
            const context = await browser.newContext({
              viewport: { width, height: 900 },
              colorScheme,
              reducedMotion: "reduce",
            });
            try {
              const page = await context.newPage();
              const errors: string[] = [];
              let documentStatus: number | null = null;
              const navigate = async (url: string) => {
                documentStatus = (await page.goto(url))?.status() ?? null;
                assertEquals(documentStatus, 200);
              };
              page.on("response", (response) => {
                if (response.status() >= 400) {
                  errors.push(`${response.status()} ${response.url()}`);
                }
              });
              page.on("pageerror", (error) => errors.push(String(error)));
              page.on("console", (message) => {
                if (message.type() === "error") errors.push(message.text());
              });
              page.on("requestfailed", (request) => errors.push(request.url()));
              const workspace = owner ? "home-owner" : "home-member";
              const binding = before
                ? {
                  binding_id: "binding-observed",
                  state: "configured",
                  connection_state: "unavailable",
                  workspace_key_id: "workspace-key",
                  workspace_trust_id: "enrollment",
                }
                : { binding_id: "binding-observed", revoked_at: null };
              const runtime = {
                runtime_id: "fixture-runtime",
                label: "Development Runtime",
                kind: "remote",
                status: "unavailable",
                source: {
                  kind: "remote_http",
                  status: "active",
                  identity_authority: "server_runtime_configuration",
                  note: "Fixture",
                },
                host_ids: [],
                worker_creation_available: false,
                os: "linux",
                arch: "x86_64",
                diagnostics: [{
                  code: "runtime_authentication_failed",
                  severity: "error",
                  message:
                    "The signed Runtime request was not authenticated. Update the endpoint or key and retry.",
                }],
                management: {
                  built_in: false,
                  config_managed: true,
                  removable: true,
                  endpoint_configured: true,
                  token_ref_configured: false,
                  binding,
                },
              };
              await page.route(
                "**/api/w/*/runtimes",
                (route) =>
                  route.fulfill({
                    json: {
                      workspace_id: workspace,
                      limit: 200,
                      source: "fixture",
                      items: [runtime],
                      diagnostics: [],
                    },
                  }),
              );
              await page.route(
                "**/api/w/*/runtimes/fixture-runtime",
                (route) =>
                  route.fulfill({
                    json: {
                      workspace_id: workspace,
                      runtime,
                      endpoint: "https://runtime.example.test",
                      trust_key: {
                        status: "active",
                        binding_id: "binding-observed",
                        fingerprint: "sha256:runtime-public-key",
                        created_at: "2026-10-01T00:00:00Z",
                        updated_at: "2026-10-01T00:00:00Z",
                        revoked_at: null,
                      },
                      recent_audit: [],
                    },
                  }),
              );
              await page.route(
                "**/api/w/*/runtimes/fixture-runtime/connection-tests",
                (route) =>
                  route.fulfill({
                    json: {
                      workspace_id: workspace,
                      runtime_id: "fixture-runtime",
                      binding_id: "binding-observed",
                      ...(before
                        ? {
                          connection_state: "unavailable",
                          verification: null,
                        }
                        : {}),
                      checked_at: "2026-10-10T00:00:00Z",
                      status: "failed",
                      failure_kind: "authentication",
                      expected_protocol_version: 1,
                      actual_protocol_version: null,
                      diagnostics: [],
                    },
                  }),
              );
              const prefix = `${colorScheme}-${width}-${
                owner ? "owner" : "member"
              }`;
              const capture = async (name: string) => {
                await page.screenshot({
                  path: `${output}${prefix}-${name}.png`,
                  fullPage: true,
                });
                evidence.push({
                  colorScheme,
                  width,
                  owner,
                  name,
                  url: page.url(),
                  documentStatus,
                  screenshot: `${prefix}-${name}.png`,
                  errors: [...errors],
                  accessibility: await page.locator("main").ariaSnapshot(),
                });
                assertEquals(
                  await page.evaluate(
                    "document.documentElement.scrollWidth > innerWidth",
                  ),
                  false,
                  prefix,
                );
              };
              await navigate(`${base}/w/${workspace}/settings/runtimes`);
              await page.getByRole("link", {
                name: "Development Runtime",
                exact: true,
              }).waitFor();
              await page.getByRole("button", { name: "Test", exact: true })
                .click();
              await page.getByText("Connection test: Authentication failed", {
                exact: true,
              }).waitFor();
              await capture("list-failed");
              assertEquals(
                await page.getByRole("button", { name: "Test", exact: true })
                  .isEnabled(),
                true,
              );
              if (owner) {
                await page.getByRole("button", {
                  name: "Add Runtime",
                  exact: true,
                }).click();
                await page.getByLabel("Runtime public bundle", { exact: true })
                  .waitFor();
                await capture("registration");
              }
              await navigate(
                `${base}/w/${workspace}/settings/runtimes/fixture-runtime`,
              );
              await page.getByRole("heading", {
                name: "Identity and binding",
                exact: true,
              }).waitFor();
              await capture("detail");
              if (!before) {
                await page.getByRole("button", { name: "Test", exact: true })
                  .click();
                await page.getByText("Signed ping failed · authentication", {
                  exact: true,
                }).waitFor();
                await capture("detail-failed");
                assert(
                  await page.getByRole("button", { name: "Test", exact: true })
                    .isEnabled(),
                );
              }
              assertEquals(
                await page.getByRole("button", {
                  name: "Edit Runtime",
                  exact: true,
                }).count(),
                owner ? 1 : 0,
              );
              if (owner) {
                await page.getByRole("button", {
                  name: "Edit Runtime",
                  exact: true,
                }).click();
                await page.getByLabel("Endpoint", { exact: true }).waitFor();
                await capture("edit");
                assert(
                  await page.getByRole("button", {
                    name: "Save Runtime settings",
                    exact: true,
                  }).isEnabled(),
                );
                if (!before) {
                  assertEquals(
                    await page.getByLabel("Workspace trust ID", { exact: true })
                      .count(),
                    0,
                  );
                }
              }
              if (!before) {
                Object.assign(binding, { revoked_at: "2026-10-10T00:00:00Z" });
                await page.goto(`${base}/w/${workspace}/settings/runtimes`);
                await page.getByRole("cell", { name: "Revoked", exact: true })
                  .waitFor();
                assertEquals(
                  await page.getByRole("button", { name: "Test", exact: true })
                    .count(),
                  0,
                );
                await capture("revoked");
              }
              assertEquals(errors, []);
            } finally {
              await context.close();
            }
          }
        }
      }
    } finally {
      await browser.close();
    }
    await Deno.writeTextFile(
      `${output}review-context.json`,
      JSON.stringify(evidence, null, 2),
    );
  } finally {
    server.kill("SIGTERM");
    await server.status;
  }
});
