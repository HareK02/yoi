import { assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium } from "npm:playwright@1.59.1";
import { startOwnedProcesses, stopOwnedProcesses } from "../../../../tools/web-ux/src/processes.ts";

// Real static Web build and shared owned-process harness. API/stream fixtures
// prove rendering and browser interaction, not Backend persistence or authority.
Deno.test("Worker removal receipts retain Workers and expose saved progress after reload", async () => {
  const before = Deno.args.includes("--before");
  const root = new URL("../../../../", import.meta.url).pathname;
  const output = `${root}web/workspace/.svelte-kit/removal-review/${before ? "before" : "after"}/`;
  await Deno.mkdir(output, { recursive: true });
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  const processes = await startOwnedProcesses(
    [{
      id: "removal-fixture",
      command: Deno.execPath(),
      args: [
        "run",
        "--allow-net",
        "--allow-read",
        `${root}tools/web-ux/browser-tests/workspace_home_fixture_server.ts`,
        String(port),
        `${root}web/workspace/build`,
      ],
      readyUrl: `${base}/health`,
      readyTimeoutMs: 10000,
    }],
    import.meta.filename!,
    `${output}process-logs`,
    [],
  );
  const evidence: unknown[] = [];
  try {
    const browser = await chromium.launch({ headless: true });
    try {
      for (const colorScheme of ["light", "dark"] as const) {
        for (const width of [1440, 768, 390]) {
          const context = await browser.newContext({
            viewport: { width, height: 900 },
            colorScheme,
            reducedMotion: "reduce",
          });
          try {
            const page = await context.newPage();
            const errors: string[] = [];
            const cancellations: string[] = [];
            page.on("pageerror", (error) => errors.push(String(error)));
            page.on("console", (message) => {
              if (message.type() === "error") errors.push(message.text());
            });
            page.on("requestfailed", (request) => {
              const message = `${request.failure()?.errorText} ${request.url()}`;
              // The test deliberately reloads the document. Production catalog
              // refresh also aborts superseded GETs; no mutation is retried.
              if (
                request.method() === "GET" &&
                request.failure()?.errorText === "net::ERR_ABORTED"
              ) cancellations.push(message);
              else errors.push(message);
            });
            page.on("response", (response) => {
              if (response.status() >= 400) {
                errors.push(`${response.status()} ${response.url()}`);
              }
            });
            const workspace = "home-owner";
            await page.route(
              "**/api/auth/whoami",
              (route) =>
                route.fulfill({
                  json: {
                    authentication_reference: "fixture-authentication",
                    session: {
                      kind: "authenticated",
                      expires_at: "2099-01-01T00:00:00Z",
                    },
                    actor: {
                      user_id: "fixture-user",
                      account_id: "fixture-owner",
                      handle: "mika",
                      display_name: "Mika",
                      auth_method: "browser_session",
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
              const fixture = await response.json();
              fixture.permissions.delete_subjects = false;
              await route.fulfill({ json: fixture });
            });
            const worker = {
              runtime_id: "runtime-a",
              worker_id: "worker-a",
              resource_key: "W-842",
              host_id: "runtime-a",
              display_name: "Journal source — long retained history",
              label: "Journal source",
              profile: "builtin:coder",
              singleton_key: null,
              tags: [],
              workspace: {
                visibility: "workspace",
                identity: workspace,
                workspace_id: workspace,
              },
              availability: "observed",
              state: "stopped",
              last_seen_at: null,
              pinned: false,
              retention_state: "stopped",
              implementation: {
                kind: "runtime",
                display_hint: "Runtime Worker",
              },
              workdir_attachments: [],
              diagnostics: [],
            };
            const observations = [
              {
                operation_id: `removal-${"long-operation-identity-".repeat(8)}`,
                state: "waiting",
                phase: "input_handoff",
                cause: "job_pending",
              },
              {
                operation_id: "same-saved-removal-operation",
                state: "held",
                phase: "awaiting_effects_end",
                cause: "execution_unconfirmed",
              },
            ];
            let active = -1;
            let deleted = false;
            let mutations = 0;
            const plan = () => ({
              workspace_id: workspace,
              runtime_id: "runtime-a",
              generated_at: "2026-01-01T00:00:00Z",
              digest: "plan-original",
              workers: deleted ? [] : [{
                target_id: "worker-target",
                action: "worker_delete",
                worker_id: "worker-a",
                runtime_id: "runtime-a",
                runtime_worker_id: "worker-a",
                reason: "Stopped Worker",
                blocking_reason: null,
                pinned: false,
                retention_state: "stopped",
                linked_workdir_ids: [],
                running_linked: false,
              }],
              workdirs: [],
              diagnostics: [],
            });
            await page.route(
              "**/api/w/*/workers",
              (route) =>
                route.fulfill({
                  json: {
                    workspace_id: workspace,
                    limit: 100,
                    source: "fixture",
                    diagnostics: [],
                    items: deleted ? [] : [{
                      ...worker,
                      ...(!before && active >= 0 ? { removal: observations[active] } : {}),
                    }],
                  },
                }),
            );
            await page.route(
              "**/api/w/*/runtimes/runtime-a/cleanup-plan",
              (route) => route.fulfill({ json: plan() }),
            );
            await page.route(
              "**/api/w/*/runtimes/runtime-a/cleanup-executions",
              (route) => {
                mutations++;
                if (active >= 1) deleted = true;
                else active++;
                return route.fulfill({
                  status: deleted ? 200 : 202,
                  json: {
                    workspace_id: workspace,
                    runtime_id: "runtime-a",
                    executed_at: "2026-01-01T00:00:01Z",
                    results: [{
                      target_id: "worker-target",
                      action: "worker_delete",
                      status: deleted ? "deleted" : "accepted",
                      message: "",
                      ...(!deleted ? { removal: observations[active] } : {}),
                    }],
                    plan_after: plan(),
                    diagnostics: [],
                  },
                });
              },
            );
            const sendEvents = new Set<(payload: unknown) => void>();
            const streamWorker = {
              worker_id: worker.worker_id,
              runtime_id: worker.runtime_id,
              resource_key: worker.resource_key,
              workspace_id: workspace,
              availability: "observed",
              state: "stopped",
              display_name: worker.display_name,
              profile: worker.profile,
              workdir_attachments: [],
              has_running_internal_workers: false,
            };
            await page.routeWebSocket("**/api/w/*/protocol/ws", (socket) => {
              const sendEvent = (payload: unknown) =>
                socket.send(JSON.stringify({
                  protocol_version: 2,
                  frame: "event",
                  message: {
                    event: "event",
                    data: {
                      subscription_id: "removal-workers",
                      payload,
                    },
                  },
                }));
              sendEvents.add(sendEvent);
              socket.onClose(() => {
                sendEvents.delete(sendEvent);
              });
              socket.onMessage((data) => {
                const frame = JSON.parse(String(data));
                if (frame?.message?.method !== "subscribe_events") return;
                socket.send(
                  JSON.stringify({
                    protocol_version: 2,
                    frame: "response",
                    message: {
                      result: "subscribed",
                      payload: {
                        request_id: frame.message.params.request_id,
                        subscription_id: "removal-workers",
                        selector: { topic: "workspace_workers" },
                        snapshot: {
                          topic: "workers",
                          data: {
                            workers: deleted ? [] : [streamWorker],
                          },
                        },
                      },
                    },
                  }),
                );
              });
            });
            const capture = async (
              name: string,
              documentStatus: number | null,
            ) => {
              const file = `${colorScheme}-${width}-${name}.png`;
              const bytes = await page.screenshot({
                path: `${output}${file}`,
                fullPage: true,
              });
              const hash = Array.from(
                new Uint8Array(
                  await crypto.subtle.digest("SHA-256", new Uint8Array(bytes)),
                ),
              ).map((b) => b.toString(16).padStart(2, "0")).join("");
              evidence.push({
                name,
                colorScheme,
                width,
                persona: "fixture workspace owner",
                documentStatus,
                screenshot: file,
                sha256: hash,
                errors: [...errors],
                expectedGetCancellations: [...cancellations],
                accessibility: await page.locator("body").ariaSnapshot(),
              });
              assertEquals(
                await page.evaluate(
                  "document.documentElement.scrollWidth > innerWidth",
                ),
                false,
              );
            };
            let documentStatus = (await page.goto(`${base}/w/${workspace}/workers`))?.status() ??
              null;
            assertEquals(documentStatus, 200);
            await page.getByRole("button", {
              name: `Delete ${worker.display_name}`,
              exact: true,
            }).waitFor({ timeout: 10000 }).catch(async (error) => {
              await capture("unexpected-initial-state", documentStatus);
              throw error;
            });
            await capture("normal", documentStatus);
            await page.getByRole("button", {
              name: `Delete ${worker.display_name}`,
              exact: true,
            }).click();
            if (before) {
              await page.getByText("Worker deletion failed", { exact: true })
                .first().waitFor();
              await capture("waiting-receipt", documentStatus);
            } else {
              await page.locator("main").getByText("Removal waiting", {
                exact: true,
              }).waitFor();
              assertEquals(
                await page.getByText("Worker deleted", { exact: true }).count(),
                0,
              );
              await page.locator("main").getByText("Removal operation", {
                exact: true,
              }).click();
              await capture("waiting-receipt", documentStatus);
              documentStatus = (await page.reload())?.status() ?? null;
              await page.locator("main").getByText("Removal waiting", {
                exact: true,
              }).waitFor();
              assertEquals(mutations, 1);
              await capture("waiting-reload", documentStatus);
              // Change the saved intent observation, not a new user mutation.
              active = 1;
              for (const send of sendEvents) {
                send({
                  event: "worker_upserted",
                  data: { worker: streamWorker },
                });
              }
              await page.locator("main").getByText("Removal held", {
                exact: true,
              }).waitFor();
              await capture("held-subscription", documentStatus);
              documentStatus = (await page.reload())?.status() ?? null;
              await page.locator("main").getByText("Removal held", {
                exact: true,
              }).waitFor();
              await page.locator("main").getByText("Removal operation", {
                exact: true,
              }).click();
              await capture("held-reload", documentStatus);
              assertEquals(mutations, 1);
              for (
                const [cause, phase, label] of [
                  [
                    "recording_disabled",
                    "input_handoff",
                    "Journal recording disabled",
                  ],
                  ["source_unavailable", "input_handoff", "Source unavailable"],
                  [
                    "ownership_unknown",
                    "awaiting_quiescence",
                    "Ownership unknown",
                  ],
                  ["job_failed", "input_handoff", "Journal job failed"],
                  [
                    "storage_unavailable",
                    "awaiting_effects_end",
                    "Storage unavailable",
                  ],
                ]
              ) {
                observations.push({
                  operation_id: "same-saved-removal-operation",
                  state: "held",
                  phase,
                  cause,
                });
                active = observations.length - 1;
                documentStatus = (await page.reload())?.status() ?? null;
                await page.locator("main").getByText(label, { exact: false })
                  .first().waitFor();
                await capture(`held-${cause}`, documentStatus);
              }
              // Saved operation completes without a second user mutation.
              await page.locator(".worker-nav-item").first().waitFor({
                state: "attached",
              });
              deleted = true;
              for (const send of sendEvents) {
                send({
                  event: "worker_removed",
                  data: {
                    runtime_id: worker.runtime_id,
                    worker_id: worker.worker_id,
                  },
                });
              }
              await page.locator("main").getByText(
                "No Workers are visible.",
                { exact: true },
              ).waitFor();
              assertEquals(mutations, 1);
              await capture("deleted", documentStatus);
            }
            assertEquals(errors, []);
          } finally {
            await context.close();
          }
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    await Deno.writeTextFile(
      `${output}review-context.json`,
      JSON.stringify({ before, evidence }, null, 2),
    );
    await stopOwnedProcesses(processes);
  }
});
