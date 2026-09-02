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
  const sidebarCss = await Deno.readTextFile(
    new URL("../sidebar/sidebar.css", import.meta.url),
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
    "top workspace page should link to Tickets, Runtimes under Settings, and the Workers page",
  );
  assert(
    !workspacePage.includes("workerConsoleHref") &&
      !workspacePage.includes("Open Console"),
    "top workspace page should not own the Worker list",
  );
  assert(
    workersPage.includes("workerHref") &&
      workersPage.includes("workers-table") &&
      workersPage.includes("workerDisplayName") &&
      workersPage.includes("worker.resource_key") &&
      workersPage.includes("Delete ${workerDisplayName}"),
    "dedicated Workers page should expose a table, canonical Worker link target, and icon actions per Worker",
  );
  assert(
    workersNav.includes("href={`/w/${workspaceId}/workers`}") &&
      workersNav.includes("filter(canShowWorkerInSidebar)") &&
      workersNav.includes("worker.display_name || worker.label") &&
      workersNav.includes("worker-status-dot") &&
      workersNav.includes("worker-status-spinner") &&
      workersNav.includes("worker.repository_id ?? '—'") &&
      workersNav.includes("worker.working_directory_id ?? '—'") &&
      !workersNav.includes('aria-disabled="true"'),
    "Workers sidebar should link to the Worker list page and show state indicators with repository/workdir metadata",
  );
  assert(
    workersNav.includes("COLLAPSED_WORKER_COUNT = 6") &&
      workersNav.includes("workers.length > COLLAPSED_WORKER_COUNT") &&
      workersNav.includes("aria-expanded={expanded}") &&
      workersNav.includes("worker-overflow-chevron") &&
      !workersNav.includes("MAX_VISIBLE_WORKERS") &&
      sidebarCss.includes(".worker-overflow-toggle") &&
      sidebarCss.includes('[aria-expanded="true"] .worker-overflow-chevron') &&
      sidebarCss.includes("transform: rotate(180deg)"),
    "Workers sidebar should collapse overflow behind a graphical chevron without dropping Workers",
  );
  assert(
    !sidebar.includes("CompanionNavSection") &&
      sidebar.includes("TicketsNavSection") &&
      sidebar.includes("MergeRequestsNavSection") &&
      sidebar.lastIndexOf("MergeRequestsNavSection") <
        sidebar.lastIndexOf("MemoryNavSection") &&
      sidebar.includes("WorkersNavSection") &&
      sidebarCss.includes("gap: var(--space-1)") &&
      sidebarCss.includes(".sidebar-nav-section--category > .sidebar-link") &&
      sidebarCss.includes("padding-block: var(--space-1)") &&
      sidebarCss.includes("margin-left: var(--space-3)") &&
      sidebarCss.includes("--sidebar-item-hover: oklch(24% 0 0)") &&
      sidebarCss.includes("--sidebar-item-active: oklch(32% 0 0)") &&
      sidebarCss.includes("background: var(--sidebar-item-hover)") &&
      sidebarCss.includes("background: var(--sidebar-item-active)") &&
      !sidebarCss.includes("background: var(--interactive-selected)") &&
      !sidebarCss.includes("margin-inline: calc(-1"),
    "workspace navigation should place Merge Requests before an indented compact Memory category",
  );
});

Deno.test("workspace Tickets surface provides Kanban and lifecycle controls", async () => {
  const ticketsNav = await Deno.readTextFile(
    new URL("../sidebar/TicketsNavSection.svelte", import.meta.url),
  );
  const objectivesNav = await Deno.readTextFile(
    new URL("../sidebar/ObjectivesNavSection.svelte", import.meta.url),
  );
  const mergeRequestsNav = await Deno.readTextFile(
    new URL("../sidebar/MergeRequestsNavSection.svelte", import.meta.url),
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
  const ticketPanelModel = await Deno.readTextFile(
    new URL("../tickets/ticket-panel.ts", import.meta.url),
  );
  const generatedTicketApi = await Deno.readTextFile(
    new URL("../../generated/ticket-api.ts", import.meta.url),
  );
  const repositoryLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/repositories/[repositoryId]/+page.ts",
      import.meta.url,
    ),
  );
  const repositoryPage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/repositories/[repositoryId]/+page.svelte",
      import.meta.url,
    ),
  );

  assert(
    ticketsNav.includes("workspaceRoute(workspaceId, '/tickets')") &&
      ticketsNav.includes("sidebar-nav-section--resource") &&
      ticketsNav.includes('class="sidebar-link"') &&
      ticketsNav.includes(">Tickets</a>") &&
      !ticketsNav.includes("Open Tickets") &&
      !ticketsNav.includes("workspace tickets") &&
      objectivesNav.includes("sidebar-nav-section--resource") &&
      objectivesNav.includes('class="sidebar-link"') &&
      objectivesNav.includes(">Objectives</a>") &&
      !objectivesNav.includes("Open Objectives") &&
      !objectivesNav.includes("workspace objectives") &&
      mergeRequestsNav.includes("sidebar-nav-section--resource") &&
      mergeRequestsNav.includes('class="sidebar-link"') &&
      mergeRequestsNav.includes(">Merge Requests</a>") &&
      !mergeRequestsNav.includes("All Merge Requests") &&
      !mergeRequestsNav.includes("review and integration resources"),
    "Tickets, Objectives, and Merge Requests should each be a single primary sidebar link",
  );
  assert(
    ticketsLoad.includes("Object.entries(LANE_STATES)") &&
      ticketsLoad.includes('limit: "30"') &&
      ticketsLoad.includes('states: states.join(",")') &&
      ticketsLoad.includes("/tickets?${search}") &&
      !ticketsLoad.includes("/tickets/query") &&
      ticketsPage.includes('class="ticket-kanban"') &&
      ticketsPage.includes("laneState") &&
      ticketsPage.includes("loadMore(lane.id)") &&
      ticketsPage.includes("handleLaneScroll(event, lane.id)"),
    "Tickets list should fetch lightweight paginated summaries for each Kanban lane",
  );
  assert(
    ticketPanelModel.includes('label: "Ready + Planning"') &&
      ticketPanelModel.includes('label: "In progress + Queued"') &&
      ticketPanelModel.includes('label: "Done + Closed"') &&
      ticketPanelModel.includes("TICKET_LANE_PAGE_SIZE = 30") &&
      ticketPanelModel.includes("nextTicketLaneVisibleCount"),
    "Ticket Kanban should combine related states into independent 30-item display windows",
  );
  assert(
    generatedTicketApi.includes("Generated from yoi-workspace-server") &&
      generatedTicketApi.includes("export type TicketListResponse") &&
      generatedTicketApi.includes("items: Array<TicketSummary>"),
    "Ticket response types should come from the generated Server contract",
  );
  assert(
    !repositoryLoad.includes("/tickets") &&
      !repositoryPage.includes("Repository Tickets") &&
      !repositoryPage.includes("RepositoryTicketKanban"),
    "Repository detail should not keep a second Repository Tickets surface",
  );
  assert(
    ticketDetailLoad.includes("/repositories") &&
      ticketDetailPage.includes('mutate("state", "/state"') &&
      ticketDetailPage.includes("async function queueTicket") &&
      ticketDetailPage.includes("const path = ticketPath") &&
      ticketDetailPage.includes("`${path}/queue`") &&
      !ticketDetailPage.includes("/merge-request/merge") &&
      ticketDetailPage.includes("mergeRequest.selector_from") &&
      ticketDetailPage.includes("mergeRequest.review_status") &&
      ticketDetailPage.includes("mergeRequestPagePath") &&
      ticketDetailPage.includes('mutate("close", "/close"') &&
      ticketDetailPage.includes("mutateAssignment") &&
      ticketDetailPage.includes("can_start_manual_coder") &&
      ticketDetailPage.includes("ticket.relations.outgoing"),
    "Ticket detail should expose typed lifecycle actions, relations, target selection, assignments, and Merge Request navigation",
  );
});

Deno.test("workspace Memory surfaces use read-only scoped memory APIs", async () => {
  const memoryNav = await Deno.readTextFile(
    new URL("../sidebar/MemoryNavSection.svelte", import.meta.url),
  );
  const memoryDocumentLoad = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/memory/+page.ts",
      import.meta.url,
    ),
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
    memoryNav.includes('workspaceRoute(workspaceId, "/memory")') &&
      memoryNav.includes(
        '<h2 class="sidebar-nav-section__header">Memory</h2>',
      ) &&
      memoryNav.includes("Document</a>") &&
      memoryNav.includes("sidebar-nav-section--category") &&
      memoryNav.includes('class="sidebar-link"') &&
      memoryNav.includes('workspaceRoute(workspaceId, "/memory/staging")') &&
      memoryNav.includes("Staging</a>") &&
      !memoryNav.includes("item-meta") &&
      !memoryNav.includes("durable workspace memory") &&
      !memoryNav.includes("pending extraction candidates"),
    "Memory sidebar section should show Document and Staging as compact single-line links",
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

Deno.test("root layout keeps Workspace selection explicit", async () => {
  const layoutLoad = await Deno.readTextFile(
    new URL("./../../../routes/+layout.ts", import.meta.url),
  );

  assert(
    layoutLoad.includes("export const load") &&
      layoutLoad.includes("() => ({})") &&
      !layoutLoad.includes("scopedCompatibilityRoute") &&
      !layoutLoad.includes("/api/workspace") &&
      !layoutLoad.includes("workspaceRoute") &&
      !layoutLoad.includes("redirect("),
    "root layout should not infer, bootstrap, or redirect through a singleton Workspace",
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
      consolePage.includes("workspaceMultiplexer") &&
      consolePage.includes('topic: "worker_protocol"') &&
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

Deno.test("Worker Console owns its narrower centered shell width", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const rootLayout = await Deno.readTextFile(
    new URL("./../../../routes/+layout.svelte", import.meta.url),
  );

  assert(
    page.includes(".console-shell {") &&
      page.includes("max-width: 920px;") &&
      page.includes("margin-inline: auto;") &&
      rootLayout.includes("max-width: 1280px;"),
    "Root content should allow 1280px while Worker Console remains centered at 920px",
  );
});

Deno.test("Worker Console overview activity summaries use 14px text", async () => {
  const consoleLine = await Deno.readTextFile(
    new URL("./ConsoleLineItem.svelte", import.meta.url),
  );

  assert(
    consoleLine.includes(".activity-summary {") &&
      consoleLine.includes("font-size: 14px;"),
    "Overview activity summaries such as ran command counts should render at 14px",
  );
});

Deno.test("Worker Console renders markdown only for message rows", async () => {
  const consoleLine = await Deno.readTextFile(
    new URL("./ConsoleLineItem.svelte", import.meta.url),
  );

  assert(
    consoleLine.includes("function shouldRenderMarkdown") &&
      consoleLine.includes("item.kind === 'tool'") &&
      consoleLine.includes("{#if isBashTool(item)}") &&
      consoleLine.includes(
        "<AnsiText text={toolBodyText(item)} />",
      ) &&
      consoleLine.includes(
        ".console-line.tool-bash .console-plain-text",
      ) &&
      consoleLine.includes(
        ".console-line.tool.tool-bash .console-plain-text",
      ) &&
      consoleLine.includes("font-size: 12px;") &&
      consoleLine.includes("line-height: 1.1;") &&
      consoleLine.includes("{:else if shouldRenderMarkdown(item)}") &&
      consoleLine.includes("<RichMarkdown text={item.body || '—'} />") &&
      !consoleLine.includes("{@html"),
    "Console should keep markdown rendering to message bodies, safely project Bash ANSI, and render other tool text literally",
  );
});

Deno.test("Worker Console expands uncapped tool body from the hover detail action", async () => {
  const consoleLine = await Deno.readTextFile(
    new URL("./ConsoleLineItem.svelte", import.meta.url),
  );

  assert(
    consoleLine.includes(
      "return detailOpen ? (line.expandedBody ?? line.body) : line.body",
    ) &&
      consoleLine.includes("line.toolCallLabel ?? line.toolCall?.name") &&
      consoleLine.includes("class={`tool-status") &&
      consoleLine.includes('class="tool-detail-button"') &&
      consoleLine.includes("aria-expanded={detailOpen}") &&
      consoleLine.includes("detailOpen = !detailOpen") &&
      consoleLine.includes("item.detail && detailOpen") &&
      consoleLine.includes('role="region"') &&
      consoleLine.includes(".console-line:hover .tool-detail-button") &&
      consoleLine.includes(".tool-detail-button:focus-visible") &&
      consoleLine.includes("@media (hover: none)") &&
      !consoleLine.includes('<details class="message-detail">'),
    "Normal tool display should keep its preview while detail reveals the uncapped body and existing metadata",
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

Deno.test("Worker Console removes redundant chrome and uses shared alerts", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const tasks = await Deno.readTextFile(
    new URL("./ConsoleTasks.svelte", import.meta.url),
  );

  assert(
    page.includes('<form class="console-composer"') &&
      !page.includes('class="console-composer card"') &&
      !page.includes(
        "padding: var(--space-3) var(--space-6) var(--space-4)",
      ) &&
      !page.includes("margin-inline: calc(-1 * var(--space-6))") &&
      page.includes("import { pushWorkspaceAlert }") &&
      page.includes('title: "Worker control"') &&
      page.includes('title: "Rewind targets"') &&
      page.includes('pushWorkspaceAlert("error"') &&
      !page.includes("controlNotice") &&
      !page.includes("console-notice") &&
      !tasks.includes("margin-bottom: -0.75rem"),
    "Console should remove redundant card/spacing chrome and route control notices through workspace alerts",
  );
});

Deno.test("Worker Console composer keeps a compact bounded chip editor", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const composerInput = await Deno.readTextFile(
    new URL("./ComposerInput.svelte", import.meta.url),
  );
  assert(
    consolePage.includes("<ComposerInput") &&
      consolePage.includes('<div class="composer-input-shell">') &&
      !consolePage.includes("handleComposerShellClick") &&
      consolePage.includes("bind:this={composerInputElement}") &&
      consolePage.includes("onchange={handleComposerChange}") &&
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
      composerInput.includes("max-height: 10rem") &&
      composerInput.includes("EditorView.lineWrapping") &&
      composerInput.includes("overflow-y: auto"),
    "Console composer should use the bounded chip-capable editor with wrapping, page scrolling, and the icon send button",
  );
});

Deno.test("Worker Console paste chips preserve typed draft and target authority", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const composerInput = await Deno.readTextFile(
    new URL("./ComposerInput.svelte", import.meta.url),
  );
  assert(
    composerInput.includes('measurement.presentation === "chip"') &&
      composerInput.includes("registerTextPaste") &&
      composerInput.includes("EditorView.atomicRanges") &&
      composerInput.includes('key: "Backspace"') &&
      composerInput.includes('key: "Delete"') &&
      composerInput.includes(
        "composerDeletionRange(selection, pastes, direction)",
      ) &&
      composerInput.includes("EditorState.readOnly.of(isDisabled)") &&
      composerInput.includes('key: "Mod-z"') &&
      composerInput.includes("if (!view || view.state.readOnly) return") &&
      composerInput.includes("if (currentView.state.readOnly) return false") &&
      consolePage.includes("activeComposerTargetKey !== targetKey") &&
      consolePage.includes("if (!composerEditable) return") &&
      composerInput.includes('chip.setAttribute("aria-label", label)') &&
      composerInput.includes("preserveExactText = false") &&
      consolePage.includes("buildComposerSegmentsRequest(value.segments, {") &&
      consolePage.includes("preserveExactText: value.textPastes.length > 0") &&
      consolePage.includes("composerDrafts.set(activeComposerTargetKey") &&
      consolePage.includes("switchComposerTarget(target)") &&
      consolePage.includes('sendControl({ method: "cancel" }, "Stop")'),
    "Paste chips should use shared threshold classification, atomic keyboard behavior, accessible labels, typed restore, and per-Worker draft authority",
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
    editor.includes("let view = $state.raw<EditorView | null>(null)") &&
      editor.includes("untrack(() => view)") &&
      editor.includes("untrack(() => value)") &&
      editor.includes("untrack(() => onChange)"),
    "CodeMirror EditorView must not be deep reactive or tracked by the mount effect; otherwise cleanup can loop forever",
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
      settingsModel.includes('id: "runtimes"') &&
      settingsModel.includes("return `${SETTINGS_ROUTE}/runtimes`;"),
    "Runtimes should be admin Settings navigation, not primary workspace sidebar navigation",
  );
  assert(
    runtimesPage.includes("Add remote Runtime") &&
      runtimesPage.includes("Open workdirs") &&
      runtimesPage.includes("settings-runtime-table") &&
      runtimesPage.includes(
        "/runtimes/${encodeURIComponent(runtime.runtime_id)}/connection-tests",
      ) &&
      runtimesPage.includes(
        "/settings/runtimes/${encodeURIComponent(runtime.runtime_id)}/workdirs",
      ),
    "Settings Runtimes page should expose canonical REST actions and link to each Runtime's workdirs",
  );
  assert(
    workdirsPage.includes(">Runtimes</a>") &&
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
      newWorkerPage.includes("buildCreateWorkspaceWorkerRequest") &&
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
      consolePage.includes("workspaceMultiplexer") &&
      consolePage.includes("sendWorkerMethod") &&
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
    consolePage.includes("const token = advanceReloadToken();") &&
      consolePage.includes("worker = targetWorker;") &&
      consolePage.includes(
        "if (!targetWorker) void loadWorker(target, token);",
      ) &&
      !consolePage.includes("void refreshConsole();\n  });\n\n  $effect"),
    "target-change effect should install route data and guard fallback loading with the new target token",
  );
  assert(
    consolePage.includes(
      'const workerRunning = $derived(workerState === "running");',
    ) &&
      consolePage.includes(
        'const composerEditable = $derived(protocolState === "open" && !sending);',
      ) &&
      consolePage.includes('sendControl({ method: "cancel" }, "Stop")') &&
      consolePage.includes("onsubmit={handleComposerSubmit}") &&
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
  const globalNavSections = await Deno.readTextFile(
    new URL("../sidebar/GlobalNavSections.svelte", import.meta.url),
  );
  const workspaceCatalogPage = await Deno.readTextFile(
    new URL("./../../../routes/+page.svelte", import.meta.url),
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
      rootLayout.includes("content={sidebar}") &&
      !rootLayout.includes("WorkspaceSidebar") &&
      rootLayout.includes('class="app-shell"') &&
      rootLayout.includes('class="app-shell__main"') &&
      rootLayout.includes("app-shell__topbar") &&
      rootLayout.includes("app-shell__icon-button") &&
      rootLayout.includes('href="/account"') &&
      rootLayout.includes("Open Account") &&
      !sidebar.includes("accountHref") &&
      !sidebar.includes("Open Account"),
    "Root layout chrome should keep GlobalSidebar as the root slot owner while account navigation stays in the header",
  );
  assert(
    globalSidebar.includes("GlobalNavSections") &&
      globalNavSections.includes('aria-label="Global pages"') &&
      !globalNavSections.includes('<p class="sidebar-section-label">') &&
      globalNavSections.includes('"/account"') &&
      globalNavSections.includes('"/login/device"') &&
      !globalNavSections.includes('label: "Workspaces"') &&
      globalNavSections.includes("sidebar-nav-section--category") &&
      globalNavSections.includes("global-workspaces-heading") &&
      globalNavSections.includes("workspaces") &&
      globalNavSections.includes("workspace.display_name") &&
      globalNavSections.includes("workspaceHref(workspace.workspace_id)") &&
      workspaceCatalogPage.includes("SidebarOverride") &&
      workspaceCatalogPage.includes("sidebar={homeSidebar}") &&
      workspaceCatalogPage.includes("GlobalNavSections") &&
      workspaceCatalogPage.includes("{workspaces}") &&
      !globalNavSections.includes("Tickets") &&
      !globalNavSections.includes("Repositories"),
    "Root page sidebar should replace the Workspaces button with a categorized accessible Workspace list below the remaining global navigation",
  );
  assert(
    workspaceLayout.includes("{#snippet workspaceSidebar()}") &&
      workspaceLayout.includes("WorkspaceSidebar") &&
      workspaceLayout.includes("controller={parentSidebarController}") &&
      workspaceLayout.includes("sidebar={workspaceSidebar}") &&
      workspaceLayoutLoad.includes("params.workspaceId") &&
      workspaceLayoutLoad.includes("workspaceApiPath(workspaceId"),
    "Workspace layout should load workspace data, register with the parent slot, and provide the same slot contract to children",
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
    sidebarOverride.includes("controller.registerSidebar(sidebar)") &&
      rootLayout.includes("createOverrideStack<SidebarSnippet>") &&
      rootLayout.includes("registerSidebar: sidebarOverrides.register"),
    "SidebarOverride should register a nested sidebar whose cleanup restores the parent override",
  );
  assert(
    rootLayoutLoad.includes("export const load") &&
      rootLayoutLoad.includes("() => ({})") &&
      !rootLayoutLoad.includes("workspaceRoute") &&
      !rootLayoutLoad.includes("redirect("),
    "Root layout should leave account and device-login routes public by avoiding Workspace redirects entirely",
  );
});

Deno.test("Workspace Worker list and Console share the multiplexed connection", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const sidebarStore = await Deno.readTextFile(
    new URL("./../sidebar/worker-subscription.ts", import.meta.url),
  );
  const multiplexer = await Deno.readTextFile(
    new URL("./../multiplexer.ts", import.meta.url),
  );
  assert(
    consolePage.includes("workspaceMultiplexer(target.workspaceId)") &&
      sidebarStore.includes("workspaceMultiplexer(workspaceId)") &&
      multiplexer.includes("const multiplexers = new Map") &&
      multiplexer.includes("frame: 'worker_protocol'"),
    "Sidebar and Console should share one Workspace multiplexer and route Worker methods through a subscription lane",
  );
  assert(
    multiplexer.includes("nextMultiplexerId") &&
      !multiplexer.includes("crypto.randomUUID"),
    "Workspace subscription correlation IDs should not require secure-context crypto APIs",
  );
  assert(
    multiplexer.includes("this.#socket?.readyState === WebSocket.OPEN") &&
      multiplexer.includes("this.#sendSubscribe(subscription)") &&
      consolePage.includes("const targetWorker = data.worker") &&
      consolePage.includes("worker = targetWorker") &&
      consolePage.includes(
        "const consoleTarget = $derived({ workspaceId, runtimeId, workerId })",
      ),
    "A reused Console route should subscribe immediately on the live Workspace socket and install the new route Worker",
  );
});

Deno.test("Web Console renders the client-projected Worker task store", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const tasksComponent = await Deno.readTextFile(
    new URL("./ConsoleTasks.svelte", import.meta.url),
  );
  const tasksModel = await Deno.readTextFile(
    new URL("./tasks.ts", import.meta.url),
  );

  assert(
    consolePage.includes("ConsoleTasks") &&
      consolePage.includes("selectedConsoleProjection.tasks") &&
      consolePage.includes("taskPaneOpen"),
    "Console should expose the projected task store through its existing client model",
  );
  assert(
    tasksComponent.includes("[ ]") &&
      tasksComponent.includes("[~]") &&
      tasksComponent.includes("[x]") &&
      tasksComponent.includes("[-]") &&
      tasksComponent.includes('return count === 1 ? "task" : "tasks"') &&
      !tasksComponent.includes(", deleted: {counts.deleted}") &&
      tasksComponent.includes("task.description"),
    "Tasks UI should mirror the TUI status marks, pluralize its summary, omit the deleted count, and show descriptions",
  );
  assert(
    tasksModel.includes('name === "TaskCreate"') &&
      tasksModel.includes('name !== "TaskUpdate"') &&
      tasksModel.includes("[Session TaskStore snapshot]") &&
      !consolePage.includes("fetchTasks"),
    "Task projection should replay the protocol client-side without adding a task API",
  );
});

Deno.test("Web Console switches main and direct SubWorker views from the Tasks row", async () => {
  const consolePage = await Deno.readTextFile(
    new URL(
      "./../../../routes/w/[workspaceId]/runtimes/[runtimeId]/workers/[workerId]/console/+page.svelte",
      import.meta.url,
    ),
  );
  const tasksComponent = await Deno.readTextFile(
    new URL("./ConsoleTasks.svelte", import.meta.url),
  );
  const consoleModel = await Deno.readTextFile(
    new URL("./model.ts", import.meta.url),
  );

  assert(
    consolePage.includes("selectedWorkerViewSessionId") &&
      consolePage.includes("selectConsoleWorkerView") &&
      consolePage.includes("selectedConsoleProjection.lines") &&
      consolePage.includes("selectedConsoleProjection.tasks") &&
      consolePage.includes("onSelectWorkerView") &&
      consolePage.includes(
        "selectConsoleWorkerView(resolvedSessionId, false)",
      ) &&
      consolePage.includes("consoleWorkerViewSelectionIsResolved") &&
      !consolePage.includes("internal-worker-pane") &&
      !consolePage.includes("flattenInternalWorkers"),
    "Console should render one selected transcript/task projection without appending Internal Worker panes",
  );
  assert(
    tasksComponent.includes('role="group"') &&
      tasksComponent.includes("aria-pressed") &&
      tasksComponent.includes("onclick") &&
      tasksComponent.includes("tasks.length > 0 || workerViews.length > 1"),
    "Tasks summary should expose a clickable and accessible Worker view selector even with zero tasks",
  );
  assert(
    consoleModel.includes("consoleWorkerViews") &&
      consoleModel.includes('worker.worker.kind === "sub_worker"') &&
      consoleModel.includes("children.map") &&
      consoleModel.includes("resolveConsoleWorkerView"),
    "Worker view selection should expose only direct SubWorker session identities with main fallback",
  );
});
