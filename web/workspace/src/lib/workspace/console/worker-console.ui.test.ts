declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: string | URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

Deno.test("workspace app css uses bundled UI fonts", async () => {
  const appCss = await Deno.readTextFile(
    new URL("./../../../app.css", import.meta.url),
  );
  assert(
    !appCss.includes('"tailwindcss"'),
    "global app css should not import Tailwind defaults",
  );
  assert(
    appCss.includes("gen-interface-jp/400.css") &&
      appCss.includes("gen-interface-jp/500.css") &&
      appCss.includes("gen-interface-jp/600.css") &&
      appCss.includes("gen-interface-jp/700.css") &&
      appCss.includes('--font-sans: "Gen Interface JP", sans-serif;') &&
      !appCss.includes("ui-sans-serif") &&
      !appCss.includes("system-ui"),
    "global app css should import the tracked Gen Interface JP weights and prefer it in the sans font stack",
  );
  assert(
    appCss.includes("@fontsource/ibm-plex-mono/latin-400.css") &&
      appCss.includes("@fontsource/ibm-plex-mono/latin-500.css") &&
      appCss.includes("@fontsource/ibm-plex-mono/latin-600.css") &&
      appCss.includes("@fontsource/ibm-plex-mono/latin-700.css") &&
      appCss.includes('--font-mono: "IBM Plex Mono", monospace;') &&
      !appCss.includes("ui-monospace") &&
      !appCss.includes("SFMono-Regular"),
    "global app css should import the tracked IBM Plex Mono weights and prefer it in the mono font stack",
  );
});

Deno.test("workspace feature css is owned outside app css", async () => {
  const appCss = await Deno.readTextFile(
    new URL("./../../../app.css", import.meta.url),
  );
  const workspaceLayout = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/+layout.svelte",
      import.meta.url,
    ),
  );
  const settingsLayout = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/settings/+layout.svelte",
      import.meta.url,
    ),
  );
  const accountPage = await Deno.readTextFile(
    new URL("./../../../routes/account/+page.svelte", import.meta.url),
  );
  const deviceLoginPage = await Deno.readTextFile(
    new URL("./../../../routes/login/device/+page.svelte", import.meta.url),
  );

  assert(
    !appCss.includes(".workspace-actions") &&
      !appCss.includes(".worker-new-page") &&
      !appCss.includes(".settings-page"),
    "app.css should not own workspace, worker, or settings page implementation classes",
  );
  assert(
    workspaceLayout.includes("$lib/workspace/styles/workspace-pages.css") &&
      workspaceLayout.includes("$lib/workspace/styles/workers.css") &&
      settingsLayout.includes("$lib/workspace/styles/settings.css") &&
      accountPage.includes("$lib/workspace/styles/settings.css") &&
      deviceLoginPage.includes("$lib/workspace/styles/settings.css"),
    "feature-owned CSS should be imported by the layouts/pages that need it",
  );
});

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
    workspacePage.includes("ticketsHref") &&
      workspacePage.includes("runtimeSettingsHref") &&
      workspacePage.includes("workersHref") &&
      workspacePage.includes("workspaceRoute(workspaceId, '/tickets')") &&
      workspacePage.includes(
        "workspaceRoute(workspaceId, '/settings/runtimes')",
      ) &&
      workspacePage.includes("workspaceRoute(workspaceId, '/workers')"),
    "top workspace page should link to Tickets, Runtime Inventory under Settings, and the Workers page",
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
      workersPage.includes("Delete ${worker.label}"),
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
      sidebar.includes("TicketsNavSection") &&
      sidebar.includes("MemoryNavSection") &&
      sidebar.includes("WorkersNavSection"),
    "standalone Companion/Console navigation should not remain canonical and Tickets should be primary workspace navigation",
  );
});

Deno.test("workspace Tickets surface uses read-only Backend Ticket APIs", async () => {
  const ticketsNav = await Deno.readTextFile(
    new URL("../sidebar/TicketsNavSection.svelte", import.meta.url),
  );
  const ticketsLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/tickets/+page.ts",
      import.meta.url,
    ),
  );
  const ticketsPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/tickets/+page.svelte",
      import.meta.url,
    ),
  );
  const ticketDetailLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/tickets/[ticketId]/+page.ts",
      import.meta.url,
    ),
  );
  const ticketDetailPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/tickets/[ticketId]/+page.svelte",
      import.meta.url,
    ),
  );

  assert(
    ticketsNav.includes("workspaceRoute(workspaceId, '/tickets')") &&
      ticketsNav.includes("Open Tickets"),
    "Tickets sidebar section should link to the workspace Tickets surface",
  );
  assert(
    ticketsLoad.includes(
      '`${workspaceApiPath(params.workspaceId, "/tickets")}?limit=1000`',
    ) &&
      ticketsPage.includes("Notion-style filtering and sorting") &&
      ticketsPage.includes("toggleSort('updated_at')") &&
      ticketsPage.includes("bind:value={visibilityFilter}") &&
      ticketsPage.includes("sortKey = $state<SortKey>('panel')") &&
      ticketsPage.includes("workspace_action_priority") &&
      ticketsPage.includes("bind:value={stateFilter}") &&
      ticketsPage.includes(
        "workspaceRoute(data.workspaceId, `/tickets/${ticket.id}`)",
      ),
    "Tickets list should read the workspace-scoped Ticket API and expose sortable/filterable table links",
  );
  assert(
    ticketDetailLoad.includes("`/tickets/${encodeURIComponent(ticketId)}`") &&
      ticketDetailPage.includes("event_count") &&
      ticketDetailPage.includes("artifact_count") &&
      ticketDetailPage.includes("<pre>{data.ticket.data.body"),
    "Ticket detail should read one Ticket record and expose body plus metadata without mutation controls",
  );
});

Deno.test("workspace Memory surfaces use read-only scoped memory APIs", async () => {
  const memoryNav = await Deno.readTextFile(
    new URL("../sidebar/MemoryNavSection.svelte", import.meta.url),
  );
  const memoryDocumentLoad = await Deno.readTextFile(
    new URL("./../../../routes/w/[workspaceId]/memory/+page.ts", import.meta.url),
  );
  const memoryDocumentPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/memory/+page.svelte",
      import.meta.url,
    ),
  );
  const memoryStagingLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/memory/staging/+page.ts",
      import.meta.url,
    ),
  );
  const memoryStagingPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/memory/staging/+page.svelte",
      import.meta.url,
    ),
  );

  assert(
    memoryNav.includes("workspaceRoute(workspaceId, '/memory')") &&
      memoryNav.includes("durable workspace memory") &&
      memoryNav.includes("workspaceRoute(workspaceId, '/memory/staging')") &&
      memoryNav.includes("pending extraction candidates"),
    "Memory sidebar section should link to Document and Staging surfaces",
  );
  assert(
    memoryDocumentLoad.includes("workspaceApiPath(params.workspaceId") &&
      memoryDocumentLoad.includes('"/memory"') &&
      memoryDocumentPage.includes("Memory Document") &&
      memoryDocumentPage.includes("This view is read-only") &&
      memoryDocumentPage.includes("data.memory.data.body_md") &&
      memoryDocumentPage.includes("data.memory.data.updated_at"),
    "Memory Document page should read the scoped API and expose the durable document without mutation controls",
  );
  assert(
    memoryStagingLoad.includes("workspaceApiPath(params.workspaceId") &&
      memoryStagingLoad.includes('"/memory/staging"') &&
      memoryStagingPage.includes("Memory Staging") &&
      memoryStagingPage.includes("Workspace Server memory authority") &&
      memoryStagingPage.includes("data.staging.data.invalid_count") &&
      memoryStagingPage.includes("entry.record.evidence"),
    "Memory Staging page should read the scoped API and expose staged records without mutation controls",
  );
});

Deno.test("root layout does not keep legacy unscoped route compatibility", async () => {
  const layoutLoad = await Deno.readTextFile(
    new URL("./../../../routes/+layout.ts", import.meta.url),
  );

  assert(
    !layoutLoad.includes("scopedCompatibilityRoute") &&
      !layoutLoad.includes('pathname === "/runtimes"') &&
      !layoutLoad.includes("return workspaceRoute(workspaceId, pathname)") &&
      layoutLoad.includes("workspaceRoute(workspace.data.workspace_id)"),
    "root layout should bootstrap the workspace entry only, not preserve legacy unscoped routes",
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
      consoleLine.includes(
        '<p class="console-plain-text">{bodyTextAfterToolSummary(item)}</p>',
      ) &&
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
    consoleLine.includes(
      '<div class="console-diff" role="group" aria-label="Edit diff">',
    ) &&
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
      consolePage.includes('class="timeline-fold"') &&
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
      consolePage.includes('<div class="composer-input-shell">') &&
      !consolePage.includes("handleComposerShellClick") &&
      consolePage.includes("bind:this={composerTextareaElement}") &&
      consolePage.includes(
        'event.key === "PageUp" || event.key === "PageDown"',
      ) &&
      consolePage.includes("scrollConsoleByPage") &&
      consolePage.includes('class="composer-input-footer"') &&
      consolePage.includes("pointer-events: none") &&
      consolePage.includes('class="composer-footer-slot"') &&
      consolePage.includes('class="composer-send-button"') &&
      consolePage.includes("pointer-events: auto") &&
      consolePage.includes('class="composer-send-icon"') &&
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

Deno.test("workspace Runtime inventory lives under Settings admin routes", async () => {
  const sidebar = await Deno.readTextFile(
    new URL("../sidebar/WorkspaceSidebar.svelte", import.meta.url),
  );
  const settingsModel = await Deno.readTextFile(
    new URL("../settings/model.ts", import.meta.url),
  );
  const runtimesPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/settings/runtimes/+page.svelte",
      import.meta.url,
    ),
  );
  const workdirsPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/settings/runtimes/[runtimeId]/workdirs/+page.svelte",
      import.meta.url,
    ),
  );
  const workdirsLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/settings/runtimes/[runtimeId]/workdirs/+page.ts",
      import.meta.url,
    ),
  );

  assert(
    !sidebar.includes("RuntimesNavSection") &&
      settingsModel.includes('id: "runtime-inventory"') &&
      settingsModel.includes("return `${SETTINGS_ROUTE}/runtimes`;"),
    "Runtime inventory should be admin Settings navigation, not primary workspace sidebar navigation",
  );
  assert(
    runtimesPage.includes("Runtime Inventory") &&
      runtimesPage.includes("Open workdirs") &&
      runtimesPage.includes("runtimes-table") &&
      runtimesPage.includes(
        "/settings/runtimes/${encodeURIComponent(runtime.runtime_id)}/workdirs",
      ),
    "Settings Runtime Inventory page should table Runtimes and link to each Runtime's workdirs",
  );
  assert(
    workdirsPage.includes("Runtime Inventory") &&
      workdirsPage.includes("workdirs-table") &&
      workdirsLoad.includes("/working-directories"),
    "Runtime workdirs should remain backed by Runtime APIs without legacy Runtime route redirects",
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
      consolePage.includes(
        "`/runtimes/${encodeURIComponent(target.runtimeId)}/workers/${encodeURIComponent(",
      ) &&
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
  assert(
    consolePage.includes(
      'const workerRunning = $derived(workerState === "running");',
    ) &&
      consolePage.includes(
        'const composerEditable = $derived(protocolState === "open" && !sending);',
      ) &&
      consolePage.includes('sendControl({ method: "cancel" }, "Stop")') &&
      consolePage.includes("enabled: canSubmitDraft") &&
      consolePage.includes("disabled={!composerEditable}") &&
      consolePage.includes("class:stop={workerRunning}") &&
      consolePage.includes('"Stop Worker"') &&
      consolePage.includes("disabled={composerSubmitDisabled}") &&
      !consolePage.includes("disabled={!inputReady || sending}") &&
      !consolePage.includes("enabled: inputReady && !sending"),
    "Worker Console composer should stay editable during runs and turn the submit button into a Stop control",
  );
});

Deno.test("Account UI owns browser passkey session state without workspace authorization", async () => {
  const accountPage = await Deno.readTextFile(
    new URL("./../../../routes/account/+page.svelte", import.meta.url),
  );
  const devicePage = await Deno.readTextFile(
    new URL("./../../../routes/login/device/+page.svelte", import.meta.url),
  );
  const authApi = await Deno.readTextFile(
    new URL("../auth/api.ts", import.meta.url),
  );
  const rootLayout = await Deno.readTextFile(
    new URL("./../../../routes/+layout.svelte", import.meta.url),
  );
  const rootLayoutLoad = await Deno.readTextFile(
    new URL("./../../../routes/+layout.ts", import.meta.url),
  );
  const globalSidebar = await Deno.readTextFile(
    new URL("../sidebar/GlobalSidebar.svelte", import.meta.url),
  );
  const sidebarFrame = await Deno.readTextFile(
    new URL("../sidebar/SidebarFrame.svelte", import.meta.url),
  );
  const sidebarCss = await Deno.readTextFile(
    new URL("../sidebar/sidebar.css", import.meta.url),
  );
  const workspaceLayout = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/+layout.svelte",
      import.meta.url,
    ),
  );
  const workspaceLayoutLoad = await Deno.readTextFile(
    new URL("./../../../routes/w/[workspaceId]/+layout.ts", import.meta.url),
  );
  const sidebarOverride = await Deno.readTextFile(
    new URL("../sidebar/SidebarOverride.svelte", import.meta.url),
  );
  const sidebar = await Deno.readTextFile(
    new URL("../sidebar/WorkspaceSidebar.svelte", import.meta.url),
  );

  assert(
    accountPage.includes("registerPasskey") &&
      accountPage.includes("loginWithPasskey") &&
      accountPage.includes("logout") &&
      accountPage.includes("loadWhoami") &&
      accountPage.includes("Current user"),
    "Account page should expose registration, login, logout, and current-session inspection",
  );
  assert(
    devicePage.includes("approveDeviceLogin") &&
      devicePage.includes("loginWithPasskey") &&
      devicePage.includes("user_code") &&
      devicePage.includes("Approve device login"),
    "Device login page should approve CLI login without DevTools console",
  );
  assert(
    authApi.includes("/api/auth/whoami") &&
      authApi.includes("/api/auth/passkeys/registration/options") &&
      authApi.includes("/api/auth/passkeys/login/complete") &&
      authApi.includes("/api/auth/logout") &&
      authApi.includes("/api/auth/device-login/approve"),
    "Auth model should stay on Backend auth APIs rather than workspace authorization APIs",
  );
  assert(
    rootLayout.includes("SIDEBAR_CONTEXT") &&
      rootLayout.includes("GlobalSidebar") &&
      rootLayout.includes("SidebarFrame") &&
      rootLayout.includes("{@render sidebar()}") &&
      !rootLayout.includes("WorkspaceSidebar") &&
      rootLayout.includes("workspace-topbar") &&
      rootLayout.includes("topbar-icon-button") &&
      rootLayout.includes('href="/account"') &&
      rootLayout.includes("Open Account") &&
      !sidebar.includes("accountHref") &&
      !sidebar.includes("Open Account"),
    "Root layout chrome should render a registered sidebar snippet or default global sidebar while account navigation stays in the header",
  );
  assert(
    globalSidebar.includes("Global") &&
      globalSidebar.includes("/account") &&
      globalSidebar.includes("/login/device") &&
      !globalSidebar.includes("Tickets") &&
      !globalSidebar.includes("Repositories"),
    "Root default sidebar should contain only global navigation, not workspace-scoped sections",
  );
  assert(
    workspaceLayout.includes("{#snippet workspaceSidebar()}") &&
      workspaceLayout.includes("WorkspaceSidebar") &&
      workspaceLayout.includes(
        "<SidebarOverride sidebar={workspaceSidebar} />",
      ) &&
      workspaceLayoutLoad.includes("params.workspaceId") &&
      workspaceLayoutLoad.includes("workspaceApiPath(workspaceId"),
    "Workspace layout should load workspace data and register a WorkspaceSidebar snippet",
  );
  assert(
    sidebarFrame.includes("let folded = $state(false)") &&
      sidebarFrame.includes("sidebar-fold-button") &&
      sidebarFrame.includes("Fold sidebar") &&
      sidebarFrame.includes("Unfold sidebar") &&
      !workspaceLayout.includes("sidebarFolded") &&
      !workspaceLayout.includes("onToggleFold") &&
      !sidebar.includes("folded?: boolean") &&
      !sidebar.includes("onToggleFold?: () => void") &&
      !sidebar.includes("sidebar-fold-button"),
    "Sidebar fold control should belong to SidebarFrame, not WorkspaceSidebar",
  );
  assert(
    sidebarCss.startsWith("@layer reset, tokens, base, layout, components;") &&
      sidebarCss.includes(".sidebar-frame") &&
      sidebarCss.includes(".sidebar-link") &&
      sidebarCss.includes("text-decoration: none"),
    "Sidebar styles should define their layer order before component rules so base link styles do not win by import order",
  );
  assert(
    sidebarOverride.includes("controller.setSidebar(sidebar)") &&
      sidebarOverride.includes("controller.clearSidebar(sidebar)"),
    "SidebarOverride should register and clean up the child-provided sidebar snippet",
  );
  assert(
    rootLayoutLoad.includes('"/account"') &&
      rootLayoutLoad.includes('"/login/device"'),
    "Root layout should not redirect account and device-login public routes to a workspace",
  );
});
