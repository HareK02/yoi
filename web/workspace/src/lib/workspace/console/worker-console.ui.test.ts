declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: string | URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

Deno.test("workspace Worker list lives on the dedicated Workers page", async () => {
  const workspacePage = await Deno.readTextFile(
    new URL("./../../../routes/w/[workspaceId]/+page.svelte", import.meta.url),
  );
  const workersPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/workers/+page.svelte",
      import.meta.url,
    ),
  );
  const workersNav = await Deno.readTextFile(
    new URL("../sidebar/WorkersNavSection.svelte", import.meta.url),
  );
  const sidebar = await Deno.readTextFile(
    new URL("../sidebar/WorkspaceSidebar.svelte", import.meta.url),
  );

  assert(
    workspacePage.includes("href={`/w/${workspaceId}/runtimes`}") &&
      workspacePage.includes("href={`/w/${workspaceId}/workers`}"),
    "top workspace page should link to Runtimes and Workers pages",
  );
  assert(
    !workspacePage.includes("workerConsoleHref") &&
      !workspacePage.includes("Open Console"),
    "top workspace page should not own the Worker list",
  );
  assert(
    workersPage.includes("workerConsoleHref(worker, data.workspaceId)") &&
      workersPage.includes('<table class="workers-table">') &&
      workersPage.includes('class="icon-action"') &&
      workersPage.includes('Delete ${worker.label}'),
    "dedicated Workers page should expose a table, console link target, and icon actions per Worker",
  );
  assert(
    workersNav.includes("href={`/w/${workspaceId}/workers`}") &&
      workersNav.includes("filter(canShowWorkerInSidebar)") &&
      !workersNav.includes('aria-disabled="true"'),
    "Workers sidebar should link to the Worker list page and omit registry-only Workers",
  );
  assert(
    !sidebar.includes("CompanionNavSection") &&
      sidebar.includes("WorkersNavSection"),
    "standalone Companion/Console navigation should not remain canonical",
  );
});

Deno.test("Worker Console uses protocol observation events without transcript fetch", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );

  assert(
    consolePage.includes("connectProtocolTransport") &&
      consolePage.includes("handleIncomingProtocolEvent") &&
      consolePage.includes("/protocol/ws") &&
      !consolePage.includes("seenObservationEventIds") &&
      consolePage.includes("createConsoleProjector") &&
      consolePage.includes("consoleProjector.append(eventBatch)") &&
      consolePage.includes("{#each lines as item (item.id)}") &&
      !consolePage.includes("projectConsole(observedEvents.map") &&
      !consolePage.includes("/transcript") &&
      !consolePage.includes("WorkerTranscriptProjection"),
    "Console should render raw protocol replay/live events directly from the unified protocol WS",
  );
});

Deno.test("Worker Console renders markdown only for message rows", async () => {
  const consoleLine = await Deno.readTextFile(
    new URL("./ConsoleLineItem.svelte", import.meta.url),
  );

  assert(
    consoleLine.includes("function shouldRenderMarkdown") &&
      consoleLine.includes("item.kind === 'tool'") &&
      consoleLine.includes('<p class="console-plain-text">{bodyTextAfterToolSummary(item)}</p>') &&
      consoleLine.includes("{:else if shouldRenderMarkdown(item)}") &&
      consoleLine.includes("<RichMarkdown text={item.body || '—'} />"),
    "Console should keep markdown rendering to user/assistant/system message bodies and render tool text literally",
  );
});

Deno.test("Worker Console renders Edit diffs without preformatted template gaps", async () => {
  const consoleLine = await Deno.readTextFile(
    new URL("./ConsoleLineItem.svelte", import.meta.url),
  );

  assert(
    consoleLine.includes('<div class="console-diff" role="group" aria-label="Edit diff">') &&
      consoleLine.includes("{#each item.diff as diffLine}") &&
      consoleLine.includes("class={`diff-line ${diffLine.kind}`}") &&
      !consoleLine.includes('<pre class="console-diff"'),
    "Edit diff rows should not be wrapped in a pre element that preserves template whitespace as blank lines",
  );
});

Deno.test("Worker Console exposes a foldable timeline beside the scroll body", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const consoleLine = await Deno.readTextFile(
    new URL("./ConsoleLineItem.svelte", import.meta.url),
  );
  const consoleTimeline = await Deno.readTextFile(
    new URL("./ConsoleTimeline.svelte", import.meta.url),
  );
  assert(
    consoleTimeline.includes('class="console-timeline"') &&
      consolePage.includes("timelineMarks") &&
      consolePage.includes("jumpToTimelineMark") &&
      consoleLine.includes("data-console-line-id={item.id}") &&
      consolePage.includes("class:timeline-open={timelineOpen}") &&
      consolePage.includes("class=\"timeline-fold\"") &&
      consolePage.includes("expanded={timelineOpen}") &&
      !consolePage.includes("{#if timelineOpen}") &&
      consolePage.includes("handleTimelineRailPointerDown") &&
      consolePage.includes("projectTimelineAxisPosition") &&
      consolePage.includes("scrollbar-width: none") &&
      consoleTimeline.includes("onpointerdown={onRailPointerDown}") &&
      consoleTimeline.includes("expanded ? 'expanded' : 'folded'") &&
      consoleTimeline.includes(".timeline-mark.expanded .timeline-card") &&
      !consoleTimeline.includes("{#if expanded}") &&
      consoleTimeline.includes(".timeline-thumb") &&
      consoleTimeline.includes(".timeline-card"),
    "Worker Console should expose a foldable timeline with scroll and line jump markers",
  );
});

Deno.test("Worker Console composer fits to content without manual resize", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  assert(
    consolePage.includes("use:fitTextarea={{ value: draft, maxRows: 10 }}") &&
      consolePage.includes("<div class=\"composer-input-shell\">") &&
      !consolePage.includes("handleComposerShellClick") &&
      consolePage.includes("bind:this={composerTextareaElement}") &&
      consolePage.includes('event.key === "PageUp" || event.key === "PageDown"') &&
      consolePage.includes("scrollConsoleByPage") &&
      consolePage.includes("class=\"composer-input-footer\"") &&
      consolePage.includes("pointer-events: none") &&
      consolePage.includes("class=\"composer-footer-slot\"") &&
      consolePage.includes("class=\"composer-send-button\"") &&
      consolePage.includes("pointer-events: auto") &&
      consolePage.includes("class=\"composer-send-icon\"") &&
      consolePage.includes('d="M8 6L12 2L16 6"') &&
      consolePage.includes(".console-composer textarea") &&
      consolePage.includes("resize: none") &&
      consolePage.includes("overflow-y: hidden"),
    "Console composer should autosize to content, cap at ten rows, wrap input and icon send button, and disable manual resize",
  );
});

Deno.test("Decodal source editor keeps imperative EditorView out of reactive state", async () => {
  const editor = await Deno.readTextFile(
    new URL(
      "../settings/DecodalSourceEditor.svelte",
      import.meta.url,
    ),
  );

  assert(
    editor.includes("let view: EditorView | null = null") &&
      !editor.includes("$state<EditorView") &&
      editor.includes("untrack(() => value)") &&
      editor.includes("untrack(() => onChange)"),
    "CodeMirror EditorView must not be reactive state; otherwise mount cleanup can loop forever",
  );
});

Deno.test("workspace Runtime management pages expose Runtimes and Runtime-owned workdirs", async () => {
  const sidebar = await Deno.readTextFile(
    new URL("../sidebar/WorkspaceSidebar.svelte", import.meta.url),
  );
  const runtimesNav = await Deno.readTextFile(
    new URL("../sidebar/RuntimesNavSection.svelte", import.meta.url),
  );
  const runtimesPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/+page.svelte",
      import.meta.url,
    ),
  );
  const workdirsPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workdirs/+page.svelte",
      import.meta.url,
    ),
  );
  const workdirsLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workdirs/+page.ts",
      import.meta.url,
    ),
  );

  assert(
    sidebar.includes("RuntimesNavSection") &&
      runtimesNav.includes("href={runtimesHref}") &&
      runtimesNav.includes("/runtimes"),
    "sidebar should expose Runtime management navigation",
  );
  assert(
    runtimesPage.includes("Open workdirs") &&
      runtimesPage.includes("runtimes-table") &&
      runtimesPage.includes("/workdirs"),
    "Runtimes page should table Runtimes and link to each Runtime's workdirs",
  );
  assert(
    workdirsPage.includes("Workdirs") &&
      workdirsPage.includes("workdirs-table") &&
      workdirsLoad.includes("/working-directories"),
    "Workdirs page should read Runtime-owned working-directory API while using workdir UI language",
  );
});

Deno.test("workspace Worker sidebar links New to the dedicated create page", async () => {
  const workersNav = await Deno.readTextFile(
    new URL("../sidebar/WorkersNavSection.svelte", import.meta.url),
  );
  const newWorkerPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/workers/new/+page.svelte",
      import.meta.url,
    ),
  );

  assert(
    workersNav.includes("href={`/w/${workspaceId}/workers/new`}") &&
      !workersNav.includes("worker-launch-form") &&
      !workersNav.includes("createWorker()"),
    "Workers sidebar should link to the dedicated New Worker page instead of owning the form",
  );
  assert(
    newWorkerPage.includes("worker-launch-form") &&
      newWorkerPage.includes("buildBrowserCreateWorkerRequest") &&
      newWorkerPage.includes("/workers/launch-options"),
    "New Worker page should own launch options and creation form behavior",
  );
});

Deno.test("Worker Console page is routed by runtime_id and worker_id through backend APIs", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const routeLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.ts",
      import.meta.url,
    ),
  );

  assert(
    routeLoad.includes("workspaceId") &&
      routeLoad.includes("runtimeId") && routeLoad.includes("workerId"),
    "route load should expose workspace and target ids",
  );
  assert(
    consolePage.includes("workspaceApiPath(workspaceId, path)") &&
      consolePage.includes("workerApiPath(") &&
      consolePage.includes("`/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(") &&
      consolePage.includes("target.workerId"),
    "Worker detail should use the scoped backend Worker detail API",
  );
  assert(
    !consolePage.includes("/transcript") &&
      consolePage.includes("/protocol/ws") &&
      !consolePage.includes("/events" + "/ws") &&
      !consolePage.includes("/input") &&
      !consolePage.includes("/completions"),
    "Console should use only the protocol WS for observation and operations",
  );
  assert(
    !consolePage.includes("/api/companion"),
    "Console page must not use Companion-specific APIs",
  );
  assert(
    consolePage.includes("function advanceReloadToken()") &&
      consolePage.includes("nextReloadToken += 1") &&
      !consolePage.includes("reloadToken += 1"),
    "reload token advancement should not synchronously read and write the rune state",
  );
  assert(
    consolePage.includes("advanceReloadToken();") &&
      consolePage.includes("void loadConsoleData(target);") &&
      !consolePage.includes("void refreshConsole();\n  });\n\n  $effect"),
    "target-change effect should load data without depending on manual refresh state reads",
  );
});
