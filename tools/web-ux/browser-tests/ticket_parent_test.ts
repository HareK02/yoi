import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { chromium, type Page } from "npm:playwright@1.59.1";
import { loadScenario } from "../src/scenario.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

const captureDiagnostics = new WeakMap<
  Page,
  {
    consoleErrors: string[];
    requestFailures: string[];
    documentStatuses: number[];
    httpFailures: string[];
  }
>();

// Production static shell + synthetic API: browser interaction/transport and
// rendering only. Real SQLite/Subject authorization is tested in the server suite.
Deno.test("Ticket hierarchy browser: reparent, retained retry, conflict, paging and readonly navigation", async () => {
  const scenarioPath =
    new URL("../scenarios/tickets.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  const baseline = Deno.env.get("TICKET_PARENT_BASELINE");
  const artifacts =
    new URL("../../../target/web-ux/t804/", import.meta.url).pathname;
  await Deno.mkdir(artifacts, { recursive: true });
  scenario.processes![0].args![4] = String(port);
  if (baseline) scenario.processes![0].args![5] = baseline;
  scenario.processes![0].readyUrl = `${base}/health`;
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    `${artifacts}/logs`,
    [],
  );
  const browser = await chromium.launch({ headless: true });
  const evidence: unknown[] = [];
  try {
    for (
      const [width, theme] of [[1440, "light"], [768, "light"], [
        390,
        "dark",
      ]] as const
    ) {
      const page = await browser.newPage({
        viewport: { width, height: 900 },
        colorScheme: theme,
      });
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const diagnostics = {
        consoleErrors: [] as string[],
        requestFailures: [] as string[],
        documentStatuses: [] as number[],
        httpFailures: [] as string[],
      };
      captureDiagnostics.set(page, diagnostics);
      page.on("console", (message) => {
        if (message.type() === "error") {
          diagnostics.consoleErrors.push(message.text());
        }
      });
      page.on(
        "requestfailed",
        (request) =>
          diagnostics.requestFailures.push(
            `${
              new URL(request.url()).pathname
            }: ${request.failure()?.errorText}`,
          ),
      );
      page.on("response", (response) => {
        if (response.status() >= 400) {
          diagnostics.httpFailures.push(
            `${new URL(response.url()).pathname}: ${response.status()}`,
          );
        }
        if (response.request().resourceType() === "document") {
          diagnostics.documentStatuses.push(response.status());
        }
      });
      const wire = await (await fetch(`${base}/api/w/tickets/tickets/T-1`))
        .json();
      wire.action_eligibility.can_set_parent = true;
      const parent = {
        id: "internal-T-2",
        resource_key: "T-2",
        title: "Design the data migration and recovery boundary",
        state: "open",
        closure_reason: null,
      };
      const other = {
        ...parent,
        id: "internal-T-3",
        resource_key: "T-3",
        title: "Independent delivery follow-up",
      };
      let link = {
        ticket_id: wire.id,
        observation: {
          parent_ticket_id: parent.id as string | null,
          last_operation_key: "initial" as string | null,
        },
        parent: parent as typeof parent | null,
      };
      const writes: string[] = [];
      const receipts = new Map<string, typeof link>();
      let lose = true;
      let conflict = false;
      let readonly = false;
      await page.route("**/api/w/tickets/tickets/**", async (route) => {
        const url = new URL(route.request().url());
        const addressed = decodeURIComponent(url.pathname.split("/").at(-2)!);
        const atOther = addressed === other.id ||
          addressed === other.resource_key;
        if (atOther && url.pathname.endsWith("/parent")) {
          return route.fulfill({
            json: {
              ticket_id: other.id,
              observation: { parent_ticket_id: null, last_operation_key: null },
              parent: null,
            },
          });
        }
        if (atOther && url.pathname.endsWith("/children")) {
          return route.fulfill({
            json: {
              parent_ticket_id: other.id,
              items: [{
                id: wire.id,
                resource_key: wire.resource_key,
                title: wire.title,
                state: wire.state,
                closure_reason: null,
              }],
              has_more: false,
              next_cursor: null,
            },
          });
        }
        if (url.pathname.endsWith("/link-summary")) {
          return route.fulfill({ json: other });
        }
        if (url.pathname.endsWith("/children")) {
          const next = url.searchParams.has("cursor");
          return route.fulfill({
            json: {
              parent_ticket_id: wire.id,
              items: [{
                ...parent,
                id: next ? "child-2" : "child-1",
                resource_key: next ? "T-12" : "T-11",
                title: next
                  ? "Follow-up deliberately not planned"
                  : "A child closed as duplicate, not success",
                state: "closed",
                closure_reason: next ? "not_planned" : "duplicate",
              }],
              has_more: !next,
              next_cursor: next ? null : "opaque-next",
            },
          });
        }
        if (url.pathname.endsWith("/parent")) {
          if (route.request().method() !== "PUT") {
            return route.fulfill({ json: link });
          }
          const body = route.request().postData()!;
          writes.push(body);
          const input = JSON.parse(body);
          if (conflict) {
            return route.fulfill({
              status: 409,
              json: { message: "stale parent observation" },
            });
          }
          if (!receipts.has(input.operation_key)) {
            assertEquals(input.expected, link.observation);
            link = {
              ticket_id: wire.id,
              observation: {
                parent_ticket_id: input.parent,
                last_operation_key: input.operation_key,
              },
              parent: input.parent === null ? null : other,
            };
            receipts.set(input.operation_key, structuredClone(link));
          }
          if (lose) {
            lose = false;
            return route.abort("failed");
          }
          return route.fulfill({ json: receipts.get(input.operation_key) });
        }
        if (/\/tickets\/T-1(?:-|$)/.test(url.pathname)) {
          const response = {
            ...wire,
            parent_link: link,
            action_eligibility: {
              ...wire.action_eligibility,
              can_set_parent: !readonly,
            },
          };
          if (baseline) {
            Reflect.deleteProperty(response, "parent_link");
            Reflect.deleteProperty(
              response.action_eligibility,
              "can_set_parent",
            );
          }
          return route.fulfill({ json: response });
        }
        return route.continue();
      });
      await page.route("**/api/auth/whoami", (route) =>
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
              handle: "alice",
              display_name: "Alice",
              auth_method: "browser_session",
            },
          },
        }));
      await page.route(
        "**/api/hosting/access",
        (route) => route.fulfill({ json: { is_admin: false } }),
      );
      await page.goto(`${base}/w/tickets/tickets/T-1`);
      await page.locator(".ticket-timeline").waitFor().catch(async (error) => {
        console.error(await page.locator("body").innerText(), errors);
        await page.screenshot({
          path: `${artifacts}/failure.png`,
          fullPage: true,
        });
        throw error;
      });
      if (baseline) {
        await capture(
          page,
          `${artifacts}/before-${width}-${theme}.png`,
          evidence,
          errors,
        );
        await page.close();
        continue;
      }
      const hierarchy = page.getByRole("region", {
        name: "Parent and children",
      });
      await hierarchy.getByRole("link", { name: /T-2 Design/ }).waitFor();
      assert((await hierarchy.innerText()).includes("closed · duplicate"));
      await hierarchy.scrollIntoViewIfNeeded();
      await capture(
        page,
        `${artifacts}/after-${width}-${theme}.png`,
        evidence,
        errors,
      );
      await hierarchy.getByText("Change or unlink parent", { exact: true })
        .click();
      await hierarchy.getByLabel("Ticket reference", { exact: true }).fill(
        "T-3",
      );
      await hierarchy.getByRole("button", {
        name: "Find Ticket by reference",
        exact: true,
      })
        .click();
      await hierarchy.getByRole("radio", { name: /T-3/ }).check();
      await hierarchy.getByRole("button", {
        name: "Set selected parent",
        exact: true,
      }).click();
      const retry = hierarchy.getByRole("button", {
        name: "Retry exact parent operation",
        exact: true,
      });
      await retry.waitFor();
      await hierarchy.getByRole("alert").filter({
        hasText: /Outcome unavailable or unknown/,
      })
        .waitFor();
      assert(
        await hierarchy.getByRole("button", {
          name: "Set selected parent",
          exact: true,
        })
          .isDisabled(),
      );
      await capture(
        page,
        `${artifacts}/unknown-${width}-${theme}.png`,
        evidence,
        errors,
      );
      await retry.click();
      await hierarchy.getByText(
        "Parent operation saved. Current parent is read separately.",
        {
          exact: true,
        },
      ).waitFor();
      assertEquals(writes[1], writes[0]);
      await hierarchy.getByRole("link", { name: /T-3 Independent/ }).waitFor();
      await hierarchy.getByText("Change or unlink parent", { exact: true })
        .click();
      await hierarchy.getByRole("button", {
        name: "Unlink parent",
        exact: true,
      }).click();
      await hierarchy.getByText("No parent.", { exact: true }).waitFor();
      assertEquals(link.observation.parent_ticket_id, null);
      await hierarchy.getByText("Set parent", { exact: true }).click();
      await hierarchy.getByLabel("Ticket reference", { exact: true }).fill(
        "T-3",
      );
      await hierarchy.getByRole("button", {
        name: "Find Ticket by reference",
        exact: true,
      })
        .click();
      await hierarchy.getByRole("radio", { name: /T-3/ }).check();
      await hierarchy.getByRole("button", {
        name: "Set selected parent",
        exact: true,
      }).click();
      await hierarchy.getByRole("link", { name: /T-3 Independent/ }).waitFor();
      await hierarchy.getByRole("button", {
        name: "Load more children",
        exact: true,
      }).click();
      await hierarchy.getByRole("link", { name: /T-12/ }).waitFor();
      conflict = true;
      await hierarchy.getByText("Change or unlink parent", { exact: true })
        .click();
      await hierarchy.getByRole("button", {
        name: "Unlink parent",
        exact: true,
      }).click();
      await hierarchy.getByRole("alert").filter({
        hasText: /Parent update conflict/,
      }).waitFor();
      assertEquals(link.observation.parent_ticket_id, other.id);
      await capture(
        page,
        `${artifacts}/conflict-${width}-${theme}.png`,
        evidence,
        errors,
      );
      readonly = true;
      await page.reload();
      await hierarchy.getByRole("link", { name: /T-3 Independent/ }).waitFor();
      assertEquals(
        await hierarchy.getByRole("button", {
          name: "Unlink parent",
          exact: true,
        }).count(),
        0,
      );
      assertEquals(
        await hierarchy.getByRole("button", {
          name: "Retry exact parent operation",
          exact: true,
        })
          .count(),
        0,
      );
      const href = await hierarchy.getByRole("link", {
        name: /T-3 Independent/,
      }).getAttribute(
        "href",
      );
      assert(href?.includes("/tickets/T-3"));
      await capture(
        page,
        `${artifacts}/readonly-${width}-${theme}.png`,
        evidence,
        errors,
      );
      await hierarchy.getByRole("link", { name: /T-3 Independent/ }).click();
      await page.waitForURL((url) => url.pathname.includes("/tickets/T-3"));
      await hierarchy.getByText("No parent.", { exact: true }).waitFor();
      await hierarchy.getByRole("link", { name: new RegExp(wire.resource_key) })
        .click();
      await page.waitForURL((url) =>
        url.pathname.includes(`/tickets/${wire.resource_key}`)
      );
      await hierarchy.getByRole("link", { name: /T-3 Independent/ }).waitFor();
      assertEquals(errors, []);
      await page.close();
    }
  } finally {
    await Deno.writeTextFile(
      `${artifacts}/${baseline ? "before" : "after"}-review-context.json`,
      JSON.stringify(
        {
          boundary:
            "synthetic API, production static shell; not Backend authorization evidence",
          captures: evidence,
        },
        null,
        2,
      ),
    );
    await browser.close();
    await stopOwnedProcesses(processes);
  }
});

async function capture(
  page: Page,
  path: string,
  evidence: unknown[],
  errors: string[],
) {
  const heading = page.getByRole("heading", {
    name: "Parent and children",
    exact: true,
  });
  if (await heading.count()) {
    await heading.evaluate((element) =>
      element.scrollIntoView({ block: "start", behavior: "instant" })
    );
  }
  await page.screenshot({ path, fullPage: true });
  const hash = await crypto.subtle.digest("SHA-256", await Deno.readFile(path));
  evidence.push({
    path,
    url: page.url(),
    status: await page.title(),
    viewport: page.viewportSize(),
    visibleAlerts: await page.getByRole("alert").allTextContents(),
    pageErrors: [...errors],
    ...structuredClone(captureDiagnostics.get(page)),
    aria: await page.locator("body").ariaSnapshot(),
    sha256: Array.from(
      new Uint8Array(hash),
      (b) => b.toString(16).padStart(2, "0"),
    ).join(""),
  });
}
