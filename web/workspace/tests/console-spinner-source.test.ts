// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Console spinner wraps a reusable timed sequence loop", async () => {
  const sequenceLoop = await Deno.readTextFile(
    new URL(
      "../src/lib/workspace/console/SequenceLoop.svelte",
      import.meta.url,
    ),
  );
  const spinner = await Deno.readTextFile(
    new URL("../src/lib/workspace/console/Spinner.svelte", import.meta.url),
  );

  for (
    const token of ["values", "intervalMs", "setInterval", "clearInterval"]
  ) {
    assert(
      sequenceLoop.includes(token),
      `missing sequence-loop token: ${token}`,
    );
  }
  for (const frame of ["⣷", "⣯", "⣟", "⡿", "⢿", "⣻", "⣽", "⣾"]) {
    assert(spinner.includes(frame), `missing spinner frame: ${frame}`);
  }
  assert(spinner.includes("SequenceLoop"), "Spinner should wrap SequenceLoop");
});

Deno.test("running status is Composer-side above mini Tasks", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const runStatus = await Deno.readTextFile(
    new URL(
      "../src/lib/workspace/console/WorkerRunStatus.svelte",
      import.meta.url,
    ),
  );
  const status = page.indexOf("<WorkerRunStatus");
  const tasks = page.indexOf('<ConsoleTasks {tasks} mode="mini"');
  const composer = page.indexOf('<form class="console-composer card"');

  assert(status >= 0, "WorkerRunStatus should be rendered");
  assert(status < tasks, "WorkerRunStatus should be above mini Tasks");
  assert(tasks < composer, "mini Tasks should remain above Composer");
  assert(
    runStatus.includes("nowMs - (startedAtMs ?? nowMs)"),
    "running elapsed should be recomputed from timestamps",
  );
});

Deno.test("RunEnd stats render as a right-aligned Console item", async () => {
  const lineItem = await Deno.readTextFile(
    new URL(
      "../src/lib/workspace/console/ConsoleLineItem.svelte",
      import.meta.url,
    ),
  );

  for (
    const token of [
      "item.kind === 'run_stats'",
      'class="run-stats"',
      "text-align: right",
    ]
  ) {
    assert(lineItem.includes(token), `missing run stats token: ${token}`);
  }
});
