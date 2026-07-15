<script lang="ts">
  import DiagnosticsList from '$lib/workspace/settings/DiagnosticsList.svelte';
  import DecodalSourceEditor from '$lib/workspace/settings/DecodalSourceEditor.svelte';
  import {
    fetchProfileSettings,
    fetchProfileSourceTree,
    deleteProfileTreeFile,
    fetchProfileTreeFile,
    writeProfileTreeFile,
  } from '$lib/workspace/settings/profile-api';
  import type { Diagnostic } from '$lib/workspace/settings/model';
  import { profileSourceTreeSettingsHref, virtualProfilePathForCreate } from '$lib/workspace/settings/profile-routes';
  import type {
    ProfileSettingsResponse,
    WorkspaceProfileSourceTreeFileResponse,
    WorkspaceProfileSourceTreeResponse,
  } from '$lib/workspace/settings/profile-types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? '');

  let profileSettings = $state<ProfileSettingsResponse | null>(null);
  let sourceTree = $state<WorkspaceProfileSourceTreeResponse | null>(null);
  let selectedFile = $state<WorkspaceProfileSourceTreeFileResponse | null>(null);
  let draftContent = $state('');
  let loading = $state(true);
  let saving = $state(false);
  let creating = $state(false);
  let deleting = $state(false);
  let newFilePath = $state('new-profile.dcdl');
  let message = $state<string | null>(null);
  let diagnostics = $state<Diagnostic[]>([]);

  async function loadSettings() {
    if (!workspaceId) return;
    loading = true;
    message = null;
    try {
      const response = await fetchProfileSettings(workspaceId);
      profileSettings = response;
      diagnostics = response.diagnostics;
      const treeId = response.source_trees[0]?.source_tree_id;
      if (treeId) {
        sourceTree = await fetchProfileSourceTree(workspaceId, treeId);
        const firstPath = sourceTree.files[0]?.path;
        if (firstPath) await selectTreeFile(treeId, firstPath);
      }
    } catch (err) {
      message = err instanceof Error ? err.message : 'profile settings request failed';
    } finally {
      loading = false;
    }
  }

  async function selectTreeFile(sourceTreeId: string, path: string) {
    selectedFile = await fetchProfileTreeFile(workspaceId, sourceTreeId, path);
    draftContent = selectedFile.content;
    diagnostics = selectedFile.diagnostics;
  }

  async function createTreeFile(sourceTreeId: string) {
    creating = true;
    message = null;
    try {
      const path = virtualProfilePathForCreate(newFilePath);
      selectedFile = await writeProfileTreeFile(workspaceId, sourceTreeId, {
        path,
        content: '{\n  slug = "new-profile";\n  description = "New profile";\n  scope = "workspace_read";\n}',
      });
      draftContent = selectedFile.content;
      sourceTree = await fetchProfileSourceTree(workspaceId, sourceTreeId);
      diagnostics = selectedFile.diagnostics;
      message = 'Created Decodal profile source.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'profile source create failed';
    } finally {
      creating = false;
    }
  }

  async function deleteSelectedFile() {
    if (!selectedFile) return;
    deleting = true;
    message = null;
    try {
      sourceTree = await deleteProfileTreeFile(workspaceId, selectedFile.source_tree_id, {
        path: selectedFile.file.path,
        revision: selectedFile.file.revision,
      });
      selectedFile = null;
      draftContent = '';
      diagnostics = sourceTree.diagnostics;
      message = 'Deleted Decodal profile source.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'profile source delete failed';
    } finally {
      deleting = false;
    }
  }

  async function saveSelectedFile() {
    if (!selectedFile) return;
    saving = true;
    message = null;
    try {
      selectedFile = await writeProfileTreeFile(workspaceId, selectedFile.source_tree_id, {
        path: selectedFile.file.path,
        revision: selectedFile.file.revision,
        content: draftContent,
      });
      draftContent = selectedFile.content;
      sourceTree = await fetchProfileSourceTree(workspaceId, selectedFile.source_tree_id);
      diagnostics = selectedFile.diagnostics;
      message = 'Saved Decodal profile source.';
    } catch (err) {
      message = err instanceof Error ? err.message : 'profile source save failed';
    } finally {
      saving = false;
    }
  }

  $effect(() => {
    let cancelled = false;
    async function load() {
      await loadSettings();
      if (cancelled) return;
    }
    if (workspaceId) load();
    else loading = false;
    return () => {
      cancelled = true;
    };
  });
</script>

<svelte:head>
  <title>Profiles · Yoi Workspace</title>
</svelte:head>

<section class="card settings-section" aria-labelledby="profile-sources-title">
  <header class="settings-section-header">
    <div>
      <p class="eyebrow">Backend-owned source tree</p>
      <h2 id="profile-sources-title">Profiles</h2>
    </div>
    <span class="badge">Decodal editor</span>
  </header>
  <p>
    Review effective launch profiles and edit Decodal source files through virtual profile-tree paths. The browser receives only safe relative paths and revision tokens.
  </p>

  {#if loading}
    <p class="status-message">Loading profiles…</p>
  {:else if profileSettings}
    <div class="settings-profile-grid">
      <article>
        <h3>Available profiles</h3>
        <p class="settings-note">Profiles that can be selected when launching a Worker.</p>
        <ul class="settings-profile-list">
          {#each profileSettings.profiles as profile (profile.profile_id)}
            <li>
              <strong>{profile.selector}</strong>
              <span>{profile.label}</span>
              <small>{profile.source_kind}{profile.is_default ? ' · default' : ''}</small>
              {#if profile.description}<p>{profile.description}</p>{/if}
              <DiagnosticsList diagnostics={profile.diagnostics} />
            </li>
          {/each}
        </ul>
      </article>
      <article>
        <h3>Profile source tree</h3>
        <p class="settings-note">Virtual Decodal paths exposed by the Backend-owned source tree.</p>
        {#if sourceTree}
          <small>{sourceTree.tree.root_path} · {sourceTree.tree.file_count} files · {sourceTree.tree.content_type} · rev {sourceTree.tree.revision}</small>
          <p><a href={profileSourceTreeSettingsHref(workspaceId, sourceTree.tree.source_tree_id)}>Open tree route</a></p>
          <div class="settings-inline-form">
            <input bind:value={newFilePath} aria-label="New profile source virtual path" placeholder="profiles/new-profile.dcdl" />
            <button type="button" disabled={creating} onclick={() => createTreeFile(sourceTree!.tree.source_tree_id)}>
              {creating ? 'Creating…' : 'Create source'}
            </button>
          </div>
          <ul class="settings-profile-list">
            {#each sourceTree.files as file (file.path)}
              <li>
                <button class="link-button" type="button" onclick={() => selectTreeFile(sourceTree!.tree.source_tree_id, file.path)}>
                  <strong>{file.path}</strong>
                </button>
                <span>{file.kind} · {file.content_type} · {file.size_bytes} bytes</span>
                <small>{file.editable ? 'editable' : 'read-only'} · rev {file.revision}</small>
                <DiagnosticsList diagnostics={file.diagnostics} />
              </li>
            {/each}
          </ul>
        {:else}
          <p class="settings-note">No project profile source tree is available.</p>
        {/if}
      </article>
    </div>

    {#if selectedFile}
      <article class="settings-editor-panel">
        <header class="settings-section-header">
          <div>
            <p class="eyebrow">{selectedFile.source_tree_id}</p>
            <h3>{selectedFile.file.path}</h3>
          </div>
          <div class="settings-editor-actions">
            <button type="button" disabled={saving || draftContent === selectedFile.content} onclick={saveSelectedFile}>
              {saving ? 'Saving…' : 'Save'}
            </button>
            <button type="button" disabled={deleting} onclick={deleteSelectedFile}>
              {deleting ? 'Deleting…' : 'Delete'}
            </button>
          </div>
        </header>
        <DecodalSourceEditor value={draftContent} onChange={(value) => (draftContent = value)} ariaLabel={`Decodal source ${selectedFile.file.path}`} />
      </article>
    {/if}
  {/if}

  {#if message}
    <p class="status-message" class:error={message.includes('failed') || message.includes('error')}>{message}</p>
  {/if}
  <DiagnosticsList {diagnostics} />
</section>
