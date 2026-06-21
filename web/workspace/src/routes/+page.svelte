<script lang="ts">
  type WorkspaceResponse = {
    workspace_id: string;
    display_name: string;
    record_authority: string;
    extension_points: {
      event_stream: { status: string; note: string };
      host_worker_bridge: { status: string; note: string };
    };
  };

  type Diagnostic = {
    code: string;
    severity: string;
    message: string;
  };

  type Host = {
    host_id: string;
    label: string;
    kind: string;
    status: string;
    observed_at: string;
    last_seen_at: string;
    capabilities: {
      local_pod_inspection: string;
      workspace_root: string;
      os: string;
      arch: string;
      max_workers: number;
    };
    diagnostics: Diagnostic[];
  };

  type Worker = {
    worker_id: string;
    host_id: string;
    label: string;
    pod_name: string;
    role?: string;
    profile?: string;
    workspace_root?: string;
    state: string;
    status: string;
    last_seen_at?: string;
    implementation: { kind: string; pod_name: string };
    diagnostics: Diagnostic[];
  };

  type ListResponse<T> = {
    workspace_id: string;
    limit: number;
    items: T[];
    source: string;
    diagnostics: Diagnostic[];
  };

  const endpoints = [
    { label: 'Workspace', path: '/api/workspace' },
    { label: 'Tickets', path: '/api/tickets' },
    { label: 'Objectives', path: '/api/objectives' },
    { label: 'Runs', path: '/api/runs' },
    { label: 'Hosts', path: '/api/hosts' },
    { label: 'Workers', path: '/api/workers' }
  ];

  let workspace = $state<WorkspaceResponse | null>(null);
  let hosts = $state<ListResponse<Host> | null>(null);
  let workers = $state<ListResponse<Worker> | null>(null);
  let loadError = $state<string | null>(null);

  async function getJson<T>(path: string): Promise<T> {
    const response = await fetch(path);
    if (!response.ok) {
      throw new Error(`GET ${path} failed: ${response.status}`);
    }
    return response.json() as Promise<T>;
  }

  async function loadWorkspace() {
    try {
      const [workspaceResponse, hostResponse, workerResponse] = await Promise.all([
        getJson<WorkspaceResponse>('/api/workspace'),
        getJson<ListResponse<Host>>('/api/hosts'),
        getJson<ListResponse<Worker>>('/api/workers')
      ]);
      workspace = workspaceResponse;
      hosts = hostResponse;
      workers = workerResponse;
      loadError = null;
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    }
  }

  function diagnosticsFor(...groups: Array<Diagnostic[] | undefined>): Diagnostic[] {
    return groups.flatMap((group) => group ?? []);
  }

  $effect(() => {
    void loadWorkspace();
  });
</script>

<svelte:head>
  <title>Yoi Workspace Control Plane</title>
  <meta
    name="description"
    content="Local single-workspace Yoi control plane bootstrap"
  />
</svelte:head>

<main class="shell">
  <section class="hero">
    <p class="eyebrow">Local / single-workspace bootstrap</p>
    <h1>Yoi Workspace Control Plane</h1>
    <p>
      Static SPA shell for reading canonical <code>.yoi</code> project records
      and the local Host / Worker execution view through bounded backend APIs.
      Ticket and Objective lifecycle authority stays in the existing local record
      workflow.
    </p>
  </section>

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
    {:else if loadError}
      <p class="error">{loadError}</p>
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
                    <dt>Local Pod inspection</dt>
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
      {:else if loadError}
        <p class="error">{loadError}</p>
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
                    <td>{worker.implementation.kind}: {worker.implementation.pod_name}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else if loadError}
        <p class="error">{loadError}</p>
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
</main>

<style>
  :global(body) {
    margin: 0;
    background: #0f172a;
    color: #e2e8f0;
    font-family:
      Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
  }

  .shell {
    width: min(1120px, calc(100vw - 32px));
    margin: 0 auto;
    padding: 48px 0;
  }

  .hero {
    margin-bottom: 24px;
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
  }

  h2 {
    margin-top: 0;
  }

  code {
    color: #bae6fd;
  }

  .grid {
    display: grid;
    gap: 16px;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    margin-top: 16px;
  }

  .runtime {
    grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
  }

  .card {
    border: 1px solid rgba(148, 163, 184, 0.25);
    border-radius: 20px;
    background: rgba(15, 23, 42, 0.75);
    padding: 24px;
    box-shadow: 0 24px 80px rgba(15, 23, 42, 0.35);
  }

  .stack {
    display: grid;
    gap: 12px;
  }

  .runtime-card {
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 16px;
    padding: 16px;
    background: rgba(15, 23, 42, 0.55);
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

  .diagnostics {
    margin-top: 16px;
  }

  .diagnostics li {
    display: grid;
    gap: 4px;
    margin-bottom: 12px;
  }

  .error {
    color: #fca5a5;
  }
</style>
