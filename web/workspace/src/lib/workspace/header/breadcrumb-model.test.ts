import { buildWorkspaceBreadcrumbs } from "./breadcrumb-model.ts";

declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
};

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

Deno.test("Ticket detail breadcrumbs expose the Ticket list and current id", () => {
  assertEquals(
    buildWorkspaceBreadcrumbs("/w/workspace/tickets/TICKET-42", "workspace"),
    [
      { label: "tickets", href: "/w/workspace/tickets" },
      { label: "TICKET-42" },
    ],
  );
});

Deno.test("Worker console breadcrumbs use the logical Workers route and display name", () => {
  assertEquals(
    buildWorkspaceBreadcrumbs(
      "/w/workspace/runtimes/runtime-a/workers/worker-7/console",
      "workspace",
      { workerName: "Review Worker" },
    ),
    [
      { label: "workers", href: "/w/workspace/workers" },
      { label: "Review Worker" },
    ],
  );
});

Deno.test("Worker console breadcrumbs fall back to Worker id", () => {
  assertEquals(
    buildWorkspaceBreadcrumbs(
      "/w/workspace/runtimes/runtime-a/workers/worker-7/console",
      "workspace",
    ),
    [
      { label: "workers", href: "/w/workspace/workers" },
      { label: "worker-7" },
    ],
  );
});
