// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Worker Console exposes Overview and Normal display modes", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );

  for (
    const token of [
      'consoleViewMode = $state<ConsoleViewMode>("overview")',
      'aria-label="Console display mode"',
      'consoleViewMode = "overview"',
      'consoleViewMode = "normal"',
      "projectConsoleLines(consoleProjection.lines, consoleViewMode)",
      "projectConsoleLines(internal.console.lines, consoleViewMode)",
      "resolveWorkerControlShortcut",
      "handleWorkerControlShortcut",
    ]
  ) {
    assert(page.includes(token), `missing Console view-mode token: ${token}`);
  }
});
