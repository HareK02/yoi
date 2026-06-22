<script lang="ts">
  import { onMount } from 'svelte';
  import WorkspaceSidebar from '$lib/workspace-sidebar/WorkspaceSidebar.svelte';
  import type {
    Diagnostic,
    Host,
    ListResponse,
    ObjectiveListResponse,
    ObjectiveSummary,
    RepositoryDetailResponse,
    RepositoryLogResponse,
    RepositorySummary,
    RepositoryTicketsResponse,
    Worker,
    WorkspaceResponse
  } from '$lib/workspace-sidebar/types';

  type RouteState =
    | { page: 'overview'; objectiveId?: undefined }
    | { page: 'repository'; objectiveId?: undefined }
    | { page: 'objectives'; objectiveId?: string };

  const endpoints = [
    { label: 'Workspace', path: '/api/workspace' },
    { label: 'Tickets', path: '/api/tickets' },
    { label: 'Objectives', path: '/api/objectives' },
    { label: 'Repositories', path: '/api/repositories' },
    { label: 'Repository log', path: '/api/repositories/local/log' },
    { label: 'Repository tickets', path: '/api/repositories/local/tickets' },
    { label: 'Hosts', path: '/api/hosts' },
    { label: 'Workers', path: '/api/workers' }
  ];

  let workspace = $state<WorkspaceResponse | null>(null);
  let hosts = $state<ListResponse<Host> | null>(null);
  let workers = $state<ListResponse<Worker> | null>(null);
  let repository = $state<RepositorySummary | null>(null);
  let repositoryLog = $state<RepositoryLogResponse | null>(null);
  let repositoryTickets = $state<RepositoryTicketsResponse | null>(null);
  let objectives = $state<ObjectiveListResponse | null>(null);

  let workspaceError = $state<string | null>(null);
  let hostsError = $state<string | null>(null);
  let workersError = $state<string | null>(null);
  let repositoryError = $state<string | null>(null);
  let repositoryLogError = $state<string | null>(null);
  let repositoryTicketsError = $state<string | null>(null);
  let objectivesError = $state<string | null>(null);
  let currentPath = $state('/');
  let route = $derived(routeFromPath(currentPath));

  async function getJson<T>(path: string): Promise<T> {
    const response = await fetch(path);
    if (!response.ok) {
      throw new Error(`GET ${path} failed: ${response.status}`);
    }
    return response.json() as Promise<T>;
  }

  async function loadWorkspace() {
    workspaceError = null;
    try {
      workspace = await getJson<WorkspaceResponse>('/api/workspace');
    } catch (error) {
      workspaceError = error instanceof Error ? error.message : String(error);
      workspace = null;
    }
  }

  async function loadHosts() {
    hostsError = null;
    try {
      hosts = await getJson<ListResponse<Host>>('/api/hosts');
    } catch (error) {
      hostsError = error instanceof Error ? error.message : String(error);
      hosts = null;
    }
  }

  async function loadWorkers() {
    workersError = null;
    try {
      workers = await getJson<ListResponse<Worker>>('/api/workers');
    } catch (error) {
      workersError = error instanceof Error ? error.message : String(error);
      workers = null;
    }
  }

  async function loadRepository() {
    repositoryError = null;
    try {
      const detail = await getJson<RepositoryDetailResponse>('/api/repositories/local');
      repository = detail.item;
    } catch (error) {
      repositoryError = error instanceof Error ? error.message : String(error);
      repository = null;
    }
  }

  async function loadRepositoryLog() {
    repositoryLogError = null;
    try {
      repositoryLog = await getJson<RepositoryLogResponse>('/api/repositories/local/log?limit=10');
    } catch (error) {
      repositoryLogError = error instanceof Error ? error.message : String(error);
      repositoryLog = null;
    }
  }

  async function loadRepositoryTickets() {
    repositoryTicketsError = null;
    try {
      repositoryTickets = await getJson<RepositoryTicketsResponse>('/api/repositories/local/tickets');
    } catch (error) {
      repositoryTicketsError = error instanceof Error ? error.message : String(error);
      repositoryTickets = null;
    }
  }

  async function loadObjectives() {
    objectivesError = null;
    try {
      objectives = await getJson<ObjectiveListResponse>('/api/objectives');
    } catch (error) {
      objectivesError = error instanceof Error ? error.message : String(error);
      objectives = null;
    }
  }

  function diagnosticsFor(...groups: Array<Diagnostic[] | undefined>): Diagnostic[] {
    return groups.flatMap((group) => group ?? []);
  }

  function routeFromPath(path: string): RouteState {
    if (path.startsWith('/repositories')) {
      return { page: 'repository' };
    }
    if (path.startsWith('/objectives')) {
      const [, , objectiveId] = path.split('/');
      return { page: 'objectives', objectiveId: objectiveId || undefined };
    }
    return { page: 'overview' };
  }

  function updateRouteFromHash() {
    const hashPath = window.location.hash.replace(/^#/, '') || '/';
    currentPath = hashPath.startsWith('/') ? hashPath : `/${hashPath}`;
  }

  function formatDate(value: string | null | undefined): string {
    return value ?? 'not recorded';
  }

  function shortHash(hash: string | null | undefined): string {
    return hash ? hash.slice(0, 12) : 'unknown';
  }

  $effect(() => {
    void loadWorkspace();
    void loadHosts();
    void loadWorkers();
    void loadRepository();
    void loadRepositoryLog();
    void loadRepositoryTickets();
    void loadObjectives();
  });

  onMount(() => {
    updateRouteFromHash();
    window.addEventListener('hashchange', updateRouteFromHash);
    return () => window.removeEventListener('hashchange', updateRouteFromHash);
  });
</script>

<svelte:head>
  <title>Yoi Workspace Control Plane</title>
  <meta
    name="description"
    content="Local single-workspace Yoi control plane bootstrap"
  />
</svelte:head>

<div class="workspace-layout">
  <WorkspaceSidebar {workspace} {workspaceError} />

  <main class="shell">
    <section class="hero">
      <p class="eyebrow">Local / single-workspace bootstrap</p>
      <h1>Yoi Workspace Control Plane</h1>
      <p>
        Static SPA shell for reading canonical <code>.yoi</code> project records,
        bounded local Repository summaries, and the local Host / Worker execution
        view. Ticket and Objective lifecycle authority stays in the existing local
        record workflow.
      </p>
      <p class="page-links" aria-label="Workspace page links">
        <a href="#/">Overview</a>
        <a href="#/repositories/local">Repository</a>
        <a href="#/objectives">Objectives</a>
      </p>
    </section>

    {#if route.page === 'repository'}
      <section class="grid runtime">
        <div class="card">
          <h2>Repository summary</h2>
          {#if repository}
            <dl>
              <div>
                <dt>ID</dt>
                <dd><code>{repository.id}</code></dd>
              </div>
              <div>
                <dt>Kind</dt>
                <dd>{repository.kind}</dd>
              </div>
              <div>
                <dt>Workspace root</dt>
                <dd><code>{repository.workspace_root}</code></dd>
              </div>
              <div>
                <dt>Record authority</dt>
                <dd>{repository.record_authority}</dd>
              </div>
              <div>
                <dt>Git</dt>
                <dd>{repository.git.status}</dd>
              </div>
            </dl>
          {:else if repositoryError}
            <p class="error">{repositoryError}</p>
          {:else}
            <p>Waiting for <code>/api/repositories/local</code>…</p>
          {/if}
        </div>

        <div class="card">
          <h2>Git summary</h2>
          {#if repository}
            {#if repository.git.status === 'available'}
              <dl>
                <div>
                  <dt>Root</dt>
                  <dd><code>{repository.git.root ?? 'unknown'}</code></dd>
                </div>
                <div>
                  <dt>Branch</dt>
                  <dd>{repository.git.branch ?? 'unknown'}</dd>
                </div>
                <div>
                  <dt>HEAD</dt>
                  <dd><code>{shortHash(repository.git.head)}</code></dd>
                </div>
                <div>
                  <dt>Dirty</dt>
                  <dd>{repository.git.dirty === null || repository.git.dirty === undefined ? 'unknown' : repository.git.dirty ? 'yes' : 'no'} <small>{repository.git.dirty_scope}</small></dd>
                </div>
                <div>
                  <dt>Remote</dt>
                  <dd>
                    {#if repository.git.remote}
                      <code>{repository.git.remote.name}</code> · {repository.git.remote.url}
                      {#if repository.git.remote.redacted}<small>credentials redacted</small>{/if}
                    {:else}
                      not configured
                    {/if}
                  </dd>
                </div>
              </dl>
            {:else}
              <p>Git metadata is unavailable for this local Repository.</p>
            {/if}
          {:else if repositoryError}
            <p class="error">{repositoryError}</p>
          {:else}
            <p>Waiting for Git summary…</p>
          {/if}
        </div>
      </section>

      <section class="card">
        <h2>Recent Git log</h2>
        {#if repositoryLog}
          {#if repositoryLog.items.length === 0}
            <p>No recent commits are available from the bounded Git log API.</p>
          {:else}
            <div class="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>Commit</th>
                    <th>Subject</th>
                    <th>Author</th>
                    <th>Timestamp</th>
                  </tr>
                </thead>
                <tbody>
                  {#each repositoryLog.items as commit (commit.hash)}
                    <tr>
                      <td><code>{shortHash(commit.hash)}</code></td>
                      <td>{commit.subject}</td>
                      <td>{commit.author_name} <small>{commit.author_email}</small></td>
                      <td>{commit.timestamp}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {:else if repositoryLogError}
          <p class="error">{repositoryLogError}</p>
        {:else}
          <p>Waiting for <code>/api/repositories/local/log</code>…</p>
        {/if}
      </section>

      <section class="card">
        <h2>Repository Ticket Kanban</h2>
        <p class="section-note">
          Read-only grouping of canonical Ticket records. No drag/drop or lifecycle mutation is exposed.
        </p>
        {#if repositoryTickets}
          <div class="kanban">
            {#each repositoryTickets.columns as column (column.state)}
              <article class="kanban-column">
                <h3>{column.state} <span>{column.items.length}</span></h3>
                {#if column.items.length === 0}
                  <p class="muted">No tickets.</p>
                {:else}
                  <ul>
                    {#each column.items as ticket (ticket.id)}
                      <li>
                        <strong>{ticket.title}</strong>
                        <small><code>{ticket.id}</code> · updated {formatDate(ticket.updated_at)}</small>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </article>
            {/each}
          </div>
        {:else if repositoryTicketsError}
          <p class="error">{repositoryTicketsError}</p>
        {:else}
          <p>Waiting for <code>/api/repositories/local/tickets</code>…</p>
        {/if}
      </section>

      {@const repositoryDiagnostics = diagnosticsFor(repository?.git.diagnostics, repositoryLog?.diagnostics, repositoryTickets?.diagnostics)}
      {#if repositoryDiagnostics.length > 0}
        <section class="card diagnostics">
          <h2>Repository diagnostics</h2>
          <ul>
            {#each repositoryDiagnostics as diagnostic}
              <li>
                <strong>{diagnostic.severity}</strong>
                <code>{diagnostic.code}</code>
                <span>{diagnostic.message}</span>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {:else if route.page === 'objectives'}
      <section class="card">
        <h2>Objectives</h2>
        <p class="section-note">
          Objectives are read from canonical filesystem records through <code>/api/objectives</code>.
        </p>
        {#if objectives}
          {#if objectives.items.length === 0}
            <p>No Objective records are present.</p>
          {:else}
            <div class="stack">
              {#each objectives.items as objective (objective.id)}
                <article class="runtime-card selected-card" class:selected={route.objectiveId === objective.id}>
                  <div class="runtime-heading">
                    <strong>{objective.title}</strong>
                    <span>{objective.state}</span>
                  </div>
                  <p>{objective.summary || 'No summary text is available.'}</p>
                  <dl>
                    <div>
                      <dt>ID</dt>
                      <dd><code>{objective.id}</code></dd>
                    </div>
                    <div>
                      <dt>Updated</dt>
                      <dd>{formatDate(objective.updated_at)}</dd>
                    </div>
                    <div>
                      <dt>Linked tickets</dt>
                      <dd>{objective.linked_tickets?.length ? objective.linked_tickets.join(', ') : 'none'}</dd>
                    </div>
                  </dl>
                  <p><a href={`#/objectives/${objective.id}`}>Detail placeholder</a></p>
                </article>
              {/each}
            </div>
          {/if}
          {#if objectives.invalid_records.length > 0}
            <p class="error">{objectives.invalid_records.length} invalid objective record(s) hidden.</p>
          {/if}
        {:else if objectivesError}
          <p class="error">{objectivesError}</p>
        {:else}
          <p>Waiting for <code>/api/objectives</code>…</p>
        {/if}
      </section>

      {#if route.objectiveId}
        <section class="card detail-placeholder">
          <h2>Objective detail</h2>
          <p>
            Selected Objective <code>{route.objectiveId}</code>. This slice keeps detail navigation as a
            static SPA placeholder; canonical Objective content remains in the filesystem record.
          </p>
        </section>
      {/if}
    {:else}
      <section class="card">
        <h2>Workspace</h2>
        {#if workspace}
          <dl>
            <div>
              <dt>ID</dt>
              <dd>{workspace.workspace_id}</dd>
            </div>
            <div>
              <dt>Name</dt>
              <dd>{workspace.display_name}</dd>
            </div>
            <div>
              <dt>Record authority</dt>
              <dd>{workspace.record_authority}</dd>
            </div>
            <div>
              <dt>Host / Worker bridge</dt>
              <dd>{workspace.extension_points.host_worker_bridge.status}</dd>
            </div>
          </dl>
        {:else if workspaceError}
          <p class="error">{workspaceError}</p>
        {:else}
          <p>Waiting for <code>/api/workspace</code>…</p>
        {/if}
      </section>

      <section class="grid">
        <div class="card">
          <h2>Read API surface</h2>
          <ul>
            {#each endpoints as endpoint}
              <li><code>{endpoint.path}</code> — {endpoint.label}</li>
            {/each}
          </ul>
        </div>

        <div class="card">
          <h2>Reserved seams</h2>
          <p>
            Event streams remain represented as extension-point state in the backend
            response. Hosts and Workers are read-only local observations; no
            scheduler, lifecycle control, or hosted multi-tenant behavior is
            implemented in this slice.
          </p>
        </div>
      </section>

      <section class="grid runtime">
        <div class="card">
          <h2>Hosts</h2>
          {#if hosts}
            {#if hosts.items.length === 0}
              <p>No local Hosts are visible.</p>
            {:else}
              <div class="stack">
                {#each hosts.items as host}
                  <article class="runtime-card">
                    <div class="runtime-heading">
                      <strong>{host.label}</strong>
                      <span class:warn={host.status !== 'available'}>{host.status}</span>
                    </div>
                    <dl>
                      <div>
                        <dt>ID</dt>
                        <dd><code>{host.host_id}</code></dd>
                      </div>
                      <div>
                        <dt>Kind</dt>
                        <dd>{host.kind}</dd>
                      </div>
                      <div>
                        <dt>Local inspection</dt>
                        <dd>{host.capabilities.local_pod_inspection}</dd>
                      </div>
                      <div>
                        <dt>Platform</dt>
                        <dd>{host.capabilities.os} / {host.capabilities.arch}</dd>
                      </div>
                    </dl>
                  </article>
                {/each}
              </div>
            {/if}
          {:else if hostsError}
            <p class="error">{hostsError}</p>
          {:else}
            <p>Waiting for <code>/api/hosts</code>…</p>
          {/if}
        </div>

        <div class="card">
          <h2>Workers</h2>
          {#if workers}
            {#if workers.items.length === 0}
              <p>No local Workers are visible.</p>
            {:else}
              <div class="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Worker</th>
                      <th>Host</th>
                      <th>State</th>
                      <th>Workspace</th>
                      <th>Implementation</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each workers.items as worker}
                      <tr>
                        <td>
                          <strong>{worker.label}</strong>
                          {#if worker.role || worker.profile}
                            <small>{worker.role ?? 'role unknown'} / {worker.profile ?? 'profile unknown'}</small>
                          {/if}
                        </td>
                        <td><code>{worker.host_id}</code></td>
                        <td>{worker.state} · {worker.status}</td>
                        <td>{worker.workspace_root ?? 'unknown'}</td>
                        <td>{worker.implementation.kind}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          {:else if workersError}
            <p class="error">{workersError}</p>
          {:else}
            <p>Waiting for <code>/api/workers</code>…</p>
          {/if}
        </div>
      </section>

      {#if hosts || workers}
        {@const diagnostics = diagnosticsFor(hosts?.diagnostics, workers?.diagnostics)}
        {#if diagnostics.length > 0}
          <section class="card diagnostics">
            <h2>Diagnostics</h2>
            <ul>
              {#each diagnostics as diagnostic}
                <li>
                  <strong>{diagnostic.severity}</strong>
                  <code>{diagnostic.code}</code>
                  <span>{diagnostic.message}</span>
                </li>
              {/each}
            </ul>
          </section>
        {/if}
      {/if}
    {/if}
  </main>
</div>

<style>
  :global(*) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    background: #0f172a;
    color: #e2e8f0;
    font-family:
      Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
  }

  .workspace-layout {
    display: grid;
    grid-template-columns: minmax(240px, 300px) minmax(0, 1fr);
    gap: 24px;
    width: min(1240px, calc(100vw - 32px));
    margin: 0 auto;
    padding: 32px 0;
    min-width: 0;
  }

  .shell {
    display: grid;
    gap: 24px;
    min-width: 0;
  }

  .hero {
    min-width: 0;
  }

  .hero p {
    max-width: 68ch;
  }

  .page-links {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }

  .page-links a,
  .card a {
    color: #7dd3fc;
  }

  .page-links a {
    border: 1px solid rgba(125, 211, 252, 0.28);
    border-radius: 999px;
    padding: 6px 12px;
    text-decoration: none;
  }

  .eyebrow {
    color: #38bdf8;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  h1 {
    margin: 0 0 16px;
    font-size: clamp(2.5rem, 8vw, 5rem);
    line-height: 0.95;
    overflow-wrap: anywhere;
  }

  h2,
  h3 {
    margin-top: 0;
  }

  p,
  li,
  dd {
    overflow-wrap: anywhere;
  }

  code {
    color: #bae6fd;
  }

  .grid {
    display: grid;
    gap: 16px;
    grid-template-columns: repeat(auto-fit, minmax(min(260px, 100%), 1fr));
    min-width: 0;
  }

  .runtime {
    grid-template-columns: repeat(auto-fit, minmax(min(360px, 100%), 1fr));
  }

  .card {
    border: 1px solid rgba(148, 163, 184, 0.25);
    border-radius: 20px;
    background: rgba(15, 23, 42, 0.75);
    padding: 24px;
    box-shadow: 0 24px 80px rgba(15, 23, 42, 0.35);
    min-width: 0;
  }

  .section-note,
  .muted {
    color: #94a3b8;
  }

  .stack {
    display: grid;
    gap: 12px;
  }

  .runtime-card,
  .kanban-column {
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 16px;
    padding: 16px;
    background: rgba(15, 23, 42, 0.55);
  }

  .selected-card.selected {
    border-color: rgba(56, 189, 248, 0.5);
    background: rgba(14, 165, 233, 0.1);
  }

  .runtime-heading {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }

  .runtime-heading span {
    color: #86efac;
  }

  .runtime-heading span.warn {
    color: #fcd34d;
  }

  dl {
    display: grid;
    gap: 12px;
  }

  dt {
    color: #94a3b8;
    font-size: 0.85rem;
    text-transform: uppercase;
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .table-wrap {
    overflow-x: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th,
  td {
    border-bottom: 1px solid rgba(148, 163, 184, 0.18);
    padding: 10px 8px;
    text-align: left;
    vertical-align: top;
  }

  th {
    color: #94a3b8;
    font-size: 0.85rem;
    text-transform: uppercase;
  }

  small {
    color: #94a3b8;
    display: block;
    margin-top: 4px;
  }

  .kanban {
    display: grid;
    gap: 14px;
    grid-template-columns: repeat(auto-fit, minmax(min(190px, 100%), 1fr));
    margin-top: 16px;
  }

  .kanban-column h3 {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    color: #cbd5e1;
    font-size: 0.95rem;
    text-transform: uppercase;
  }

  .kanban-column ul {
    display: grid;
    gap: 10px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .kanban-column li {
    border-radius: 12px;
    background: rgba(30, 41, 59, 0.72);
    padding: 10px;
  }

  .diagnostics {
    margin-top: 16px;
  }

  .diagnostics li {
    display: grid;
    gap: 4px;
    margin-bottom: 12px;
  }

  .detail-placeholder {
    border-style: dashed;
  }

  .error {
    color: #fca5a5;
  }

  @media (max-width: 760px) {
    .workspace-layout {
      grid-template-columns: 1fr;
      width: min(100vw - 24px, 620px);
      gap: 18px;
      padding: 18px 0;
    }

    .card {
      padding: 18px;
    }
  }
</style>
