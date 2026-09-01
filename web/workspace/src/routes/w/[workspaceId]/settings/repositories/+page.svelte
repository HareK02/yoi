<script lang="ts">
  import { invalidateAll } from '$app/navigation';
  import { workspaceApiPath, workspaceRoute } from '$lib/workspace/api/http';
  import type { RepositorySourceKind } from '$lib/generated/workspace-api';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  function sourceLabel(kind: RepositorySourceKind): string {
    if (kind === 'local_path' || kind === 'file') return 'Local';
    if (kind === 'invalid') return 'Invalid';
    return 'Remote Git';
  }

  function supportsRepositoryAccess(kind: RepositorySourceKind): boolean {
    return kind === 'ssh' || kind === 'http' || kind === 'https';
  }
  let showAddRepository = $state(false);
  let repositoryId = $state('');
  let displayName = $state('');
  let source = $state('');
  let defaultRef = $state('');
  let pending = $state(false);
  let requestError = $state<string | null>(null);

  async function responseError(response: Response): Promise<string> {
    const payload = await response.json().catch(() => null) as
      | { message?: string; error?: string }
      | null;
    return payload?.message ?? payload?.error ?? `Request failed (${response.status})`;
  }

  async function addRepository(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    pending = true;
    requestError = null;
    try {
      const response = await fetch(workspaceApiPath(data.workspaceId, '/repositories'), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          repository_id: repositoryId,
          display_name: displayName,
          source,
          default_ref: defaultRef || null,
        }),
      });
      if (!response.ok) throw new Error(await responseError(response));
      repositoryId = '';
      displayName = '';
      source = '';
      defaultRef = '';
      showAddRepository = false;
      await invalidateAll();
    } catch (error) {
      requestError = error instanceof Error ? error.message : String(error);
    } finally {
      pending = false;
    }
  }
</script>

<svelte:head>
  <title>Repositories · Settings · Yoi Workspace</title>
  <meta name="description" content="Workspace Repository resources" />
</svelte:head>

<section class="repositories-page" aria-labelledby="repositories-heading">
  <header class="page-header-row">
    <div>
      <p class="eyebrow">owner only</p>
      <h1 id="repositories-heading">Repositories</h1>
      <p>Register the local and remote Git repositories available to this Workspace.</p>
    </div>
    <button type="button" onclick={() => showAddRepository = !showAddRepository}>
      {showAddRepository ? 'Close' : 'Add Repository'}
    </button>
  </header>

  {#if showAddRepository}
    <form class="settings-repository-form" onsubmit={addRepository}>
      <h2>Add Repository</h2>
      <div class="settings-form-grid">
        <label>
          Repository ID
          <input bind:value={repositoryId} required pattern="[A-Za-z0-9_.-]+" maxlength="128" autocomplete="off" />
        </label>
        <label>
          Display name
          <input bind:value={displayName} required maxlength="256" autocomplete="off" />
        </label>
        <label class="settings-form-field-wide">
          Source
          <input bind:value={source} required autocomplete="off" placeholder="/absolute/path or ssh://git@example.test/org/repository.git" />
          <small>Registration validates the source without accessing the filesystem or network.</small>
        </label>
        <label>
          Default ref
          <input bind:value={defaultRef} maxlength="512" autocomplete="off" placeholder="main" />
        </label>
      </div>
      <p class="status-message">
        SSH credentials and pinned host keys are managed separately in
        <a class="inline-link" href={workspaceRoute(data.workspaceId, '/settings/repository-access')}>Repository Access</a>.
      </p>
      <div class="settings-action-row">
        <button type="submit" disabled={pending}>{pending ? 'Adding…' : 'Add Repository'}</button>
        <button type="button" disabled={pending} onclick={() => showAddRepository = false}>Cancel</button>
      </div>
    </form>
  {/if}

  {#if requestError}
    <p class="section-state error">{requestError}</p>
  {/if}

  {#if data.repositoriesError}
    <p class="section-state error">{data.repositoriesError}</p>
  {:else if !data.repositories}
    <p class="section-state">Loading Repositories…</p>
  {:else if data.repositories.items.length === 0}
    <p class="section-state">No Repositories are registered.</p>
  {:else}
    <div class="settings-repository-table-wrap">
      <table class="settings-repository-table">
        <thead>
          <tr>
            <th>Repository</th>
            <th>Source</th>
            <th>Default ref</th>
            <th>Status</th>
            <th>Access</th>
          </tr>
        </thead>
        <tbody>
          {#each data.repositories.items as repository (repository.id)}
            <tr>
              <td>
                <a class="inline-link" href={workspaceRoute(data.workspaceId, `/repositories/${encodeURIComponent(repository.id)}`)}>
                  <strong>{repository.display_name}</strong>
                </a>
                <small><code>{repository.id}</code></small>
              </td>
              <td>
                <span>{sourceLabel(repository.source.kind)}</span>
                <small><code>{repository.source.uri}</code></small>
              </td>
              <td>{repository.default_selector ?? '—'}</td>
              <td>{repository.observed_status}</td>
              <td>
                {#if supportsRepositoryAccess(repository.source.kind)}
                  <a class="inline-link" href={workspaceRoute(data.workspaceId, '/settings/repository-access')}>Configure access</a>
                {:else}
                  <span class="settings-muted-action">Not required</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
