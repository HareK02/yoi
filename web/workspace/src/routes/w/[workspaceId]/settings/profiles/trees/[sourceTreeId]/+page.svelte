<script lang="ts">
  import DiagnosticsList from '$lib/workspace-settings/DiagnosticsList.svelte';
  import DecodalSourceEditor from '$lib/workspace-settings/DecodalSourceEditor.svelte';
  import {
    deleteProfileTreeFile,
    fetchProfileSourceTree,
    fetchProfileTreeFile,
    writeProfileTreeFile,
  } from '$lib/workspace-settings/profile-api';
  import { profileSettingsHref, virtualProfilePathForCreate } from '$lib/workspace-settings/profile-routes';
  import type { Diagnostic } from '$lib/workspace-settings/model';
  import type {
    WorkspaceProfileSourceTreeFileResponse,
    WorkspaceProfileSourceTreeResponse,
  } from '$lib/workspace-settings/profile-types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? '');
  let sourceTreeId = $derived(data.sourceTreeId);

  let tree = $state<WorkspaceProfileSourceTreeResponse | null>(null);
  let selectedFile = $state<WorkspaceProfileSourceTreeFileResponse | null>(null);
  let draftContent = $state('');
  let newFilePath = $state('new-profile.dcdl');
  let loading = $state(true);
  let saving = $state(false);
  let creating = $state(false);
  let deleting = $state(false);
  let message = $state<string | null>(null);
  let diagnostics = $state<Diagnostic[]>([]);

  async function reloadTree() {
    tree = await fetchProfileSourceTree(workspaceId, sourceTreeId);
    diagnostics = tree.diagnostics;
  }

  async function selectFile(path: string) {
    selectedFile = await fetchProfileTreeFile(workspaceId, sourceTreeId, path);
    draftContent = selectedFile.content;
    diagnostics = selectedFile.diagnostics;
  }

  async function createFile() {
    creating = true;
    message = null;
    try {
      selectedFile = await writeProfileTreeFile(workspaceId, sourceTreeId, {
        path: virtualProfilePathForCreate(newFilePath),
        content: '{\n  slug = "new-profile";\n  description = "New profile";\n  scope = "workspace_read";\n}',
      });
      draftContent = selectedFile.content;
      await reloadTree();
      message = 'Created Decodal profile source.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'profile source create failed';
    } finally {
      creating = false;
    }
  }

  async function saveFile() {
    if (!selectedFile) return;
    saving = true;
    message = null;
    try {
      selectedFile = await writeProfileTreeFile(workspaceId, sourceTreeId, {
        path: selectedFile.file.path,
        revision: selectedFile.file.revision,
        content: draftContent,
      });
      draftContent = selectedFile.content;
      await reloadTree();
      message = 'Saved Decodal profile source.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'profile source save failed';
    } finally {
      saving = false;
    }
  }

  async function deleteFile() {
    if (!selectedFile) return;
    deleting = true;
    message = null;
    try {
      tree = await deleteProfileTreeFile(workspaceId, sourceTreeId, {
        path: selectedFile.file.path,
        revision: selectedFile.file.revision,
      });
      selectedFile = null;
      draftContent = '';
      diagnostics = tree.diagnostics;
      message = 'Deleted Decodal profile source.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'profile source delete failed';
    } finally {
      deleting = false;
    }
  }

  $effect(() => {
    if (!workspaceId || !sourceTreeId) {
      loading = false;
      return;
    }
    loading = true;
    reloadTree()
      .catch((err) => {
        message = err instanceof Error ? err.message : 'profile source tree load failed';
      })
      .finally(() => {
        loading = false;
      });
  });
</script>

<svelte:head>
  <title>Profile source tree · Yoi Workspace</title>
</svelte:head>

<section class="card settings-section" aria-labelledby="profile-tree-title">
  <header class="settings-section-header">
    <div>
      <p class="eyebrow">Profile source tree</p>
      <h2 id="profile-tree-title">{sourceTreeId}</h2>
    </div>
    <a href={profileSettingsHref(workspaceId)}>Back to profiles</a>
  </header>

  {#if loading}
    <p class="status-message">Loading source tree…</p>
  {:else if tree}
    <p class="settings-note">
      {tree.tree.root_path} · {tree.tree.file_count} files · {tree.tree.content_type} · {tree.tree.content_digest}
    </p>
    <div class="settings-inline-form">
      <input bind:value={newFilePath} aria-label="New profile source virtual path" placeholder="profiles/new-profile.dcdl" />
      <button type="button" disabled={creating} onclick={createFile}>{creating ? 'Creating…' : 'Create source'}</button>
    </div>
    <div class="settings-profile-grid">
      <article>
        <h3>Files</h3>
        <ul class="settings-profile-list">
          {#each tree.files as file (file.path)}
            <li>
              <button class="link-button" type="button" onclick={() => selectFile(file.path)}><strong>{file.path}</strong></button>
              <span>{file.content_type} · {file.content_digest}</span>
              <small>{file.size_bytes} bytes · rev {file.revision}</small>
              <DiagnosticsList diagnostics={file.diagnostics} />
            </li>
          {/each}
        </ul>
      </article>
      {#if selectedFile}
        <article>
          <header class="settings-section-header">
            <div>
              <p class="eyebrow">{selectedFile.file.content_type}</p>
              <h3>{selectedFile.file.path}</h3>
            </div>
            <div class="settings-editor-actions">
              <button type="button" disabled={saving || draftContent === selectedFile.content} onclick={saveFile}>{saving ? 'Saving…' : 'Save'}</button>
              <button type="button" disabled={deleting} onclick={deleteFile}>{deleting ? 'Deleting…' : 'Delete'}</button>
            </div>
          </header>
          <DecodalSourceEditor value={draftContent} onChange={(value) => (draftContent = value)} ariaLabel={`Decodal source ${selectedFile.file.path}`} />
        </article>
      {/if}
    </div>
  {/if}

  {#if message}<p class="status-message">{message}</p>{/if}
  <DiagnosticsList {diagnostics} />
</section>
