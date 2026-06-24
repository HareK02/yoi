<script lang="ts">
  import RepositoryTicketKanban from '$lib/workspace-pages/RepositoryTicketKanban.svelte';
  import WorkspaceSidebar from '$lib/workspace-sidebar/WorkspaceSidebar.svelte';
  import type {
    Diagnostic,
    Host,
    ListResponse,
    ObjectiveDetail,
    ObjectiveListResponse,
    RepositoryDetailResponse,
    RepositoryLogResponse,
    RepositorySummary,
    RepositoryTicketsResponse,
    Worker,
    WorkspaceResponse
  } from '$lib/workspace-sidebar/types';

  type WorkspaceView = 'overview' | 'repository' | 'objectives' | 'objective';

  type RouteState =
    | { page: 'overview'; objectiveId?: undefined }
    | { page: 'repository'; objectiveId?: undefined }
    | { page: 'objectives'; objectiveId?: undefined }
    | { page: 'objective'; objectiveId: string };

  let {
    view = 'overview',
    objectiveId = null
  }: { view?: WorkspaceView; repositoryId?: string; objectiveId?: string | null } = $props();

  const endpoints = [
    { label: 'Workspace', path: '/api/workspace' },
    { label: 'Tickets', path: '/api/tickets' },
    { label: 'Objectives', path: '/api/objectives' },
    { label: 'Repositories', path: '/api/repositories' },
    { label: 'Repository log', path: '/api/repositories/local/log' },
    { label: 'Repository tickets', path: '/api/repositories/local/tickets' },
    { label: 'Runs', path: '/api/runs' },
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
  let objectiveDetail = $state<ObjectiveDetail | null>(null);

  let workspaceError = $state<string | null>(null);
  let hostsError = $state<string | null>(null);
  let workersError = $state<string | null>(null);
  let repositoryError = $state<string | null>(null);
  let repositoryLogError = $state<string | null>(null);
  let repositoryTicketsError = $state<string | null>(null);
  let objectivesError = $state<string | null>(null);
  let objectiveDetailError = $state<string | null>(null);
  let objectiveDetailLoading = $state(false);
  let objectiveDetailRequest = 0;
  let route = $derived(routeFromView(view, objectiveId));
  let currentPath = $derived(pathFromRoute(route));

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

  async function loadObjectiveDetail(id: string) {
    const request = ++objectiveDetailRequest;
    objectiveDetailLoading = true;
    objectiveDetailError = null;
    objectiveDetail = null;
    try {
      const detail = await getJson<ObjectiveDetail>(`/api/objectives/${encodeURIComponent(id)}`);
      if (request === objectiveDetailRequest) {
        objectiveDetail = detail;
      }
    } catch (error) {
      if (request === objectiveDetailRequest) {
        objectiveDetailError = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (request === objectiveDetailRequest) {
        objectiveDetailLoading = false;
      }
    }
  }

  function diagnosticsFor(...groups: Array<Diagnostic[] | undefined>): Diagnostic[] {
    return groups.flatMap((group) => group ?? []);
  }

  function routeFromView(view: WorkspaceView, objectiveId: string | null): RouteState {
    if (view === 'repository') {
      return { page: 'repository' };
    }
    if (view === 'objective' && objectiveId) {
      return { page: 'objective', objectiveId };
    }
    if (view === 'objectives') {
      return { page: 'objectives' };
    }
    return { page: 'overview' };
  }

  function pathFromRoute(route: RouteState): string {
    if (route.page === 'repository') {
      return '/repositories/local';
    }
    if (route.page === 'objective') {
      return `/objectives/${route.objectiveId}`;
    }
    if (route.page === 'objectives') {
      return '/objectives';
    }
    return '/';
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

  $effect(() => {
    const selectedObjectiveId = route.page === 'objective' ? route.objectiveId : null;
    if (selectedObjectiveId) {
      void loadObjectiveDetail(selectedObjectiveId);
    } else {
      objectiveDetailRequest += 1;
      objectiveDetail = null;
      objectiveDetailError = null;
      objectiveDetailLoading = false;
    }
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
  <WorkspaceSidebar {workspace} {workspaceError} {currentPath} />

  <main class="shell">
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
          <RepositoryTicketKanban tickets={repositoryTickets} />
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
    {:else if route.page === 'objectives' || route.page === 'objective'}
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
                <article class="runtime-card selected-card" class:selected={route.page === 'objective' && route.objectiveId === objective.id}>
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
                  <p><a href={`/objectives/${objective.id}`}>View detail</a></p>
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

      {#if route.page === 'objective'}
        <section class="card">
          <h2>Objective detail</h2>
          {#if objectiveDetail}
            <div class="detail-heading">
              <h3>{objectiveDetail.title}</h3>
              <span>{objectiveDetail.state}</span>
            </div>
            <dl>
              <div>
                <dt>ID</dt>
                <dd><code>{objectiveDetail.id}</code></dd>
              </div>
              <div>
                <dt>Updated</dt>
                <dd>{formatDate(objectiveDetail.updated_at)}</dd>
              </div>
              <div>
                <dt>Created</dt>
                <dd>{formatDate(objectiveDetail.created_at)}</dd>
              </div>
              <div>
                <dt>Linked tickets</dt>
                <dd>{objectiveDetail.linked_tickets.length ? objectiveDetail.linked_tickets.join(', ') : 'none'}</dd>
              </div>
              <div>
                <dt>Source</dt>
                <dd>{objectiveDetail.record_source}</dd>
              </div>
            </dl>
            {#if objectiveDetail.body_truncated}
              <p class="section-note">Objective body was truncated by the backend response limit.</p>
            {/if}
            <pre class="record-body">{objectiveDetail.body || 'No Objective body text is available.'}</pre>
          {:else if objectiveDetailError}
            <p class="error">{objectiveDetailError}</p>
          {:else if objectiveDetailLoading}
            <p>Loading Objective <code>{route.objectiveId}</code>…</p>
          {:else}
            <p>Waiting for Objective detail…</p>
          {/if}
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
                        <dt>Runtime</dt>
                        <dd><code>{host.runtime_id}</code></dd>
                      </div>
                      <div>
                        <dt>Scope</dt>
                        <dd>{host.capabilities.workspace_scope}</dd>
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
                        <td>{worker.workspace.visibility} · {worker.workspace.identity}</td>
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
