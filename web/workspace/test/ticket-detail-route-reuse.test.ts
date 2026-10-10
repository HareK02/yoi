import { assert, assertStringIncludes } from "jsr:@std/assert";

const pageSource = await Deno.readTextFile(
  new URL(
    "../src/routes/w/[workspaceId]/tickets/[ticketId]/+page.svelte",
    import.meta.url,
  ),
);

Deno.test("ticket detail synchronizes reused route data", () => {
  const effectStart = pageSource.indexOf("$effect(() => {");
  assert(effectStart >= 0, "ticket detail must react to reused route props");

  const effectSource = pageSource.slice(effectStart);
  for (
    const token of [
      "data.ticketId",
      "data.ticket.data",
      "JSON.stringify([data.workspaceId, incomingTicketId, incomingTicket])",
      "routeGeneration += 1",
      "resetTicketView(incomingTicket)",
    ]
  ) {
    assertStringIncludes(effectSource, token);
  }
});

Deno.test("ticket detail fences stale mutation responses", () => {
  for (
    const operation of [
      "async function mutate(",
      "async function queueTicket(",
      "async function mutateAssignment(",
    ]
  ) {
    const operationStart = pageSource.indexOf(operation);
    assert(operationStart >= 0, `missing ${operation}`);
    const nextOperation = pageSource.indexOf(
      "\n  async function ",
      operationStart + 1,
    );
    const operationSource = pageSource.slice(
      operationStart,
      nextOperation === -1 ? undefined : nextOperation,
    );
    assertStringIncludes(operationSource, "const generation = routeGeneration");
    assertStringIncludes(operationSource, "generation !== routeGeneration");
    assertStringIncludes(operationSource, "generation === routeGeneration");
  }
});

Deno.test("ticket responsibility display does not claim Worker liveness or work authority", () => {
  assertStringIncludes(pageSource, "Retained responsibility");
  assertStringIncludes(pageSource, "after work ends and Worker removal");
  assertStringIncludes(
    pageSource,
    "does not indicate a running Worker or active work authority",
  );
  assertStringIncludes(pageSource, "assignment.principal.runtime_id");
  assertStringIncludes(pageSource, "assignment.principal.worker_id");
  assert(!pageSource.includes("No active role assignment."));
});
