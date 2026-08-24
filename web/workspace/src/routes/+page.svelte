<script lang="ts">
  import { goto } from "$app/navigation";
  import {
    createOperationKey,
    createWorkspace,
    creationErrorMessage,
    loadWorkspaceCatalog,
    type CreateWorkspaceRequest,
    type WorkspaceCatalogItem,
  } from "$lib/workspace/api/workspace-catalog";
  import "$lib/workspace/styles/workspace-catalog.css";

  let { data } = $props();
  let workspaces = $state<WorkspaceCatalogItem[]>([]);
  let catalogError = $state<string | null>(null);
  let refreshing = $state(false);
  let creating = $state(false);
  let creationError = $state<string | null>(null);
  let displayName = $state("");
  let repositoryUri = $state("");
  let repositoryName = $state("Main");
  let defaultRef = $state("");
  let lastSubmission = $state<{
    signature: string;
    request: CreateWorkspaceRequest;
  } | null>(null);

  $effect(() => {
    workspaces = data.workspaces;
    catalogError = data.catalogError;
  });

  async function refreshCatalog() {
    refreshing = true;
    catalogError = null;
    try {
      workspaces = await loadWorkspaceCatalog(fetch);
    } catch (error) {
      catalogError = error instanceof Error ? error.message : String(error);
    } finally {
      refreshing = false;
    }
  }

  async function submitCreation(event: SubmitEvent) {
    event.preventDefault();
    if (creating) return;
    const normalized = {
      displayName: displayName.trim(),
      repositoryUri: repositoryUri.trim(),
      repositoryName: repositoryName.trim(),
      defaultRef: defaultRef.trim(),
    };
    const signature = JSON.stringify(normalized);
    const request = lastSubmission?.signature === signature
      ? lastSubmission.request
      : {
        operation_key: createOperationKey(),
        display_name: normalized.displayName,
        repository: {
          uri: normalized.repositoryUri,
          display_name: normalized.repositoryName || null,
          default_ref: normalized.defaultRef || null,
        },
      };
    lastSubmission = { signature, request };
    creating = true;
    creationError = null;
    try {
      const response = await createWorkspace(fetch, request);
      await goto(`/w/${encodeURIComponent(response.workspace.workspace_id)}`);
    } catch (error) {
      creationError = creationErrorMessage(error);
    } finally {
      creating = false;
    }
  }

  function formatUpdated(value: string): string {
    const timestamp = Date.parse(value);
    return Number.isNaN(timestamp)
      ? value
      : new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(timestamp);
  }
</script>

<svelte:head>
  <title>Workspaces · Yoi</title>
</svelte:head>

<div class="workspace-catalog-shell">
  <section class="workspace-catalog-heading">
    <div>
      <p class="workspace-catalog-eyebrow">Backend</p>
      <h1>Workspaces</h1>
      <p>Select an accessible team space or create one on this Backend.</p>
    </div>
    <button class="workspace-secondary-action" onclick={refreshCatalog} disabled={refreshing}>
      {refreshing ? "Refreshing…" : "Refresh"}
    </button>
  </section>

  {#if catalogError}
    <div class="workspace-catalog-alert" role="alert">
      Refresh failed. Existing results were kept. {catalogError}
    </div>
  {/if}

  <section aria-labelledby="workspace-list-title">
    <h2 id="workspace-list-title">Available Workspaces</h2>
    {#if workspaces.length === 0}
      <div class="workspace-empty-state">
        <strong>No accessible Workspaces</strong>
        <p>Create the first Workspace if you have Backend permission.</p>
      </div>
    {:else}
      <div class="workspace-catalog-grid">
        {#each workspaces as workspace (workspace.workspace_id)}
          <a
            class="workspace-catalog-card"
            href={`/w/${encodeURIComponent(workspace.workspace_id)}`}
          >
            <span class="workspace-card-heading">
              <strong>{workspace.display_name}</strong>
              <span class:workspace-state-active={workspace.state === "active"}>
                {workspace.state}
              </span>
            </span>
            <code>{workspace.workspace_id}</code>
            {#if workspace.repositories[0]}
              <span class="workspace-repository-summary">
                {workspace.repositories[0].name}
                <small>
                  {workspace.repositories[0].default_ref ?? "repository default"} ·
                  {workspace.repositories[0].kind}
                </small>
              </span>
            {:else if workspace.repository_error}
              <small>Repository summary unavailable</small>
            {:else}
              <small>No repositories</small>
            {/if}
            <small>Updated {formatUpdated(workspace.updated_at)}</small>
          </a>
        {/each}
      </div>
    {/if}
  </section>

  <section class="workspace-create-panel" aria-labelledby="workspace-create-title">
    <div>
      <p class="workspace-catalog-eyebrow">New team space</p>
      <h2 id="workspace-create-title">Create Workspace</h2>
      <p>
        Repository sources are interpreted by Backend authority. Supported Git sources are absolute local paths, file://, ssh://, http(s)://, and user@host:path; Browser-local paths and embedded credentials are not authority.
      </p>
    </div>
    <form onsubmit={submitCreation}>
      <label>
        Workspace display name
        <input bind:value={displayName} required autocomplete="off" />
      </label>
      <label>
        Initial repository absolute path or URI
        <input bind:value={repositoryUri} required autocomplete="off" />
      </label>
      <div class="workspace-create-row">
        <label>
          Repository display name
          <input bind:value={repositoryName} autocomplete="off" />
        </label>
        <label>
          Default ref
          <input bind:value={defaultRef} placeholder="repository default" autocomplete="off" />
        </label>
      </div>
      {#if creationError}
        <div class="workspace-catalog-alert" role="alert">{creationError}</div>
      {/if}
      <button class="workspace-primary-action" type="submit" disabled={creating}>
        {creating ? "Creating…" : creationError ? "Retry creation" : "Create Workspace"}
      </button>
    </form>
  </section>
</div>
