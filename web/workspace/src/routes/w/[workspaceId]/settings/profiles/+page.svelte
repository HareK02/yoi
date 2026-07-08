<script lang="ts">
  import DiagnosticsList from '$lib/workspace-settings/DiagnosticsList.svelte';
  import { fetchProfileSettings } from '$lib/workspace-settings/profile-api';
  import type { Diagnostic } from '$lib/workspace-settings/model';
  import type { ProfileSettingsResponse } from '$lib/workspace-settings/profile-types';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? '');

  let profileSettings = $state<ProfileSettingsResponse | null>(null);
  let loading = $state(true);
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
        const response = await fetchProfileSettings(workspaceId);
        if (!cancelled) {
          profileSettings = response;
          diagnostics = response.diagnostics;
        }
      } catch (err) {
        if (!cancelled) message = err instanceof Error ? err.message : 'profile settings request failed';
      } finally {
        if (!cancelled) loading = false;
      }
    }
    load();
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
      <p class="eyebrow">read-only</p>
      <h2 id="profile-sources-title">Profiles</h2>
    </div>
    <span class="badge warning">editing pending</span>
  </header>
  <p>
    Review the effective launch profiles and their source files. Editing is intentionally deferred until the Decodal profile source model is settled.
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
        <h3>Profile source files</h3>
        <p class="settings-note">Decodal source files that define or contribute to launch profiles.</p>
        <ul class="settings-profile-list">
          {#each profileSettings.sources as source (source.profile_source_id)}
            <li>
              <strong>{source.display_path}</strong>
              <span>{source.kind} · {source.size_bytes} bytes</span>
              <small>{source.editable ? 'editable later' : 'read-only'} · rev {source.revision}</small>
              <DiagnosticsList diagnostics={source.diagnostics} />
            </li>
          {/each}
        </ul>
      </article>
    </div>
  {/if}

  {#if message}
    <p class="status-message" class:error={message.includes('failed')}>{message}</p>
  {/if}
  <DiagnosticsList {diagnostics} />
</section>
