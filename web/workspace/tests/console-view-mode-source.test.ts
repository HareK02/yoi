// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Worker Console exposes Overview and Normal display modes", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/workers/[workerRef]/console/PageContent.svelte",
      import.meta.url,
    ),
  );

  for (
    const token of [
      'consoleViewMode = $state<ConsoleViewMode>("overview")',
      'role="switch"',
      'aria-checked={consoleViewMode === "overview"}',
      'title="On: Overview · Off: Normal"',
      'consoleViewMode = consoleViewMode === "overview" ? "normal" : "overview"',
      "selectedConsoleProjection = $derived(selectedWorkerView.console)",
      "mergeCommittedHistoryLines(",
      "selectedConsoleProjection.lines",
      "resolveWorkerControlShortcut",
      "handleWorkerControlShortcut",
    ]
  ) {
    assert(page.includes(token), `missing Console view-mode token: ${token}`);
  }
  assert(
    /projectConsoleLines\(\s*mergeCommittedHistoryLines\(\s*committedHistoryLines,\s*selectedConsoleProjection\.lines,?\s*\),\s*consoleViewMode,?\s*\)/.test(page),
    "the chosen main or SubWorker view must apply display mode to its merged retained and live lines",
  );
});
