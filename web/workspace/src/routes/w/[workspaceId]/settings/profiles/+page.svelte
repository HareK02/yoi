<script lang="ts">
  import DiagnosticsList from "$lib/workspace/settings/DiagnosticsList.svelte";
  import { fetchProfileSettings } from "$lib/workspace/settings/profile-api";
  import type { ProfileSettingsResponse } from "$lib/workspace/settings/profile-types";
  import type { PageProps } from "./$types";

  let { data }: PageProps = $props();
  let workspaceId = $derived(data.workspace?.workspace_id ?? "");
  let profileSettings = $state<ProfileSettingsResponse | null>(null);
  let loading = $state(true);
  let message = $state<string | null>(null);

  $effect(() => {
    let cancelled = false;
    async function load() {
      loading = true;
      message = null;
      try {
        const response = await fetchProfileSettings(workspaceId);
        if (!cancelled) profileSettings = response;
      } catch (error) {
        if (!cancelled) {
          message = error instanceof Error ? error.message : "profile settings request failed";
        }
      } finally {
        if (!cancelled) loading = false;
      }
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

<section class="card settings-section" aria-labelledby="profiles-title">
  <header class="settings-section-header">
    <div>
      <p class="eyebrow">Workspace configuration projection</p>
      <h2 id="profiles-title">Profiles</h2>
    </div>
    <a class="badge" href={`/w/${encodeURIComponent(workspaceId)}/settings/configuration-sources`}>
      Edit configuration
    </a>
  </header>
  <p>
    These launch profiles are derived from the active Workspace configuration revision. Edit Profile declarations and Decodal sources in the shared configuration editor.
  </p>

  {#if loading}
    <p class="status-message">Loading profiles…</p>
  {:else if message}
    <p class="status-message error">{message}</p>
  {:else if profileSettings}
    <p class="settings-note">
      Revision {profileSettings.config_revision ?? "unknown"} · tree {profileSettings.tree_digest ?? "unknown"} · projection {profileSettings.projection_digest ?? "unknown"}
    </p>
    <ul class="settings-profile-list">
      {#each profileSettings.profiles as profile (profile.profile_id)}
        <li>
          <strong>{profile.selector}</strong>
          <span>{profile.label}</span>
          <small>{profile.source_kind}{profile.is_default ? " · default" : ""}</small>
          {#if profile.description}<p>{profile.description}</p>{/if}
          <DiagnosticsList diagnostics={profile.diagnostics} />
        </li>
      {/each}
    </ul>
    <DiagnosticsList diagnostics={profileSettings.diagnostics} />
  {/if}
</section>
