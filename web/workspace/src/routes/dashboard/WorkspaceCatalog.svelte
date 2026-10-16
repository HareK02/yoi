<script lang="ts">
  import { onMount } from "svelte";
  import {
    loadWorkspaceCatalog,
    type WorkspaceCatalogItem,
  } from "#lib/workspace/api/workspace-catalog.ts";

  import type { WorkspaceCreation } from "./workspace-creation.svelte.ts";
  let { creation }: { creation: WorkspaceCreation } = $props();
  let disposed = false;
  let request = 0;
  let workspaces = $state<WorkspaceCatalogItem[]>([]);
  let catalogError = $state<string | null>(null);
  let refreshing = $state(false);

  onMount(() => {
    if (window.location.hash === "#workspace-create-title") creation.open = true;
    const hashChanged = () => { if (window.location.hash === "#workspace-create-title") creation.open = true; };
    window.addEventListener("hashchange", hashChanged);
    void refreshCatalog();
    return () => { disposed = true; ++request; window.removeEventListener("hashchange", hashChanged); };
  });

  async function refreshCatalog() {
    const current = ++request;
    refreshing = true;
    workspaces = [];
    catalogError = null;
    try {
      const result = await loadWorkspaceCatalog(fetch);
      if (!disposed && current === request) workspaces = result;
    } catch {
      if (!disposed && current === request) catalogError = "Workspaces could not be loaded. Check your session or retry.";
    } finally {
      if (!disposed && current === request) refreshing = false;
    }
  }

  function submitCreation(event: SubmitEvent) {
    event.preventDefault();
    void creation.submit();
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

<div class="workspace-catalog-shell">
  <section class="workspace-catalog-heading">
    <div>
      <p>Choose a Workspace to open its dashboard. Workspace settings are managed inside each Workspace.</p>
    </div>
    <button class="workspace-secondary-action" onclick={refreshCatalog} disabled={refreshing}>
      {refreshing ? "Refreshing…" : "Refresh"}
    </button>
  </section>

  {#if catalogError}
    <div class="workspace-catalog-alert" role="alert">
      {catalogError}
    </div>
  {/if}

  <section aria-labelledby="workspace-list-title">
    <h2 id="workspace-list-title">Choose Workspace</h2>
    {#if refreshing}
      <p role="status">Loading Workspaces…</p>
    {:else if catalogError}
      <a href="/account">Check Account</a>
    {:else if workspaces.length === 0}
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
            {#if workspace.repositories[0]}
              <span class="workspace-repository-summary">
                {workspace.repositories[0].repository_key}
                <small>
                  {workspace.repositories[0].default_selector ?? "repository default"} ·
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

  <details class="workspace-create-panel" bind:open={creation.open}>
    <summary id="workspace-create-title">Create Workspace</summary>
    <div class="workspace-create-content">
    <div>
      <p>You can add repositories later in Workspace Settings.</p>
    </div>
    {#if creation.createdWorkspaceId}
      <p role="status">Workspace creation confirmed.</p>
      <a href={`/w/${encodeURIComponent(creation.createdWorkspaceId)}`}>Open created Workspace</a>
    {:else}
    <form onsubmit={submitCreation}>
      <label>
        Workspace display name
        <input bind:value={creation.displayName} required autocomplete="off" />
      </label>
      <label class="workspace-repository-toggle">
        <input type="checkbox" bind:checked={creation.includeRepository} aria-controls={creation.includeRepository ? "initial-repository-fields" : undefined} />
        Add an initial repository (optional)
      </label>
      {#if creation.includeRepository}
        <div id="initial-repository-fields" class="workspace-repository-fields">
          <p>
            Repository sources are interpreted by Backend authority. Supported Git sources are absolute local paths, file://, ssh://, http(s)://, and user@host:path; Browser-local paths and embedded credentials are not authority. Plain HTTP is unencrypted, so prefer HTTPS or SSH.
          </p>
          <label>
            Initial repository absolute path or URI
            <input bind:value={creation.repositoryUri} required autocomplete="off" />
          </label>
          <div class="workspace-create-row">
            <label>
              Repository key
              <input bind:value={creation.repositoryKey} required pattern="[a-z0-9]|[a-z0-9](?:[a-z0-9]|-)*[a-z0-9]" maxlength="64" autocomplete="off" />
            </label>
            <label>
              Default ref
              <input bind:value={creation.defaultRef} placeholder="repository default" autocomplete="off" />
            </label>
          </div>
        </div>
      {/if}
      {#if creation.error}
        <div class="workspace-catalog-alert" role="alert">{creation.error}</div>
      {/if}
      <button class="workspace-primary-action" type="submit" disabled={creation.creating}>
        {creation.creating ? "Creating…" : creation.error ? "Retry creation" : "Create Workspace"}
      </button>
    </form>
    {/if}
    </div>
  </details>
</div>

<style>
  .workspace-catalog-shell { display: grid; gap: var(--space-5); min-width: 0; }
  .workspace-catalog-heading, .workspace-card-heading { display: flex; justify-content: space-between; align-items: center; gap: var(--space-4); }
  h2 { margin: 0 0 var(--space-3); font-size: var(--font-size-body); }
  p { margin: 0; color: var(--text-muted); }
  .workspace-catalog-grid { display: grid; }
  .workspace-catalog-card { display: grid; gap: var(--space-2); padding-block: var(--space-4); border-bottom: 1px solid var(--line); min-width: 0; overflow-wrap: anywhere; text-decoration: none; color: var(--text-strong); }
  .workspace-catalog-card:hover { background: var(--interactive-hover); }
  .workspace-card-heading strong { min-width: 0; }
  .workspace-card-heading > span { flex: 0 0 auto; font-size: var(--font-size-compact); color: var(--text-muted); }
  .workspace-card-heading > .workspace-state-active { color: var(--success); }
  small { color: var(--text-muted); font-size: var(--font-size-compact); }
  .workspace-repository-summary { display: grid; gap: var(--space-1); }
  .workspace-empty-state { padding-block: var(--space-4); }
  .workspace-create-panel, .workspace-create-content, form { min-width: 0; }
  summary { cursor: pointer; font-weight: 700; }
  .workspace-create-content, form, .workspace-repository-fields { display: grid; gap: var(--space-4); }
  .workspace-create-content { padding-block: var(--space-4); }
  label { display: grid; gap: var(--space-2); min-width: 0; }
  input { width: 100%; min-width: 0; padding: var(--space-2) var(--space-3); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: var(--bg); color: var(--text-strong); font: inherit; }
  .workspace-repository-toggle { display: flex; align-items: center; gap: var(--space-2); }
  input[type=checkbox] { width: auto; }
  .workspace-create-row { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(180px, 100%), 1fr)); gap: var(--space-4); }
  button { justify-self: start; min-height: 2.25rem; padding: var(--space-2) var(--space-3); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: transparent; color: var(--text-strong); font: inherit; cursor: pointer; }
  .workspace-primary-action { background: var(--text-strong); color: var(--bg); }
  button:hover:not(:disabled), summary:hover { background: var(--interactive-hover); color: var(--text-strong); }
  :is(a, button, summary):active { background: var(--interactive-selected); color: var(--text-strong); }
  button:disabled { opacity: .55; cursor: not-allowed; }
  :is(a, button, input, summary):focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .workspace-catalog-alert { color: var(--danger); }
  @media (max-width: 760px) { .workspace-catalog-heading { align-items: start; flex-direction: column; } }
</style>
