<script lang="ts">
  import RepositoryTicketKanban from '$lib/workspace-pages/RepositoryTicketKanban.svelte';
  import { workerConsoleHref } from '$lib/workspace-console/model';
  import WorkspaceSidebar from '$lib/workspace-sidebar/WorkspaceSidebar.svelte';
  import type {
    Host,
    ListResponse,
    ObjectiveDetail,
    ObjectiveListResponse,
    RepositoryDetailResponse,
    RepositorySummary,
    RepositoryTicketsResponse,
    Worker,
    WorkspaceResponse
  } from '$lib/workspace-sidebar/types';

  type WorkspaceView = 'overview' | 'repository' | 'objectives' | 'objective';

  type RouteState =
    | { page: 'overview'; objectiveId?: undefined; repositoryId?: undefined }
    | { page: 'repository'; repositoryId: string; objectiveId?: undefined }
    | { page: 'objectives'; objectiveId?: undefined; repositoryId?: undefined }
    | { page: 'objective'; objectiveId: string; repositoryId?: undefined };

  let {
    view = 'overview',
    objectiveId = null,
    repositoryId = 'main'
  }: { view?: WorkspaceView; repositoryId?: string; objectiveId?: string | null } = $props();

  let workspace = $state<WorkspaceResponse | null>(null);
  let hosts = $state<ListResponse<Host> | null>(null);
  let workers = $state<ListResponse<Worker> | null>(null);
  let repository = $state<RepositorySummary | null>(null);
  let repositoryTickets = $state<RepositoryTicketsResponse | null>(null);
  let objectives = $state<ObjectiveListResponse | null>(null);
  let objectiveDetail = $state<ObjectiveDetail | null>(null);

  let workspaceError = $state<string | null>(null);
  let hostsError = $state<string | null>(null);
  let workersError = $state<string | null>(null);
  let repositoryError = $state<string | null>(null);
  let repositoryTicketsError = $state<string | null>(null);
  let objectivesError = $state<string | null>(null);
  let objectiveDetailError = $state<string | null>(null);
  let objectiveDetailLoading = $state(false);
  let objectiveDetailRequest = 0;
  let route = $derived(routeFromView(view, objectiveId, repositoryId));
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
    const selectedRepositoryId = route.page === 'repository' ? route.repositoryId : repositoryId;
    try {
      const detail = await getJson<RepositoryDetailResponse>(
        `/api/repositories/${encodeURIComponent(selectedRepositoryId)}`
      );
      repository = detail.item;
    } catch (error) {
      repositoryError = error instanceof Error ? error.message : String(error);
      repository = null;
    }
  }

  async function loadRepositoryTickets() {
    repositoryTicketsError = null;
    const selectedRepositoryId = route.page === 'repository' ? route.repositoryId : repositoryId;
    try {
      repositoryTickets = await getJson<RepositoryTicketsResponse>(
        `/api/repositories/${encodeURIComponent(selectedRepositoryId)}/tickets`
      );
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

  function routeFromView(
    view: WorkspaceView,
    objectiveId: string | null,
    repositoryId: string
  ): RouteState {
    if (view === 'repository') {
      return { page: 'repository', repositoryId };
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
      return `/repositories/${route.repositoryId}`;
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

  $effect(() => {
    void loadWorkspace();
    void loadHosts();
    void loadWorkers();
    void loadRepository();
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
      <section class="card">
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
              <dt>Provider</dt>
              <dd>{repository.provider}</dd>
            </div>
            <div>
              <dt>Default selector</dt>
              <dd>{repository.default_selector ?? 'none configured'}</dd>
            </div>
            <div>
              <dt>Record authority</dt>
              <dd>{repository.record_authority}</dd>
            </div>
            <div>
              <dt>Git</dt>
              <dd>{repository.git?.status ?? 'not available'}</dd>
            </div>
            {#if repository.diagnostics && repository.diagnostics.length > 0}
              <div>
                <dt>Diagnostics</dt>
                <dd>
                  <ul>
                    {#each repository.diagnostics as diagnostic}
                      <li><code>{diagnostic.code}</code>: {diagnostic.message}</li>
                    {/each}
                  </ul>
                </dd>
              </div>
            {/if}
          </dl>
        {:else if repositoryError}
          <p class="error">{repositoryError}</p>
        {:else}
          <p>Waiting for <code>/api/repositories/{route.repositoryId}</code>…</p>
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
          <p>Waiting for <code>/api/repositories/{route.repositoryId}/tickets</code>…</p>
        {/if}
      </section>

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
            <div class="objective-list">
              {#each objectives.items as objective (objective.id)}
                <a class="objective-row" class:selected={route.page === 'objective' && route.objectiveId === objective.id} href={`/objectives/${objective.id}`}>
                  <div class="objective-main">
                    <div class="objective-title-row">
                      <strong class="objective-title">{objective.title}</strong>
                      <span class="state-pill">{objective.state}</span>
                    </div>
                    <p class="objective-summary">{objective.summary || 'No summary text is available.'}</p>
                  </div>
                  <div class="objective-meta" aria-label="Objective metadata">
                    <span>Updated {formatDate(objective.updated_at)}</span>
                    <span>{objective.linked_tickets?.length ? `${objective.linked_tickets.length} linked ticket(s)` : 'No linked tickets'}</span>
                    <code>{objective.id}</code>
                  </div>
                </a>
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
                      <th>Attach</th>
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
                        <td><a class="inline-link" href={workerConsoleHref(worker)}>Open Console</a></td>
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

    {/if}
  </main>
</div>
