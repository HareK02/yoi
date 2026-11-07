import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../../tools/web-ux/src/processes.ts";

// 本番buildと既存の隔離fixtureを接続する。実Backendの認可・副作用は検証しない。
Deno.test("Runtime connection HTTP control failure displays the public cause without disabling other controls", async () => {
  const before = Deno.args.includes("--before");
  const primaryBefore = Deno.args.includes("--primary-before");
  const output = new URL(
    `../../.svelte-kit/runtime-control/${
      before ? "before" : primaryBefore ? "primary-before" : "after"
    }/`,
    import.meta.url,
  ).pathname;
  const root = new URL("../../../../", import.meta.url).pathname;
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  const processes = await startOwnedProcesses(
    [{
      id: "fixture",
      command: Deno.execPath(),
      cwd: root,
      args: [
        "run",
        "--allow-read",
        "--allow-net",
        "tools/web-ux/browser-tests/workspace_home_fixture_server.ts",
        String(port),
        "web/workspace/build",
      ],
      readyUrl: `${base}/health`,
      readyTimeoutMs: 10000,
    }],
    import.meta.url,
    output + "logs",
    [],
  );
  const captures: unknown[] = [];
  try {
    const browser = await chromium.launch({ headless: true });
    try {
      for (const width of [1440, 768, 390]) {
        const context = await browser.newContext({
          viewport: { width, height: 900 },
          colorScheme: "light",
          reducedMotion: "reduce",
        });
        try {
          const page = await context.newPage();
          page.setDefaultTimeout(10000);
          const errors: string[] = [];
          const consoleErrors: string[] = [];
          const navigationAborts: string[] = [];
          page.on("pageerror", (e) => errors.push(String(e)));
          page.on("requestfailed", (r) => {
            if (
              r.failure()?.errorText === "net::ERR_ABORTED" &&
              new URL(r.url()).pathname.endsWith("/workers")
            ) navigationAborts.push(r.url());
            else errors.push(r.url());
          });
          page.on("console", (m) => {
            if (m.type() === "error") consoleErrors.push(m.text());
          });
          await page.route(
            "**/api/auth/whoami",
            (route) =>
              route.fulfill({
                json: {
                  authentication_reference: "fixture-authentication",
                  actor: {
                    user_id: "fixture-user",
                    account_id: "fixture-owner",
                    handle: "alice",
                    display_name: "Alice",
                    auth_method: "browser_session",
                  },
                  session: {
                    kind: "authenticated",
                    expires_at: "2099-01-01T00:00:00Z",
                  },
                },
              }),
          );
          await page.route(
            "**/api/hosting/access",
            (route) => route.fulfill({ json: { is_admin: false } }),
          );
          await page.route("**/api/w/*/workspace", async (route) => {
            const response = await route.fetch();
            const payload = await response.json();
            payload.permissions.delete_subjects = true;
            await route.fulfill({ json: payload });
          });
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
            diagnostics: [],
            management: {
              built_in: false,
              config_managed: true,
              removable: true,
              endpoint_configured: true,
              token_ref_configured: false,
              binding: { binding_id: "binding-observed", revoked_at: null },
            },
          };
          await page.route(
            "**/api/w/*/runtimes/fixture-runtime",
            (route) =>
              route.fulfill({
                json: {
                  workspace_id: "home-owner",
                  runtime,
                  endpoint: "https://runtime.example.test",
                  trust_key: {
                    status: "active",
                    binding_id: "binding-observed",
                    fingerprint: "sha256:fixture-key",
                    created_at: "2026-10-01T00:00:00Z",
                    updated_at: "2026-10-01T00:00:00Z",
                    revoked_at: null,
                  },
                  recent_audit: [],
                },
              }),
          );
          const explanation =
            "Authorization for the Runtime request failed. Other Workspace resources remain available.";
          let calls = 0;
          await page.route("**/connection-tests", (route) => {
            calls++;
            return route.fulfill({
              status: 403,
              contentType: "application/problem+json",
              json: {
                type: "urn:yoi:problem:runtime-control",
                instance: "",
                status: 403,
                title: "Runtime control failed",
                detail: explanation,
                yoi_failure: {
                  version: 1,
                  required_extensions: [],
                  domain: "runtime_control",
                  reason: "authorization_failed",
                  category: "authorization",
                  details_version: 1,
                  details: "authorization_failed",
                  explanation,
                  context: {
                    operations: [],
                    causes: [],
                    diagnostics: [],
                    correlation: null,
                    supplementary_omitted: false,
                  },
                },
              },
            });
          });
          const response = await page.goto(
            `${base}/w/home-owner/settings/runtimes/fixture-runtime`,
          );
          assertEquals(response?.status(), 200);
          const button = page.getByRole("button", {
            name: "Test",
            exact: true,
          });
          await button.click().catch(async (error) => {
            await page.screenshot({
              path: output + `failure-${width}.png`,
              fullPage: true,
            });
            await Deno.writeTextFile(
              output + `failure-${width}.json`,
              JSON.stringify(
                {
                  errors,
                  consoleErrors,
                  body: await page.locator("body").ariaSnapshot(),
                },
                null,
                2,
              ),
            );
            throw error;
          });
          await page.getByText(
            before ? "Connection test failed (403)" : explanation,
            { exact: true },
          ).waitFor();
          assert(await button.isEnabled());
          assert(
            await page.getByRole("button", {
              name: "Edit Runtime",
              exact: true,
            }).isEnabled(),
          );
          assertEquals(calls, 1);
          assertEquals(
            await page.evaluate(
              "document.documentElement.scrollWidth > innerWidth",
            ),
            false,
          );
          assertEquals(errors, []);
          const image = `light-${width}.png`;
          const bytes = await page.screenshot({
            path: output + image,
            fullPage: true,
          });
          const hash = Array.from(
            new Uint8Array(
              await crypto.subtle.digest("SHA-256", new Uint8Array(bytes)),
            ),
            (b) => b.toString(16).padStart(2, "0"),
          ).join("");
          captures.push({
            image,
            sha256: hash,
            width,
            theme: "light",
            persona: "synthetic owner",
            route: page.url(),
            documentStatus: response?.status(),
            expectedHttpFailure: 403,
            consoleErrors,
            navigationAborts,
            errors,
            accessibility: await page.locator("main").ariaSnapshot(),
          });
          if (!before) {
            const worker = {
              runtime_id: "fixture-runtime",
              worker_id: "fixture-worker",
              resource_key: "W-1",
              availability: "observed",
              host_id: "fixture-host",
              display_name: "Fixture Worker",
              label: "Fixture Worker",
              profile: "builtin:coder",
              singleton_key: null,
              tags: [],
              workspace: {
                visibility: "workspace",
                identity: "home-owner",
                workspace_id: "home-owner",
              },
              state: "stopped",
              restore_observation_token: "fixture-observation",
              last_seen_at: null,
              pinned: false,
              retention_state: "active",
              implementation: {
                kind: "runtime",
                display_hint: "Runtime Worker",
              },
              diagnostics: [],
            };
            await page.route(
              "**/api/w/*/workers",
              (route) =>
                route.fulfill({
                  json: {
                    workspace_id: "home-owner",
                    limit: 100,
                    source: "fixture",
                    items: [worker],
                    diagnostics: [],
                  },
                }),
            );
            await page.route("**/cleanup-plan", (route) =>
              route.fulfill({
                json: {
                  workspace_id: "home-owner",
                  runtime_id: "fixture-runtime",
                  generated_at: "2026-10-01T00:00:00Z",
                  digest: "fixture-digest",
                  workers: [],
                  workdirs: [],
                  diagnostics: [],
                },
              }));
            const restoreWire = JSON.parse(
              await Deno.readTextFile(
                new URL(
                  "../../../../crates/error-contract/fixtures/runtime-restore-session-denied.json",
                  import.meta.url,
                ),
              ),
            );
            const primary = restoreWire.result.control_explanation;
            let restores = 0;
            await page.route("**/workers/fixture-worker/restore", (route) => {
              restores++;
              return route.fulfill({
                json: restoreWire,
              });
            });
            const workersResponse = await page.goto(
              `${base}/w/home-owner/workers`,
            );
            await page.getByRole("button", {
              name: "Restore Fixture Worker",
              exact: true,
            }).click();
            await page.getByText(
              primaryBefore ? "Worker Restore outcome unknown" : primary,
              { exact: true },
            ).waitFor();
            assertEquals(restores, 1);
            assertEquals(errors, []);
            assertEquals(
              await page.evaluate(
                "document.documentElement.scrollWidth > innerWidth",
              ),
              false,
            );
            const restoreImage = `light-${width}-restore.png`;
            const restoreBytes = await page.screenshot({
              path: output + restoreImage,
              fullPage: true,
            });
            captures.push({
              image: restoreImage,
              sha256: Array.from(
                new Uint8Array(
                  await crypto.subtle.digest(
                    "SHA-256",
                    new Uint8Array(restoreBytes),
                  ),
                ),
                (b) => b.toString(16).padStart(2, "0"),
              ).join(""),
              width,
              theme: "light",
              persona: "synthetic owner",
              route: page.url(),
              documentStatus: workersResponse?.status(),
              expectedDomainState: "rolled_back",
              primaryBefore,
              consoleErrors,
              navigationAborts,
              errors,
              accessibility: await page.locator("body").ariaSnapshot(),
            });
          }
        } finally {
          await context.close();
        }
      }
    } finally {
      await browser.close();
    }
    await Deno.writeTextFile(
      output + "review-context.json",
      JSON.stringify(captures, null, 2),
    );
  } finally {
    await stopOwnedProcesses(processes);
  }
});
