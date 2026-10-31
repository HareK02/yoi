#!/usr/bin/env -S deno run -A
/** Actual Web build + synthetic HTTP only. No real Backend/Runtime is contacted. */
import { assert, assertEquals } from "@std/assert";
import { dirname, fromFileUrl, join, resolve } from "@std/path";
import { chromium } from "playwright";
import type {
  SubjectBodyApiError as SubjectBodyApiErrorPayload,
  SubjectBodyFailedRequest,
  SubjectBodyMutationResponse,
  SubjectBodyRef,
  SubjectBodyResponse,
  SubjectBodyStartRequest,
  SubjectBodyStopOutcomeResponse,
  SubjectBodyStopRequest,
} from "../src/lib/generated/subject-body-api.ts";
import { sha256File } from "../../../tools/web-ux/src/artifacts.ts";
import {
  startOwnedProcesses,
  stopOwnedProcesses,
} from "../../../tools/web-ux/src/processes.ts";
import {
  assertNoPageWideOverflow,
  freePort,
} from "../../../tools/web-ux/browser-tests/memory_document_harness.ts";

const root = resolve(dirname(fromFileUrl(import.meta.url)), "../../..");
const run = Deno.args[0] ?? "review";
if (!/^[\w-]+$/.test(run)) throw new Error("safe unique run id required");
const outputBase = join(root, "target/t807-stale-web");
await Deno.mkdir(outputBase, { recursive: true });
const output = join(outputBase, run);
await Deno.mkdir(output); // Never replace previous evidence.
const buildRoot = join(root, "web/workspace/build");
await Deno.stat(join(buildRoot, "index.html"));
const git = async (args: string[]) => {
  const result = await new Deno.Command("git", {
    args,
    cwd: root,
    stdout: "piped",
  }).output();
  assert(result.success);
  return new TextDecoder().decode(result.stdout).trim();
};
const manifest = {
  evidence: "actual-Web-with-synthetic-HTTP-NOT-real-Backend-or-Runtime",
  source: {
    commit: await git(["rev-parse", "HEAD"]),
    status: await git(["status", "--short"]),
  },
  buildIndexSha256: await sha256File(join(buildRoot, "index.html")),
  browser: "",
  status: "running",
  visualVerdict: "unreviewed",
  expectedFaults: [
    "structured session.record rejection",
    "unknown start/stop receipts",
    "dedicated HTTP409 generation/absent-start superseded and stale-token not_accepted",
    "HTTP409 superseded outcome is not historical stop failure or confirmation",
    "busy unresolved, generic409, request input conflict and 401 retain input",
    "mismatched identities and malformed terminal error shapes retain input",
    "mutation response lost after synthetic admission; exact retry only",
    "503 body GET",
    "navigation teardown aborts GET /workers refresh",
  ],
  captures: [] as unknown[],
  assertions: [] as string[],
  failures: [] as string[],
};
const persist = () =>
  Deno.writeTextFile(
    join(output, "review-context.json"),
    JSON.stringify(manifest, null, 2),
  );
const port = await freePort();
const base = `http://127.0.0.1:${port}`;
const owned = await startOwnedProcesses(
  [{
    id: "synthetic-memory-http",
    command: Deno.execPath(),
    cwd: join(root, "tools/web-ux"),
    args: [
      "run",
      "--cached-only",
      "-A",
      "browser-tests/memory_document_fixture_server.ts",
      String(port),
      buildRoot,
      "3",
    ],
    readyUrl: `${base}/health`,
  }],
  join(output, "scenario.json"),
  join(output, "logs"),
  [],
);
let browser;
try {
  browser = await chromium.launch({ headless: true });
  manifest.browser = browser.version();
  for (const colorScheme of ["light", "dark"] as const) {
    for (const width of [1440, 768, 390]) {
      const prefix = `${colorScheme}-${width}`;
      const context = await browser.newContext({
        viewport: { width, height: width === 390 ? 844 : 900 },
        colorScheme,
        reducedMotion: "reduce",
      });
      try {
        let accountId = "owner";
        let observation: "absent" | "idle" | "stopped" | "unavailable" =
          "absent";
        let permission = true;
        let getFailure = false;
        type Action =
          | "reject"
          | "unknown"
          | "accepted"
          | "stale-generation"
          | "stale-token"
          | "absent-race"
          | "busy"
          | "generic-conflict"
          | "input-conflict"
          | "transport-lost"
          | "unauthorized"
          | "wrong-workspace"
          | "wrong-subject"
          | "wrong-request"
          | "extra-request-field"
          | "extra-error-field"
          | "invalid-resolution";
        let action: Action = "reject";
        let outcome: SubjectBodyStopOutcomeResponse["outcome"] | "superseded" =
          "pending";
        let generation = "17";
        let tokenChanged = false;
        let heldBodyRead: Promise<void> | null = null;
        const expectedHttp: { status: number; path: string }[] = [];
        const expectedLost: string[] = [];
        const posts: { path: string; body: string }[] = [];
        const owner = (): SubjectBodyRef => ({
          runtime_id: "runtime",
          worker_id: "worker",
          generation,
        });
        const observationToken = () =>
          `token-${generation}-${observation}${tokenChanged ? "-fresh" : ""}`;
        function body(workspace: string, subject: string): SubjectBodyResponse {
          return {
            workspace_id: workspace,
            subject_id: subject,
            can_start: permission,
            can_stop: permission,
            current: observation === "absent" ? null : owner(),
            observation: observation === "absent"
              ? { kind: "absent" }
              : observation === "unavailable"
              ? {
                kind: "unavailable",
                diagnostics: [{
                  code: "runtime_unavailable",
                  severity: "warning",
                  message:
                    "Runtime observation unavailable; no replacement body will be started.",
                }],
              }
              : {
                kind: "available",
                worker: {
                  runtime_id: "runtime",
                  worker_id: "worker",
                  resource_key: "W-1",
                  host_id: "host",
                  label: "Conversation body",
                  workspace: {
                    visibility: "workspace",
                    identity: workspace,
                    workspace_id: workspace,
                  },
                  implementation: { kind: "embedded", display_hint: "" },
                  availability: "observed",
                  state: observation,
                  worker_state: observation === "stopped"
                    ? null
                    : { last_command_id: 1, state: { kind: "idle" } },
                  restore_observation_token: observationToken(),
                },
              },
          };
        }
        await context.route("**/api/auth/whoami", async (route) => {
          const response = await route.fetch();
          const value = await response.json();
          value.actor.account_id = accountId;
          await route.fulfill({ response, json: value });
        });
        // The second Workspace uses a synthetic, silent protocol channel too.
        await context.routeWebSocket(
          "**/api/w/other-review/protocol/ws",
          (socket) => {
            socket.onMessage(() => {});
          },
        );
        // Reuse the existing isolated fixture for a second Workspace identity.
        await context.route(
          /\/api\/w\/other-review(?:\/.*)?$/,
          async (route) => {
            const response = await route.fetch({
              url: route.request().url().replace(
                "/api/w/other-review/",
                "/api/w/memory-review/",
              ),
            });
            await route.fulfill({
              response,
              body: (await response.text()).replaceAll(
                "memory-review",
                "other-review",
              ),
            });
          },
        );
        await context.route(
          /\/api\/w\/[^/]+\/subjektiv\/subjects\/[^/]+\/body(?:\/.*)?$/,
          async (route) => {
            const request = route.request();
            const path = new URL(request.url()).pathname;
            const [, workspace, subject] = path.match(
              /\/api\/w\/([^/]+)\/subjektiv\/subjects\/([^/]+)\/body/,
            )!;
            if (request.method() === "GET") {
              if (heldBodyRead) await heldBodyRead;
              if (getFailure) expectedHttp.push({ status: 503, path });
              await route.fulfill({
                status: getFailure ? 503 : 200,
                json: getFailure
                  ? { error: "Synthetic body observation unavailable" }
                  : body(workspace, subject),
              });
              return;
            }
            const raw = request.postData()!;
            posts.push({ path, body: raw });
            const input: SubjectBodyStartRequest | SubjectBodyStopRequest = JSON
              .parse(raw);
            assert(!("accountId" in input));
            const apiError = (
              resolution: SubjectBodyFailedRequest["resolution"] | null,
              error: string,
              message: string,
            ): SubjectBodyApiErrorPayload => ({
              error,
              message,
              diagnostics: [{ code: error, severity: "warning", message }],
              request: resolution === null ? null : {
                workspace_id: workspace,
                subject_id: subject,
                request_id: input.request_id,
                resolution,
              },
            });
            const fail = async (
              json: unknown,
              status = 409,
            ) => {
              expectedHttp.push({ status, path });
              await route.fulfill({ status, json });
            };
            if (path.endsWith("/outcome")) {
              if (outcome === "superseded") {
                await fail(apiError(
                  "superseded",
                  "subject_body_request_superseded",
                  "The saved stop intent no longer targets the current body.",
                ));
              } else {
                await route.fulfill({
                  json: {
                    request_id: input.request_id,
                    outcome,
                    body: null,
                  } satisfies SubjectBodyStopOutcomeResponse,
                });
              }
              return;
            }
            if (action === "transport-lost") {
              // 合成受付後に応答だけを失わせる。後続GETで本体が見えても要求は決済しない。
              observation = "idle";
              expectedLost.push(path);
              await route.abort("failed");
              return;
            }
            if (
              action === "stale-generation" || action === "stale-token" ||
              action === "absent-race"
            ) {
              if (action === "stale-token") tokenChanged = true;
              else generation = String(Number(generation) + 1);
              if (action === "absent-race") observation = "idle";
              await fail(apiError(
                action === "stale-token" ? "not_accepted" : "superseded",
                action === "stale-token"
                  ? "worker_lifecycle_observation_stale"
                  : "subject_body_superseded",
                action === "stale-token"
                  ? "The current body observation token changed."
                  : "The current body generation changed.",
              ));
              return;
            }
            if (action === "busy" || action === "input-conflict") {
              await fail(apiError(
                "unresolved",
                action === "busy"
                  ? "subject_body_busy"
                  : "subject_body_request_input_conflict",
                action === "busy"
                  ? "The body is busy; the saved request is unresolved."
                  : "This request ID is already bound to different input.",
              ));
              return;
            }
            if (action === "generic-conflict") {
              // 要求の証拠がないstatusだけでは決済しない。
              await fail(apiError(null, "conflict", "Generic conflict"));
              return;
            }
            if (
              action !== "reject" && action !== "unknown" &&
              action !== "accepted"
            ) {
              const terminal = apiError(
                "not_accepted",
                "subject_body_observation_stale",
                "Synthetic stale observation rejection.",
              );
              assert(terminal.request);
              let json: unknown = terminal;
              switch (action) {
                case "unauthorized":
                  await fail(terminal, 401);
                  return;
                case "wrong-workspace":
                  terminal.request.workspace_id = "other-review";
                  break;
                case "wrong-subject":
                  terminal.request.subject_id = "different-subject";
                  break;
                case "wrong-request":
                  terminal.request.request_id = "another-request";
                  break;
                case "extra-request-field":
                  json = {
                    ...terminal,
                    request: { ...terminal.request, extra: true },
                  };
                  break;
                case "extra-error-field":
                  json = { ...terminal, extra: true };
                  break;
                case "invalid-resolution":
                  json = {
                    ...terminal,
                    request: { ...terminal.request, resolution: "rejected" },
                  };
                  break;
              }
              // 意図的な不正JSONはDTOへcastせずunknownのまま扱う。
              await fail(json);
              return;
            }
            if (action === "accepted" && path.endsWith("/start")) {
              observation = "idle";
            }
            await route.fulfill({
              json: {
                request_id: input.request_id,
                disposition: action === "reject" ? "rejected" : action,
                diagnostics: action === "reject"
                  ? [{
                    code: "permission_missing",
                    severity: "error",
                    message:
                      "Required permission session.record is missing. Ask an administrator to review the default profile; no permissions were added.",
                  }]
                  : [],
                body: body(workspace, subject),
              } satisfies SubjectBodyMutationResponse,
            });
          },
        );
        const page = await context.newPage();
        type BrowserDiagnostic =
          | { kind: "page" | "console"; message: string }
          | { kind: "request"; url: string; message: string | undefined }
          | { kind: "http"; status: number; url: string };
        const errors: BrowserDiagnostic[] = [];
        let documentStatus: number | null = null;
        page.on(
          "pageerror",
          (e) => errors.push({ kind: "page", message: e.message }),
        );
        page.on("console", (e) => {
          if (["warning", "error"].includes(e.type())) {
            errors.push({ kind: "console", message: e.text() });
          }
        });
        page.on(
          "requestfailed",
          (r) =>
            errors.push({
              kind: "request",
              url: r.url(),
              message: r.failure()?.errorText,
            }),
        );
        page.on("response", (r) => {
          if (r.request().isNavigationRequest()) documentStatus = r.status();
          if (r.status() >= 400) {
            errors.push({ kind: "http", status: r.status(), url: r.url() });
          }
        });
        const panel = () =>
          page.getByRole("region", { name: "Conversation body", exact: true });
        const button = (name: string) =>
          panel().getByRole("button", { name, exact: true });
        async function ready() {
          await page.locator(".subject-body header button:not(:disabled)")
            .waitFor();
        }
        async function refresh() {
          await ready();
          await button("Refresh body state").click();
          await ready();
        }
        function holdBodyRead() {
          let release!: () => void;
          heldBodyRead = new Promise<void>((resolve) => {
            release = resolve;
          });
          return () => {
            heldBodyRead = null;
            release();
          };
        }
        const lastInput = ():
          | SubjectBodyStartRequest
          | SubjectBodyStopRequest => JSON.parse(posts.at(-1)!.body);
        async function retainedAfterReadAndReload(kind: "start" | "stop") {
          const count = posts.length;
          await refresh();
          assertEquals(await button("Stop conversation body").count(), 0);
          assertEquals(await button("Start conversation body").count(), 0);
          assertEquals(await button("Restart conversation body").count(), 0);
          assert(await button(`Retry same ${kind} request`).isEnabled());
          await page.reload();
          await panel().getByText(
            new RegExp(`An unresolved ${kind} request is retained`),
          )
            .waitFor();
          await ready();
          assert(await button(`Retry same ${kind} request`).isEnabled());
          assertEquals(
            posts.length,
            count,
            "GET/reload must not send another mutation",
          );
        }
        async function freshStop(
          previous: SubjectBodyStartRequest | SubjectBodyStopRequest,
        ) {
          const count = posts.length;
          const currentOwner = owner();
          const token = observationToken();
          action = "accepted";
          await button("Stop conversation body").click();
          await panel().getByText("Request accepted.", { exact: true })
            .waitFor();
          await ready();
          assertEquals(
            posts.length,
            count + 1,
            "one fresh explicit operation only",
          );
          const next = lastInput();
          assert(next.request_id !== previous.request_id);
          assertEquals(next.expected_current, currentOwner);
          assertEquals(next.expected_observation_token, token);
          return next;
        }
        async function capture(name: string, expectedErrors: string[] = []) {
          await panel().scrollIntoViewIfNeeded();
          const id = `${prefix}-${name}`;
          const image = join(output, `${id}.png`);
          const aria = join(output, `${id}.aria.txt`);
          const panelImage = join(output, `${id}.panel.png`);
          await page.screenshot({ path: image });
          await panel().screenshot({ path: panelImage });
          await Deno.writeTextFile(
            aria,
            await page.locator("body").ariaSnapshot(),
          );
          manifest.captures.push({
            id,
            route: new URL(page.url()).pathname,
            accountId,
            viewport: page.viewportSize(),
            theme: colorScheme,
            documentStatus,
            visibleAlerts: await page.getByRole("alert").allTextContents(),
            bodyText: await panel().innerText(),
            errors: [...errors],
            expectedErrors,
            expectedHttp: [...expectedHttp],
            expectedLost: [...expectedLost],
            image,
            imageSha256: await sha256File(image),
            panelImage,
            panelImageSha256: await sha256File(panelImage),
            accessibilitySnapshot: aria,
            reviewed: false,
          });
          await persist();
          await assertNoPageWideOverflow(page);
        }
        // Enter through the ordinary Subjects list, not a special-purpose shell.
        await page.goto(`${base}/w/memory-review/memory`);
        await page.locator('a.subject-row[href$="/empty-subject"]').click();
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        await button("Refresh body state").focus();
        await page.keyboard.press("Tab");
        assert(
          await button("Start conversation body").evaluate((el) =>
            el === el.ownerDocument.activeElement
          ),
        );
        await capture("before-absent-focus");
        await page.keyboard.press("Enter");
        await panel().getByText("Request rejected.", { exact: true }).waitFor();
        await ready();
        await capture("permission-rejection");
        action = "unknown";
        await button("Start conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const unknownInput = posts.at(-1)!.body;
        observation = "idle";
        await refresh();
        assertEquals(await button("Start conversation body").count(), 0);
        await capture("unknown-after-get");
        await page.reload();
        await panel().getByText(/An unresolved start request is retained/)
          .waitFor();
        await ready();
        action = "accepted";
        await button("Retry same start request").click();
        await panel().getByText("Request accepted.", { exact: true }).waitFor();
        await ready();
        assertEquals(posts.at(-1)!.body, unknownInput);
        await capture("running-reconciled");
        action = "unknown";
        await button("Stop conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const stopInput = posts.at(-1)!.body;
        await button("Check stop outcome").click();
        await panel().getByText("Stop outcome pending.", { exact: true })
          .waitFor();
        await ready();
        observation = "stopped";
        await refresh();
        assertEquals(await button("Restart conversation body").count(), 0);
        await capture("stopped-but-unresolved");
        outcome = "stopped";
        await button("Check stop outcome").click();
        await panel().getByText("Historical stop confirmed.", { exact: true })
          .waitFor();
        await ready();
        assertEquals(posts.at(-1)!.body, stopInput);
        await capture("stop-confirmed");
        action = "accepted";
        await button("Restart conversation body").click();
        await panel().getByText("Request accepted.", { exact: true }).waitFor();
        await ready();
        const restart = JSON.parse(posts.at(-1)!.body);
        assertEquals(restart.expected_observation_token, "token-17-stopped");
        assertEquals(restart.expected_current.generation, "17");
        assert(restart.request_id !== JSON.parse(stopInput).request_id);
        await capture("restarted-running");
        // 実契約の専用409で決済しても、独立した本体GETが終わるまで新操作は許可しない。
        action = "stale-generation";
        let releaseBody = holdBodyRead();
        await button("Stop conversation body").click();
        await panel().getByRole("alert").filter({
          hasText: /Request superseded\./,
        }).waitFor();
        const staleGeneration = lastInput();
        assertEquals(staleGeneration.expected_current?.generation, "17");
        assert(await button("Stop conversation body").isDisabled());
        assertEquals(await button("Retry same stop request").count(), 0);
        await capture("stale-generation-awaiting-get", [
          "intentional dedicated HTTP409 superseded",
        ]);
        const beforeGenerationRead = posts.length;
        releaseBody();
        await ready();
        assertEquals(posts.length, beforeGenerationRead);
        await capture("old-generation-rejected", [
          "intentional dedicated HTTP409 superseded",
        ]);
        await freshStop(staleGeneration);
        await capture("generation-fresh-explicit");

        action = "stale-token";
        await button("Stop conversation body").click();
        await panel().getByRole("alert").filter({
          hasText: /Request not accepted\./,
        }).waitFor();
        await ready();
        const staleToken = lastInput();
        assertEquals(
          staleToken.expected_current,
          owner(),
          "same generation, stale token",
        );
        assert(staleToken.expected_observation_token !== observationToken());
        await capture("old-token-rejected", [
          "intentional dedicated HTTP409 not_accepted",
        ]);
        await freshStop(staleToken);
        await capture("token-fresh-explicit");

        // 旧操作は作用済みかもしれない。supersededは現在への作用失効であり過去の結果ではない。
        action = "transport-lost";
        await button("Stop conversation body").click();
        await panel().getByRole("alert").filter({
          hasText: /Request outcome unknown\./,
        }).waitFor();
        await ready();
        const lostStop = posts.at(-1)!.body;
        await retainedAfterReadAndReload("stop");
        await capture("lost-stop-retained", [
          "intentional mutation response loss",
        ]);
        generation = String(Number(generation) + 1);
        outcome = "superseded";
        releaseBody = holdBodyRead();
        await button("Check stop outcome").click();
        await panel().getByRole("alert").filter({
          hasText: /Request superseded\./,
        }).waitFor();
        assertEquals(
          posts.at(-1)!.body,
          lostStop,
          "outcome uses the frozen stop input",
        );
        const supersessionText = await panel().innerText();
        assert(
          supersessionText.includes(
            "This does not determine whether an earlier operation took effect.",
          ),
        );
        assert(!supersessionText.includes("Historical stop confirmed."));
        assert(!supersessionText.includes("Stop failed"));
        assert(await button("Stop conversation body").isDisabled());
        await capture("outcome-superseded-awaiting-get", [
          "intentional dedicated HTTP409 superseded",
        ]);
        releaseBody();
        await ready();
        const beforeSupersededReload = posts.length;
        await page.reload();
        await ready();
        assertEquals(await button("Retry same stop request").count(), 0);
        assertEquals(posts.length, beforeSupersededReload);
        await freshStop(JSON.parse(lostStop));
        await capture("superseded-reload-fresh-explicit");
        outcome = "pending";

        // 並行absent-startで別要求が本体を確保した場面を模擬する。
        observation = "absent";
        await refresh();
        action = "absent-race";
        await button("Start conversation body").click();
        await panel().getByRole("alert").filter({
          hasText: /Request superseded\./,
        }).waitFor();
        await ready();
        const absentRace = lastInput();
        assertEquals(absentRace.expected_current, null);
        assertEquals(absentRace.expected_observation_token, null);
        assertEquals(await button("Start conversation body").count(), 0);
        await freshStop(absentRace);
        await capture("absent-start-race-fresh-explicit", [
          "synthetic concurrent owner HTTP409 superseded",
        ]);

        // busyは明示的にunresolvedであり、409を一律terminalとしない。
        observation = "stopped";
        await refresh();
        action = "busy";
        await button("Restart conversation body").click();
        await panel().getByRole("alert").filter({
          hasText: /Request outcome unknown\./,
        }).waitFor();
        await ready();
        const busyStart = posts.at(-1)!.body;
        await retainedAfterReadAndReload("start");
        await capture("busy-retained", ["intentional HTTP409 unresolved"]);
        action = "accepted";
        await button("Retry same start request").click();
        await panel().getByText("Request accepted.", { exact: true }).waitFor();
        await ready();
        assertEquals(posts.at(-1)!.body, busyStart);
        await capture("busy-exact-retry");

        // 本番HTTP parserでstatus・identity・JSONの拒否を検査する。
        // 正常fixtureは生成DTOに従い、不正JSONはunknownとして保持する。
        const retainingFaults: Action[] = [
          "generic-conflict",
          "input-conflict",
          "unauthorized",
          "wrong-workspace",
          "wrong-subject",
          "wrong-request",
          "extra-request-field",
          "extra-error-field",
          "invalid-resolution",
        ];
        for (const fault of retainingFaults) {
          action = fault;
          await button("Stop conversation body").click();
          await panel().getByRole("alert").filter({
            hasText: /Request outcome unknown\./,
          }).waitFor();
          await ready();
          const frozen = posts.at(-1)!.body;
          await retainedAfterReadAndReload("stop");
          // 非terminal応答の明示再送でも同じ入力を保持する。
          await button("Retry same stop request").click();
          await panel().getByRole("alert").filter({
            hasText: /Request outcome unknown\./,
          }).waitFor();
          await ready();
          assertEquals(posts.at(-1)!.body, frozen);
          assertEquals(await button("Stop conversation body").count(), 0);
          await capture(`${fault}-retained`, [
            `intentional ${fault}; no terminal authority`,
          ]);
          action = "accepted";
          await button("Retry same stop request").click();
          await panel().getByText("Request accepted.", { exact: true })
            .waitFor();
          await ready();
          assertEquals(
            posts.at(-1)!.body,
            frozen,
            "reconciliation never substitutes new input",
          );
        }
        permission = false;
        await refresh();
        assertEquals(await button("Stop conversation body").count(), 0);
        await capture("read-only");
        permission = true;
        observation = "unavailable";
        await refresh();
        assertEquals(await button("Start conversation body").count(), 0);
        await capture("unavailable-owner");
        // Account identity changes must not expose another account's retained retry.
        observation = "absent";
        action = "unknown";
        await refresh();
        await button("Start conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const beforeIdentitySwitch = posts.length;
        accountId = "other-account";
        await page.reload();
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        assertEquals(await button("Retry same start request").count(), 0);
        assertEquals(posts.length, beforeIdentitySwitch);
        // Ordinary back-to-list and Subject navigation must isolate the old request too.
        await page.getByRole("link", { name: "← All subjects", exact: true })
          .click();
        await page.locator('a.subject-row[href$="/stale-subject"]').click();
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        assertEquals(await button("Retry same start request").count(), 0);
        await capture("identity-subject-switched");
        await button("Start conversation body").click();
        await panel().getByText("Request outcome unknown.", { exact: true })
          .waitFor();
        await ready();
        const beforeWorkspaceSwitch = posts.length;
        await page.goto(`${base}/w/other-review/memory/stale-subject`);
        await panel().getByText("No current conversation body.", {
          exact: true,
        }).waitFor();
        await ready();
        assertEquals(await button("Retry same start request").count(), 0);
        assertEquals(posts.length, beforeWorkspaceSwitch);
        await capture("workspace-switched");
        await page.goto(`${base}/w/memory-review/memory/stale-subject`);
        await panel().getByText(/An unresolved start request is retained/)
          .waitFor();
        await ready();
        assertEquals(posts.length, beforeWorkspaceSwitch);
        getFailure = true;
        await refresh();
        await panel().getByRole("alert").waitFor();
        assert(await button("Retry same start request").isDisabled());
        await capture("get-failure", [
          "intentional 503 body GET; browser console reports failed resource",
        ]);
        const actualHttp = errors.flatMap((e) =>
          e.kind === "http"
            ? [{ status: e.status, path: new URL(e.url).pathname }]
            : []
        );
        const httpKey = (e: { status: number; path: string }) =>
          `${e.status} ${e.path}`;
        assertEquals(
          actualHttp.map(httpKey).sort(),
          expectedHttp.map(httpKey).sort(),
          "every HTTP error must match an injected status/path (including count)",
        );
        const lostRequests = errors.flatMap((e) =>
          e.kind === "request" && e.message === "net::ERR_FAILED"
            ? [new URL(e.url).pathname]
            : []
        );
        assertEquals(lostRequests.sort(), [...expectedLost].sort());
        const unexpected = errors.filter((e) => {
          if (e.kind === "request") {
            const path = new URL(e.url).pathname;
            return !((e.message === "net::ERR_ABORTED" &&
              /^\/api\/w\/(memory-review|other-review)\/workers$/.test(path)) ||
              (e.message === "net::ERR_FAILED" && expectedLost.includes(path)));
          }
          if (e.kind === "http") return false; // statusとpathの正確な件数は上で検査する。
          if (e.kind === "console") {
            const status = e.message.match(
              /^Failed to load resource: the server responded with a status of (401|409|503)\b/,
            );
            return !(status &&
              expectedHttp.some((h) => h.status === Number(status[1]))) &&
              !(e.message === "Failed to load resource: net::ERR_FAILED" &&
                expectedLost.length > 0);
          }
          return true;
        });
        assertEquals(unexpected, [], "unexpected browser diagnostics");
        manifest.assertions.push(
          `${prefix}: synthetic HTTP only; normal entry, keyboard focus/activation, missing session.record, unknown GET/reload/exact retry, pending stop, confirmed stop/restart; dedicated 409 generation/absent-start superseded and token not_accepted with GET-gated fresh explicit operations; lost stop -> exact outcome superseded -> GET/reload -> fresh operation with distinct historical wording; busy exact retry; generic409/input-conflict/401/mismatched workspace/subject/request/malformed shape retention; read-only, unavailable owner, account, Workspace and Subject isolation, GET failure`,
        );
      } finally {
        await context.close();
      }
    }
  }
  manifest.status = "pass";
} catch (error) {
  manifest.status = "fail";
  manifest.failures.push(
    error instanceof Error ? error.stack ?? error.message : String(error),
  );
  throw error;
} finally {
  await persist();
  await browser?.close();
  await stopOwnedProcesses(owned);
}
console.log(output);
