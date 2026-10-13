import { assert, assertEquals } from "jsr:@std/assert@1.0.19";
import { join } from "@std/path";
import { chromium } from "npm:playwright@1.59.1";
import { loadScenario } from "../src/scenario.ts";
import { startOwnedProcesses, stopOwnedProcesses } from "../src/processes.ts";

// Native select and confirmed replacement against the loopback browser fixture.
// happy-dom does not match selected options with :checked (used by Svelte's
// select binding); do not substitute fake component state for this boundary.
Deno.test("principal replacement snapshots the target and observed assignment for exact unknown-outcome retry", async () => {
  const scenarioPath = new URL("../scenarios/tickets.json", import.meta.url).pathname;
  const scenario = await loadScenario(scenarioPath);
  const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 });
  const port = (listener.addr as Deno.NetAddr).port;
  listener.close();
  const base = `http://127.0.0.1:${port}`;
  scenario.processes![0].args![4] = String(port);
  scenario.processes![0].readyUrl = `${base}/health`;
  const processes = await startOwnedProcesses(
    scenario.processes!,
    scenarioPath,
    join(Deno.env.get("WEB_UX_TEST_ARTIFACTS") ?? new URL("../../../target/web-ux", import.meta.url).pathname, "t743-principal", String(port)),
    [],
  );
  let browser: Awaited<ReturnType<typeof chromium.launch>> | null = null;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    await page.goto(`${base}/w/tickets/tickets/T-2`);
    await page.getByText("Self-reported handling: waiting", { exact: true }).waitFor();
    await page.getByText("Choose responsibility principal", { exact: true }).click();
    await page.getByRole("combobox", { name: "Principal kind", exact: true }).selectOption("user");
    await page.getByLabel("Account identity", { exact: true }).fill("alice");
    await page.getByText("Replace this delegation", { exact: true }).click();
    const sent: string[] = [];
    await page.route("**/assignments/orchestrator", async (route) => {
      if (route.request().method() === "PUT") {
        sent.push(route.request().postData()!);
        await route.abort("failed");
      } else await route.continue();
    });
    page.once("dialog", (dialog) => dialog.accept());
    await page.getByRole("button", { name: "Replace assignment", exact: true }).click();
    await page.getByRole("alert").filter({ hasText: "Outcome unknown" }).waitFor();
    assertEquals(sent.length, 1);
    const input = JSON.parse(sent[0]);
    assertEquals(input.principal, { kind: "user", account_id: "alice" });
    assertEquals(input.expected_assignment_id, "assignment-T-2");
    assert(typeof input.operation_id === "string" && input.operation_id.length > 0);
    assert(await page.getByRole("combobox", { name: "Principal kind", exact: true }).isDisabled());
    const retryFailed = page.waitForEvent(
      "requestfailed",
      (request) =>
        request.method() === "PUT" && request.url().endsWith("/assignments/orchestrator"),
    );
    await page.getByRole("button", { name: "Retry exact operation", exact: true }).click();
    // The alert text is unchanged on retry; synchronize on the actual second
    // request failure, not a fixed delay or an unrelated Worker state.
    await retryFailed;
    assertEquals(sent.length, 2);
    assertEquals(sent[1], sent[0]);
  } finally {
    await browser?.close();
    await stopOwnedProcesses(processes);
  }
});
