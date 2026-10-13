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
      "import Spinner from '#lib/workspace/console/Spinner.svelte'",
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
  const activityModel = await Deno.readTextFile(
    new URL("../src/lib/workspace/sidebar/workers.ts", import.meta.url),
  );
  const running = activityModel.indexOf("if (worker.state === 'running')");
  const subworker = activityModel.indexOf("if (worker.has_running_internal_workers)");
  const idle = activityModel.indexOf("if (worker.state === 'idle')");
  assert(
    running >= 0 && running < subworker,
    "parent running state should keep the green Spinner priority",
  );
  assert(
    subworker >= 0 && subworker < idle,
    "SubWorker activity should replace the idle dot with the purple Spinner",
  );
  assert(
    sidebar.includes("sidebarWorkerActivity(worker)") &&
      sidebar.includes("activity === 'worker-running'") &&
      sidebar.includes("activity === 'subworker-running'") &&
      sidebar.includes("activity === 'idle'"),
    "sidebar should render the authoritative parent and SubWorker activity projection",
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
      "../src/routes/w/[workspaceId]/workers/[workerRef]/console/PageContent.svelte",
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
  const miniMode = page.indexOf('mode="mini"');
  const tasks = page.lastIndexOf("<ConsoleTasks", miniMode);
  const composer = page.indexOf('<form class="console-composer"');

  assert(status >= 0, "WorkerRunStatus should be rendered");
  assert(composer >= 0, "Composer should be rendered");
  assert(miniMode >= 0 && tasks >= 0, "mini Tasks should be rendered");
  assert(status < tasks, "WorkerRunStatus should be above mini Tasks");
  assert(tasks < composer, "mini Tasks should remain above Composer");
  assert(
    runStatus.includes("compactionStatus?.progress.started_at_ms ?? startedAtMs") &&
      runStatus.includes("nowMs - (clockStartedAtMs ?? nowMs)"),
    "running elapsed should be recomputed from the active compaction or run timestamp",
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
