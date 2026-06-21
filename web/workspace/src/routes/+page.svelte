<script lang="ts">
  type WorkspaceResponse = {
    workspace_id: string;
    display_name: string;
    record_authority: string;
    extension_points: {
      event_stream: { status: string; note: string };
      runner_connection: { status: string; note: string };
    };
  };

  const endpoints = [
    { label: 'Workspace', path: '/api/workspace' },
    { label: 'Tickets', path: '/api/tickets' },
    { label: 'Objectives', path: '/api/objectives' },
    { label: 'Runs', path: '/api/runs' },
    { label: 'Runners', path: '/api/runners' }
  ];

  let workspace = $state<WorkspaceResponse | null>(null);
  let loadError = $state<string | null>(null);

  async function loadWorkspace() {
    try {
      const response = await fetch('/api/workspace');
      if (!response.ok) {
        throw new Error(`GET /api/workspace failed: ${response.status}`);
      }
      workspace = await response.json();
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    }
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
      through bounded backend APIs. Ticket and Objective lifecycle authority stays
      in the existing local record workflow.
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
        Event streams and runner connections are represented as extension-point
        state in the backend response, but no scheduler, write API, or hosted
        multi-tenant behavior is implemented in this slice.
      </p>
    </div>
  </section>
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
    width: min(980px, calc(100vw - 32px));
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
  }

  .card {
    border: 1px solid rgba(148, 163, 184, 0.25);
    border-radius: 20px;
    background: rgba(15, 23, 42, 0.75);
    padding: 24px;
    box-shadow: 0 24px 80px rgba(15, 23, 42, 0.35);
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
  }

  .error {
    color: #fca5a5;
  }
</style>
