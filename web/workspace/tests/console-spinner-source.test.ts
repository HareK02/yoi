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

Deno.test("sidebar running status reuses the green symbol spinner", async () => {
  const sidebar = await Deno.readTextFile(
    new URL(
      "../src/lib/workspace/sidebar/WorkersNavSection.svelte",
      import.meta.url,
    ),
  );
  const sidebarCss = await Deno.readTextFile(
    new URL(
      "../src/lib/workspace/sidebar/sidebar.css",
      import.meta.url,
    ),
  );

  assert(
    sidebar.includes(
      "import Spinner from '$lib/workspace/console/Spinner.svelte'",
    ),
    "Workers sidebar should import the reusable symbol Spinner",
  );
  assert(
    sidebar.includes('<Spinner label="Running" />'),
    "running Workers should render the reusable symbol Spinner",
  );
  assert(
    sidebarCss.includes("--spinner-color: var(--success)"),
    "sidebar spinner should use the green success token",
  );
  assert(
    sidebar.indexOf("worker.state === 'running'") <
      sidebar.indexOf("worker.has_running_internal_workers"),
    "parent running state should keep the green Spinner priority",
  );
  assert(
    sidebar.indexOf("worker.has_running_internal_workers") <
      sidebar.indexOf("worker.state === 'idle'"),
    "SubWorker activity should replace the idle dot with the purple Spinner",
  );
  assert(
    sidebar.includes("worker.has_running_internal_workers"),
    "idle parents should render SubWorker activity from the Workspace projection",
  );
  assert(
    sidebar.includes('<Spinner label="SubWorker running" />'),
    "running SubWorkers should use the reusable symbol Spinner",
  );
  assert(
    sidebarCss.includes("--spinner-color: var(--tui-magenta)"),
    "SubWorker spinner should use the purple TUI token",
  );
  assert(
    !sidebarCss.includes("@keyframes worker-status-spin"),
    "legacy rotating ring spinner should be removed",
  );
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
