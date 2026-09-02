<script lang="ts">
  import type {
    Diagnostic,
    WorkspaceMetadataSettingsResponse,
  } from '$lib/generated/workspace-api';
  import DiagnosticsList from '$lib/workspace/settings/DiagnosticsList.svelte';
  import {
    fetchWorkspaceMetadata,
    updateWorkspaceMetadata,
  } from '$lib/workspace/settings/profile-api';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? '');

  let workspaceMetadata = $state<WorkspaceMetadataSettingsResponse | null>(null);
  let displayNameDraft = $state('');
  let loading = $state(true);
  let submitting = $state(false);
  let message = $state<string | null>(null);
  let diagnostics = $state<Diagnostic[]>([]);

  $effect(() => {
    if (!workspaceId) {
      loading = false;
      return;
    }
    let cancelled = false;
    async function load() {
      loading = true;
      message = null;
      try {
        const response = await fetchWorkspaceMetadata(workspaceId);
        if (!cancelled) {
          workspaceMetadata = response;
          displayNameDraft = response.display_name;
          diagnostics = response.diagnostics;
        }
      } catch (err) {
        if (!cancelled) {
          message = err instanceof Error ? err.message : 'workspace settings request failed';
        }
      } finally {
        if (!cancelled) loading = false;
      }
    }
    load();
    return () => {
      cancelled = true;
    };
  });

  async function submitWorkspaceName() {
    if (!workspaceMetadata) return;
    submitting = true;
    message = null;
    try {
      const response = await updateWorkspaceMetadata(workspaceId, {
        display_name: displayNameDraft,
        revision: workspaceMetadata.revision
      });
      workspaceMetadata = response.workspace;
      displayNameDraft = response.workspace.display_name;
      diagnostics = response.diagnostics.concat(response.workspace.diagnostics);
      message = 'Workspace display name updated.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'workspace update failed';
    } finally {
      submitting = false;
    }
  }
</script>

<svelte:head>
  <title>Workspace settings · Yoi Workspace</title>
</svelte:head>

<section class="card settings-section" aria-labelledby="workspace-settings-title">
  <header class="settings-section-header">
    <div>
      <p class="eyebrow">editable</p>
      <h2 id="workspace-settings-title">Workspace Identity</h2>
    </div>
    <span class="badge success">Backend scoped</span>
  </header>

  {#if loading}
    <p class="status-message">Loading workspace settings…</p>
  {:else}
    <form class="settings-form" onsubmit={(event) => { event.preventDefault(); void submitWorkspaceName(); }}>
      <label>
        <span>Display name</span>
        <input bind:value={displayNameDraft} autocomplete="off" />
      </label>
      <p class="settings-note">Workspace id: <code>{workspaceMetadata?.workspace_id ?? workspaceId}</code></p>
      <button type="submit" disabled={submitting || !workspaceMetadata}>{submitting ? 'Saving…' : 'Save workspace name'}</button>
    </form>

    <dl class="settings-identity-list">
      <div>
        <dt>Source</dt>
        <dd>{workspaceMetadata?.source ?? 'unknown'}</dd>
      </div>
      <div>
        <dt>Revision</dt>
        <dd><code>{workspaceMetadata?.revision ?? 'unknown'}</code></dd>
      </div>
    </dl>
  {/if}

  {#if message}
    <p class="status-message" class:error={message.includes('failed')}>{message}</p>
  {/if}
  <DiagnosticsList {diagnostics} />
</section>
