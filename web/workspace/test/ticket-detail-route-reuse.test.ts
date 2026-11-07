import { assert, assertStringIncludes } from "jsr:@std/assert";

const pageSource = await Deno.readTextFile(
  new URL(
    "../src/routes/w/[workspaceId]/tickets/[ticketId]/PageContent.svelte",
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

Deno.test("ticket child operations reset across Workspaces even when the retained Ticket id matches", () => {
  const keyedChildren = [...pageSource.matchAll(
    /\{#key JSON\.stringify\(\[data\.workspaceId, ticket\.id\]\)\}([\s\S]*?)\{\/key\}/g,
  )].map((match) => match[1]);
  for (
    const child of ["TicketTags", "TicketHandling", "TicketWorkerObservation"]
  ) {
    assert(
      keyedChildren.some((block) => block.includes(`<${child}`)),
      `${child} must reset for the Workspace and Ticket, whether in main or rail`,
    );
  }
});

Deno.test("ticket detail fences stale mutation responses", () => {
  for (
    const operation of [
      "async function mutate(",
      "async function runCaseDecision(",
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
  assertStringIncludes(pageSource, "Role assignments and manual Workers");
  assertStringIncludes(
    pageSource,
    "Assignments are retained independently of Worker state.",
  );
  assertStringIncludes(
    pageSource,
    "assignmentPrincipalLabel(assignment.principal)",
  );
  const modelSource = Deno.readTextFileSync(
    new URL("../src/lib/workspace/tickets/ticket-panel.ts", import.meta.url),
  );
  assertStringIncludes(modelSource, 'principal.kind === "subject"');
  assertStringIncludes(modelSource, "principal.subject_id");
  assertStringIncludes(modelSource, "principal.runtime_id");
  assertStringIncludes(modelSource, "principal.worker_id");
  assert(!pageSource.includes("No active role assignment."));
});
