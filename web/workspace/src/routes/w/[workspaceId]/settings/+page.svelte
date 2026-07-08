<script lang="ts">
  import type { PageProps } from "./$types";
  import { workspaceApiPath, workspaceRoute } from "$lib/workspace-api/http";
  import {
    SETTINGS_PATTERNS,
    SETTINGS_PERMISSION_NOTICE,
    SETTINGS_SECTIONS,
    diagnosticLabel,
    settingsSectionHref,
    type Diagnostic,
    type RemoteRuntimeConnectionSummary,
    type RemoteRuntimeTestResponse,
    type RuntimeConnectionMutationResponse,
    type RuntimeConnectionSettingsResponse,
  } from "$lib/workspace-settings/model";
  import type {
    ProfileSettingsResponse,
    WorkspaceMetadataSettingsResponse,
    WorkspaceProfileSourceDetailResponse,
  } from "$lib/workspace-settings/profile-types";
  import {
    createProfileSource,
    deleteProfileSource,
    fetchProfileSettings,
    fetchProfileSource,
    fetchWorkspaceMetadataSettings,
    updateProfileRegistry,
    updateProfileSource,
    updateWorkspaceMetadataSettings,
  } from "$lib/workspace-settings/profile-api";

  type RemoteAddForm = {
    runtime_id: string;
    display_name: string;
    endpoint: string;
  };

  let { data }: PageProps = $props();
  let workspace = $derived(data.workspace);
  let workspaceId = $derived(data.workspace?.workspace_id ?? '');
  let workspaceError = $derived(data.workspaceError);

  function settingsApiPath(path: string): string {
    return workspaceApiPath(workspaceId, path);
  }

  let runtimeSettings = $state<RuntimeConnectionSettingsResponse | null>(null);
  let runtimeLoading = $state(true);
  let runtimeError = $state<string | null>(null);
  let mutationMessage = $state<string | null>(null);
  let mutationDiagnostics = $state<Diagnostic[]>([]);
  let tests = $state<Record<string, RemoteRuntimeTestResponse>>({});
  let deleting = $state<string | null>(null);
  let testing = $state<string | null>(null);
  let submitting = $state(false);
  let showAddRuntimeForm = $state(false);
  let remoteForm = $state<RemoteAddForm>({
    runtime_id: "",
    display_name: "",
    endpoint: "",
  });
  let workspaceMetadata = $state<WorkspaceMetadataSettingsResponse | null>(null);
  let profileSettings = $state<ProfileSettingsResponse | null>(null);
  let selectedProfileSource = $state<WorkspaceProfileSourceDetailResponse | null>(null);
  let profileSourceContent = $state("");
  let newProfileName = $state("");
  let newProfileDescription = $state("");
  let newProfileContent = $state("{\n  slug = \"workspace-profile\";\n  description = \"Workspace profile\";\n  scope = \"workspace_read\";\n}\n");
  let profileLoading = $state(true);
  let profileMessage = $state<string | null>(null);
  let profileDiagnostics = $state<Diagnostic[]>([]);
  let profileSubmitting = $state(false);
  let workspaceNameDraft = $state("");
  let defaultProfileDraft = $state("");

  $effect(() => {
    if (!workspaceId) {
      runtimeLoading = false;
      return;
    }

    let cancelled = false;

    async function loadRuntimeSettings() {
      runtimeLoading = true;
      runtimeError = null;
      try {
        const response = await fetch(settingsApiPath('/settings/runtime-connections'));
        if (!response.ok) {
          throw new Error(`runtime settings request failed (${response.status})`);
        }
        const data = (await response.json()) as RuntimeConnectionSettingsResponse;
        if (!cancelled) {
          runtimeSettings = data;
        }
      } catch (err) {
        if (!cancelled) {
          runtimeError = err instanceof Error ? err.message : "runtime settings request failed";
        }
      } finally {
        if (!cancelled) {
          runtimeLoading = false;
        }
      }
    }

    loadRuntimeSettings();

    return () => {
      cancelled = true;
    };
  });

  $effect(() => {
    if (!workspaceId) {
      profileLoading = false;
      return;
    }
    let cancelled = false;
    async function loadProfileAndWorkspaceSettings() {
      profileLoading = true;
      profileMessage = null;
      profileDiagnostics = [];
      try {
        const [workspace, profiles] = await Promise.all([
          fetchWorkspaceMetadataSettings(workspaceId),
          fetchProfileSettings(workspaceId),
        ]);
        if (!cancelled) {
          workspaceMetadata = workspace;
          workspaceNameDraft = workspace.display_name;
          profileSettings = profiles;
          defaultProfileDraft = profiles.default_profile ?? "";
          profileDiagnostics = [...workspace.diagnostics, ...profiles.diagnostics];
        }
      } catch (err) {
        if (!cancelled) {
          profileMessage = err instanceof Error ? err.message : "profile settings request failed";
        }
      } finally {
        if (!cancelled) {
          profileLoading = false;
        }
      }
    }
    loadProfileAndWorkspaceSettings();
    return () => {
      cancelled = true;
    };
  });

  async function refreshProfileSettings() {
    profileSettings = await fetchProfileSettings(workspaceId);
    profileDiagnostics = profileSettings.diagnostics;
  }

  async function submitWorkspaceName() {
    if (!workspaceMetadata) return;
    profileSubmitting = true;
    profileMessage = null;
    try {
      const response = await updateWorkspaceMetadataSettings(workspaceId, {
        display_name: workspaceNameDraft,
        revision: workspaceMetadata.revision,
      });
      workspaceMetadata = response.workspace;
      workspaceNameDraft = response.workspace.display_name;
      profileDiagnostics = response.diagnostics.concat(response.workspace.diagnostics);
      profileMessage = "Workspace display name updated.";
    } catch (err) {
      profileMessage = err instanceof Error ? err.message : "workspace update failed";
    } finally {
      profileSubmitting = false;
    }
  }

  async function submitProfileRegistry() {
    if (!profileSettings) return;
    profileSubmitting = true;
    profileMessage = null;
    try {
      const response = await updateProfileRegistry(workspaceId, {
        registry_revision: profileSettings.registry_revision,
        default_profile: defaultProfileDraft || null,
        profiles: profileSettings.profiles
          .filter((profile) => profile.source_kind === "project")
          .map((profile) => ({
            name: profile.selector.replace(/^project:/, ""),
            description: profile.description ?? null,
            profile_source_id: profile.profile_source_id ?? null,
          })),
      });
      profileSettings = response.settings;
      defaultProfileDraft = response.settings.default_profile ?? "";
      profileDiagnostics = response.diagnostics.concat(response.settings.diagnostics);
      profileMessage = "Profile registry updated.";
    } catch (err) {
      profileMessage = err instanceof Error ? err.message : "profile registry update failed";
    } finally {
      profileSubmitting = false;
    }
  }

  async function submitNewProfileSource() {
    if (!profileSettings) return;
    profileSubmitting = true;
    profileMessage = null;
    try {
      const response = await createProfileSource(workspaceId, {
        name: newProfileName,
        description: newProfileDescription || undefined,
        content: newProfileContent,
        registry_revision: profileSettings.registry_revision,
      });
      profileSettings = response.settings;
      defaultProfileDraft = response.settings.default_profile ?? "";
      profileDiagnostics = response.diagnostics.concat(response.settings.diagnostics);
      newProfileName = "";
      newProfileDescription = "";
      profileMessage = "Profile source created.";
    } catch (err) {
      profileMessage = err instanceof Error ? err.message : "profile source create failed";
    } finally {
      profileSubmitting = false;
    }
  }

  async function selectProfileSource(sourceId: string) {
    profileSubmitting = true;
    profileMessage = null;
    try {
      selectedProfileSource = await fetchProfileSource(workspaceId, sourceId);
      profileSourceContent = selectedProfileSource.content;
      profileDiagnostics = selectedProfileSource.diagnostics.concat(
        selectedProfileSource.source.diagnostics,
        selectedProfileSource.profile.diagnostics,
      );
    } catch (err) {
      profileMessage = err instanceof Error ? err.message : "profile source load failed";
    } finally {
      profileSubmitting = false;
    }
  }

  async function submitProfileSourceUpdate() {
    if (!selectedProfileSource) return;
    profileSubmitting = true;
    profileMessage = null;
    try {
      const response = await updateProfileSource(workspaceId, selectedProfileSource.source.profile_source_id, {
        content: profileSourceContent,
        revision: selectedProfileSource.source.revision,
      });
      profileSettings = response.settings;
      defaultProfileDraft = response.settings.default_profile ?? "";
      profileDiagnostics = response.diagnostics.concat(response.settings.diagnostics);
      await selectProfileSource(selectedProfileSource.source.profile_source_id);
      profileMessage = "Profile source updated.";
    } catch (err) {
      profileMessage = err instanceof Error ? err.message : "profile source update failed";
    } finally {
      profileSubmitting = false;
    }
  }

  async function submitProfileSourceDelete() {
    if (!profileSettings || !selectedProfileSource) return;
    profileSubmitting = true;
    profileMessage = null;
    try {
      const response = await deleteProfileSource(workspaceId, selectedProfileSource.source.profile_source_id, {
        registry_revision: profileSettings.registry_revision,
        source_revision: selectedProfileSource.source.revision,
      });
      profileSettings = response.settings;
      selectedProfileSource = null;
      profileSourceContent = "";
      profileDiagnostics = response.diagnostics.concat(response.settings.diagnostics);
      profileMessage = "Profile source deleted.";
    } catch (err) {
      profileMessage = err instanceof Error ? err.message : "profile source delete failed";
    } finally {
      profileSubmitting = false;
    }
  }

  async function submitRemoteRuntime() {
    submitting = true;
    mutationMessage = null;
    mutationDiagnostics = [];
    try {
      const response = await fetch(settingsApiPath('/settings/runtime-connections/remotes'), {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          runtime_id: remoteForm.runtime_id,
          display_name: remoteForm.display_name || null,
          endpoint: remoteForm.endpoint,
        }),
      });
      if (!response.ok) {
        throw new Error(await responseErrorMessage(response, "add remote Runtime failed"));
      }
      const data = (await response.json()) as RuntimeConnectionMutationResponse;
      applyRuntimeMutation(data);
      remoteForm = { runtime_id: "", display_name: "", endpoint: "" };
      showAddRuntimeForm = false;
    } catch (err) {
      mutationMessage = err instanceof Error ? err.message : "add remote Runtime failed";
    } finally {
      submitting = false;
    }
  }

  async function deleteRemoteRuntime(runtimeId: string) {
    deleting = runtimeId;
    mutationMessage = null;
    mutationDiagnostics = [];
    try {
      const response = await fetch(settingsApiPath(`/settings/runtime-connections/remotes/${encodeURIComponent(runtimeId)}`), {
        method: "DELETE",
      });
      if (!response.ok) {
        throw new Error(await responseErrorMessage(response, "delete remote Runtime failed"));
      }
      const data = (await response.json()) as RuntimeConnectionMutationResponse;
      applyRuntimeMutation(data);
      const nextTests = { ...tests };
      delete nextTests[runtimeId];
      tests = nextTests;
    } catch (err) {
      mutationMessage = err instanceof Error ? err.message : "delete remote Runtime failed";
    } finally {
      deleting = null;
    }
  }

  async function testRemoteRuntime(runtimeId: string) {
    testing = runtimeId;
    try {
      const response = await fetch(settingsApiPath(`/settings/runtime-connections/remotes/${encodeURIComponent(runtimeId)}/test`), {
        method: "POST",
      });
      if (!response.ok) {
        throw new Error(await responseErrorMessage(response, "test remote Runtime failed"));
      }
      const data = (await response.json()) as RemoteRuntimeTestResponse;
      tests = { ...tests, [runtimeId]: data };
    } catch (err) {
      tests = {
        ...tests,
        [runtimeId]: {
          workspace_id: runtimeSettings?.workspace_id ?? "unknown",
          runtime_id: runtimeId,
          checked_at: new Date().toISOString(),
          state: "failed",
          protocol_version: null,
          compatibility_basis: "browser request failed",
          capabilities: [],
          health_result: "failed",
          diagnostics: [
            {
              code: "browser_runtime_test_failed",
              severity: "error",
              message: err instanceof Error ? err.message : "test remote Runtime failed",
            },
          ],
        },
      };
    } finally {
      testing = null;
    }
  }

  function capabilityOperations(test: RemoteRuntimeTestResponse, state: "available" | "unknown" | "incompatible"): string[] {
    const suffix = `:${state}`;
    return test.capabilities
      .filter((capability) => capability.endsWith(suffix))
      .map((capability) => capability.slice(0, -suffix.length));
  }

  function applyRuntimeMutation(data: RuntimeConnectionMutationResponse) {
    runtimeSettings = runtimeSettings
      ? { ...runtimeSettings, remotes: data.remotes, diagnostics: data.diagnostics }
      : {
          workspace_id: data.workspace_id,
          embedded: {
            runtime_id: "embedded-worker-runtime",
            display_name: "Embedded Runtime",
            kind: "embedded_worker_runtime",
            built_in: true,
            config_managed: false,
            active: false,
            can_spawn_worker: false,
            restart_required: false,
            status: "unknown",
            diagnostics: [],
          },
          remotes: data.remotes,
          diagnostics: data.diagnostics,
        };
    mutationDiagnostics = data.diagnostics;
    mutationMessage = data.restart_required
      ? "Runtime config saved. Restart the Workspace backend to apply live registry changes."
      : "Runtime config saved.";
  }

  async function responseErrorMessage(response: Response, fallback: string): Promise<string> {
    try {
      const payload = (await response.json()) as { error?: { message?: string; code?: string } | string; message?: string };
      if (typeof payload.error === "object" && payload.error?.message) {
        return `${payload.error.code ?? "request_failed"}: ${payload.error.message}`;
      }
      if (payload.message) {
        const code = typeof payload.error === "string" ? payload.error : "request_failed";
        return `${code}: ${payload.message}`;
      }
    } catch {
      // fall through
    }
    return `${fallback} (${response.status})`;
  }
</script>

<svelte:head>
  <title>Settings · Yoi Workspace</title>
</svelte:head>

<div class="settings-shell" aria-labelledby="settings-title">
    <section class="hero settings-hero">
      <div>
        <p class="eyebrow">Workspace Browser</p>
        <h1 id="settings-title">Settings / Admin</h1>
        <p class="hero-copy">
          Local administration surfaces for the Workspace backend. Runtime Connections v0 is editable through typed APIs; broader admin controls remain bounded placeholders.
        </p>
      </div>
      <span class="badge warning">local only</span>
    </section>

    <section class="card settings-notice" aria-labelledby="settings-boundary-title">
      <div>
        <p class="eyebrow">Authority boundary</p>
        <h2 id="settings-boundary-title">No browser admin permission model</h2>
        <p>{SETTINGS_PERMISSION_NOTICE}</p>
      </div>
      <div class="settings-diagnostic" role="note">
        <strong>Restart-required</strong>
        <span>Runtime config changes are persisted, then applied after backend restart.</span>
      </div>
    </section>

    <section class="settings-nav-card" aria-label="Settings sections">
      {#each SETTINGS_SECTIONS as section}
        <a class="settings-nav-link" href={workspaceRoute(workspaceId, settingsSectionHref(section.id))}>
          <span>{section.label}</span>
          <small>{section.status === "editable" ? "Editable" : section.status === "read-only" ? "Read-only" : "Placeholder"}</small>
        </a>
      {/each}
    </section>

    <section class="card settings-section" id="runtime-connections" aria-labelledby="runtime-connections-title">
      <header class="settings-section-header">
        <div>
          <p class="eyebrow">editable</p>
          <h2 id="runtime-connections-title">Runtime Connections</h2>
        </div>
        <span class="badge success">typed API</span>
      </header>
      <p>{SETTINGS_SECTIONS.find((section) => section.id === "runtime-connections")?.summary}</p>

      {#if runtimeLoading}
        <p class="status-message">Loading Runtime connections…</p>
      {:else if runtimeError}
        <p class="status-message error">Runtime connection settings unavailable: {runtimeError}</p>
      {:else if runtimeSettings}
        <div class="settings-action-row">
          <button type="button" onclick={() => showAddRuntimeForm = !showAddRuntimeForm}>
            {showAddRuntimeForm ? "Cancel adding Runtime" : "Add remote Runtime"}
          </button>
        </div>

        {#if showAddRuntimeForm}
          <form class="settings-runtime-form" onsubmit={(event) => { event.preventDefault(); void submitRemoteRuntime(); }}>
            <h3>Add remote Runtime</h3>
            <p>Endpoint is submitted to the Backend but not echoed back in settings responses.</p>
            <label>
              <span>Runtime id</span>
              <input bind:value={remoteForm.runtime_id} required maxlength="96" pattern="[A-Za-z0-9_.-]+" placeholder="team-runtime" />
            </label>
            <label>
              <span>Display name</span>
              <input bind:value={remoteForm.display_name} maxlength="80" placeholder="Team Runtime" />
            </label>
            <label>
              <span>Endpoint</span>
              <input bind:value={remoteForm.endpoint} required inputmode="url" placeholder="https://runtime.example" />
            </label>
            <button type="submit" disabled={submitting}>{submitting ? "Saving…" : "Add Runtime"}</button>
          </form>
        {/if}

        {#if mutationMessage}
          <p class="status-message" class:error={mutationMessage.includes("failed")}>{mutationMessage}</p>
        {/if}
        {@render DiagnosticsList({ diagnostics: mutationDiagnostics })}

        <div class="settings-runtime-list" aria-label="Runtime connections">
          <h3>Runtimes</h3>
          <div class="settings-runtime-table-wrap">
            <table class="settings-runtime-table">
              <thead>
                <tr>
                  <th scope="col">Runtime</th>
                  <th scope="col">Source</th>
                  <th scope="col">Connection</th>
                  <th scope="col">Status</th>
                  <th scope="col">Actions</th>
                </tr>
              </thead>
              <tbody>
                <tr class:inactive={!runtimeSettings.embedded.active}>
                  <td>
                    <strong>{runtimeSettings.embedded.display_name}</strong>
                    <code>{runtimeSettings.embedded.runtime_id}</code>
                  </td>
                  <td>
                    <strong>embedded</strong>
                    <span>Workspace backend process</span>
                  </td>
                  <td>Local Backend runtime</td>
                  <td>
                    <span class="badge" class:success={runtimeSettings.embedded.active} class:warning={!runtimeSettings.embedded.active}>{runtimeSettings.embedded.status}</span>
                    {#if runtimeSettings.embedded.restart_required}
                      <span class="badge warning">restart required</span>
                    {/if}
                  </td>
                  <td><span class="settings-muted-action">Managed by backend</span></td>
                </tr>
                {#if runtimeSettings.embedded.diagnostics.length > 0}
                  <tr class="settings-runtime-detail-row">
                    <td colspan="5">{@render DiagnosticsList({ diagnostics: runtimeSettings.embedded.diagnostics })}</td>
                  </tr>
                {/if}
                {#each runtimeSettings.remotes as remote (remote.runtime_id)}
                  <tr class:inactive={!remote.active}>
                    <td>
                      <strong>{remote.display_name}</strong>
                      <code>{remote.runtime_id}</code>
                    </td>
                    <td>
                      <strong>remote</strong>
                      <span>Configured Runtime endpoint</span>
                    </td>
                    <td>
                      <span>Endpoint: {remote.endpoint_configured ? "configured" : "not configured"}</span>
                      {#if remote.endpoint_configured}<small>hidden</small>{/if}
                      <span>Token: {remote.token_ref_configured ? "configured" : "not configured"}</span>
                    </td>
                    <td>
                      <span class="badge" class:success={remote.active} class:warning={!remote.active}>{remote.status}</span>
                      {#if remote.restart_required}
                        <span class="badge warning">restart required</span>
                      {/if}
                    </td>
                    <td>
                      <div class="settings-action-row">
                        <button type="button" onclick={() => void testRemoteRuntime(remote.runtime_id)} disabled={testing === remote.runtime_id}>
                          {testing === remote.runtime_id ? "Testing…" : "Test"}
                        </button>
                        <button type="button" class="danger" onclick={() => void deleteRemoteRuntime(remote.runtime_id)} disabled={deleting === remote.runtime_id}>
                          {deleting === remote.runtime_id ? "Deleting…" : "Delete"}
                        </button>
                      </div>
                    </td>
                  </tr>
                  {#if remote.diagnostics.length > 0}
                    <tr class="settings-runtime-detail-row">
                      <td colspan="5">{@render DiagnosticsList({ diagnostics: remote.diagnostics })}</td>
                    </tr>
                  {/if}
                  {#if tests[remote.runtime_id]}
                    {@const test = tests[remote.runtime_id]}
                    {@const available = capabilityOperations(test, "available")}
                    {@const unchecked = capabilityOperations(test, "unknown")}
                    <tr class="settings-runtime-detail-row">
                      <td colspan="5">
                        <div class="settings-test-result">
                          <strong>Test: {test.state}</strong>
                          <span>{test.health_result} · {test.checked_at}</span>
                          <p>{test.compatibility_basis}</p>
                          {#if available.length > 0}
                            <p class="settings-test-verified">Verified areas: {available.join(', ')}</p>
                          {/if}
                          {#if unchecked.length > 0}
                            <p class="settings-test-verified">Unchecked warning areas: {unchecked.join(', ')}</p>
                          {/if}
                          {@render DiagnosticsList({ diagnostics: test.diagnostics })}
                        </div>
                      </td>
                    </tr>
                  {/if}
                {/each}
              </tbody>
            </table>
          </div>
          {#if runtimeSettings.remotes.length === 0}
            <p class="status-message">No remote Runtime connections configured.</p>
          {/if}
        </div>
      {/if}
    </section>

    <section class="card profile-settings" id="profile-sources" aria-labelledby="profile-settings-title">
      <header class="settings-section-header">
        <div>
          <p class="eyebrow">editable</p>
          <h2 id="profile-settings-title">Profile Sources</h2>
        </div>
        <span class="badge success">Backend scoped</span>
      </header>
      <p>
        Workspace profile settings are surfaced as source-qualified selectors and Decodal source summaries. The browser never receives raw host paths, runtime endpoints, tokens, resource handles, archive content, or archive digests.
      </p>

      {#if profileLoading}
        <p class="status-message">Loading profile settings…</p>
      {:else}
        <div class="grid settings-grid">
          <form class="settings-form" onsubmit={(event) => { event.preventDefault(); submitWorkspaceName(); }}>
            <h3>Workspace display name</h3>
            <label>
              <span>Display name</span>
              <input bind:value={workspaceNameDraft} autocomplete="off" />
            </label>
            <p class="settings-note">Workspace id: <code>{workspaceMetadata?.workspace_id ?? workspaceId}</code></p>
            <button type="submit" disabled={profileSubmitting || !workspaceMetadata}>Save workspace name</button>
          </form>

          <form class="settings-form" onsubmit={(event) => { event.preventDefault(); submitProfileRegistry(); }}>
            <h3>Default launch profile</h3>
            <label>
              <span>Default selector</span>
              <select bind:value={defaultProfileDraft} disabled={!profileSettings}>
                <option value="">Runtime default</option>
                {#each profileSettings?.profiles ?? [] as profile}
                  <option value={profile.selector}>{profile.label} ({profile.selector})</option>
                {/each}
              </select>
            </label>
            <button type="submit" disabled={profileSubmitting || !profileSettings}>Save profile registry</button>
          </form>
        </div>

        <div class="settings-table-wrapper">
          <h3>Discovered profiles</h3>
          <table class="settings-table">
            <thead>
              <tr><th>Selector</th><th>Label</th><th>Source</th><th>Status</th><th>Action</th></tr>
            </thead>
            <tbody>
              {#each profileSettings?.profiles ?? [] as profile}
                <tr>
                  <td><code>{profile.selector}</code></td>
                  <td>{profile.label}</td>
                  <td>{profile.source_kind}</td>
                  <td>{profile.diagnostics.length === 0 ? "ok" : `${profile.diagnostics.length} diagnostics`}</td>
                  <td>
                    {#if profile.profile_source_id}
                      <button type="button" onclick={() => selectProfileSource(profile.profile_source_id!)} disabled={profileSubmitting}>Open source</button>
                    {:else}
                      <span class="settings-note">builtin</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        <div class="grid settings-grid">
          <form class="settings-form" onsubmit={(event) => { event.preventDefault(); submitNewProfileSource(); }}>
            <h3>Create project profile source</h3>
            <label><span>Name</span><input bind:value={newProfileName} placeholder="team-coder" autocomplete="off" /></label>
            <label><span>Description</span><input bind:value={newProfileDescription} placeholder="Optional summary" autocomplete="off" /></label>
            <label><span>Decodal source</span><textarea bind:value={newProfileContent} rows="8"></textarea></label>
            <button type="submit" disabled={profileSubmitting || !profileSettings}>Create source</button>
          </form>

          {#if selectedProfileSource}
            <form class="settings-form" onsubmit={(event) => { event.preventDefault(); submitProfileSourceUpdate(); }}>
              <h3>Edit {selectedProfileSource.profile.label}</h3>
              <p class="settings-note">Source id: <code>{selectedProfileSource.source.profile_source_id}</code>; display path: {selectedProfileSource.source.display_path}</p>
              <label><span>Decodal source</span><textarea bind:value={profileSourceContent} rows="14"></textarea></label>
              <div class="settings-actions">
                <button type="submit" disabled={profileSubmitting}>Validate & save source</button>
                <button type="button" class="danger" onclick={submitProfileSourceDelete} disabled={profileSubmitting}>Delete source</button>
              </div>
            </form>
          {:else}
            <div class="settings-empty-state">
              <h3>No profile source selected</h3>
              <p>Open a project source from the discovered profile table to edit it.</p>
            </div>
          {/if}
        </div>

        {#if profileMessage}
          <p class="status-message">{profileMessage}</p>
        {/if}
        {@render DiagnosticsList({ diagnostics: profileDiagnostics })}
      {/if}
    </section>

    <div class="grid settings-grid">
      {#each SETTINGS_SECTIONS.filter((section) => section.id !== "runtime-connections" && section.id !== "profile-sources") as section}
        <section class="card settings-section" id={section.id} aria-labelledby={`${section.id}-title`}>
          <header class="settings-section-header">
            <div>
              <p class="eyebrow">{section.status}</p>
              <h2 id={`${section.id}-title`}>{section.label}</h2>
            </div>
            {#if section.status === "placeholder"}
              <span class="badge neutral">not implemented</span>
            {:else}
              <span class="badge success">read-only</span>
            {/if}
          </header>
          <p>{section.summary}</p>
          <ul>
            {#each section.bullets as bullet}
              <li>{bullet}</li>
            {/each}
          </ul>

          {#if section.id === "workspace-identity"}
            <dl class="settings-identity-list">
              <div>
                <dt>Workspace id</dt>
                <dd><code>{workspace?.workspace_id ?? "loading"}</code></dd>
              </div>
              <div>
                <dt>Display name</dt>
                <dd>{workspace?.display_name ?? "loading"}</dd>
              </div>
              <div>
                <dt>Record authority</dt>
                <dd>.yoi tickets/objectives through the Backend projection</dd>
              </div>
            </dl>
          {/if}
        </section>
      {/each}
    </div>

    <section class="card settings-patterns" aria-labelledby="settings-patterns-title">
      <div>
        <p class="eyebrow">Implementation patterns</p>
        <h2 id="settings-patterns-title">How settings should appear</h2>
      </div>
      <div class="grid settings-pattern-grid">
        {#each SETTINGS_PATTERNS as pattern}
          <article class="settings-pattern">
            <h3>{pattern.title}</h3>
            <p>{pattern.body}</p>
          </article>
        {/each}
      </div>
    </section>

    {#if !workspace && !workspaceError}
      <p class="status-message">Loading workspace summary…</p>
    {:else if workspaceError}
      <p class="status-message error">Workspace summary unavailable: {workspaceError}</p>
    {/if}
</div>

{#snippet DiagnosticsList({ diagnostics }: { diagnostics: Diagnostic[] })}
  {#if diagnostics.length > 0}
    <ul class="settings-diagnostics-list">
      {#each diagnostics as diagnostic}
        <li class={diagnostic.severity}>
          <strong>{diagnosticLabel(diagnostic)}</strong>
          <span>{diagnostic.message}</span>
        </li>
      {/each}
    </ul>
  {/if}
{/snippet}
