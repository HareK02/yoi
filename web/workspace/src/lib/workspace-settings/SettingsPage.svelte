<script lang="ts">
  import WorkspaceSidebar from "$lib/workspace-sidebar/WorkspaceSidebar.svelte";
  import type { WorkspaceResponse } from "$lib/workspace-sidebar/types";
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
    type RuntimeConnectionSummary,
  } from "./model";

  type RemoteAddForm = {
    runtime_id: string;
    display_name: string;
    endpoint: string;
  };

  let workspace = $state<WorkspaceResponse | null>(null);
  let runtimeSettings = $state<RuntimeConnectionSettingsResponse | null>(null);
  let loading = $state(true);
  let runtimeLoading = $state(true);
  let error = $state<string | null>(null);
  let runtimeError = $state<string | null>(null);
  let mutationMessage = $state<string | null>(null);
  let mutationDiagnostics = $state<Diagnostic[]>([]);
  let tests = $state<Record<string, RemoteRuntimeTestResponse>>({});
  let deleting = $state<string | null>(null);
  let testing = $state<string | null>(null);
  let submitting = $state(false);
  let remoteForm = $state<RemoteAddForm>({
    runtime_id: "",
    display_name: "",
    endpoint: "",
  });

  $effect(() => {
    let cancelled = false;

    async function loadWorkspace() {
      loading = true;
      error = null;

      try {
        const response = await fetch("/api/workspace");
        if (!response.ok) {
          throw new Error(`workspace request failed (${response.status})`);
        }
        const data = (await response.json()) as WorkspaceResponse;
        if (!cancelled) {
          workspace = data;
        }
      } catch (err) {
        if (!cancelled) {
          error = err instanceof Error ? err.message : "workspace request failed";
        }
      } finally {
        if (!cancelled) {
          loading = false;
        }
      }
    }

    async function loadRuntimeSettings() {
      runtimeLoading = true;
      runtimeError = null;
      try {
        const response = await fetch("/api/settings/runtime-connections");
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

    loadWorkspace();
    loadRuntimeSettings();

    return () => {
      cancelled = true;
    };
  });

  async function submitRemoteRuntime() {
    submitting = true;
    mutationMessage = null;
    mutationDiagnostics = [];
    try {
      const response = await fetch("/api/settings/runtime-connections/remotes", {
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
      const response = await fetch(`/api/settings/runtime-connections/remotes/${encodeURIComponent(runtimeId)}`, {
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
      const response = await fetch(`/api/settings/runtime-connections/remotes/${encodeURIComponent(runtimeId)}/test`, {
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

<div class="workspace-layout">
  <WorkspaceSidebar workspace={workspace} currentPath="/settings" />

  <main class="shell settings-shell" aria-labelledby="settings-title">
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
        <a class="settings-nav-link" href={settingsSectionHref(section.id)}>
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
        {@render RuntimeConnectionCard({ connection: runtimeSettings.embedded })}

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

        {#if mutationMessage}
          <p class="status-message" class:error={mutationMessage.includes("failed")}>{mutationMessage}</p>
        {/if}
        {@render DiagnosticsList({ diagnostics: mutationDiagnostics })}

        <div class="settings-runtime-list" aria-label="Remote Runtime connections">
          <h3>Remote Runtimes</h3>
          {#if runtimeSettings.remotes.length === 0}
            <p class="status-message">No remote Runtime connections configured.</p>
          {:else}
            {#each runtimeSettings.remotes as remote (remote.runtime_id)}
              <article class="settings-runtime-card">
                {@render RuntimeConnectionCard({ connection: remote })}
                <dl class="settings-identity-list compact">
                  <div>
                    <dt>Endpoint</dt>
                    <dd>{remote.endpoint_configured ? "configured (hidden)" : "not configured"}</dd>
                  </div>
                  <div>
                    <dt>Token ref</dt>
                    <dd>{remote.token_ref_configured ? "configured (hidden)" : "not configured"}</dd>
                  </div>
                </dl>
                <div class="settings-action-row">
                  <button type="button" onclick={() => void testRemoteRuntime(remote.runtime_id)} disabled={testing === remote.runtime_id}>
                    {testing === remote.runtime_id ? "Testing…" : "Test"}
                  </button>
                  <button type="button" class="danger" onclick={() => void deleteRemoteRuntime(remote.runtime_id)} disabled={deleting === remote.runtime_id}>
                    {deleting === remote.runtime_id ? "Deleting…" : "Delete"}
                  </button>
                </div>
                {#if tests[remote.runtime_id]}
                  {@const test = tests[remote.runtime_id]}
                  <div class="settings-test-result">
                    <strong>Test: {test.state}</strong>
                    <span>{test.health_result} · {test.checked_at}</span>
                    <p>{test.compatibility_basis}</p>
                    {@render DiagnosticsList({ diagnostics: test.diagnostics })}
                  </div>
                {/if}
              </article>
            {/each}
          {/if}
        </div>
      {/if}
    </section>

    <div class="grid settings-grid">
      {#each SETTINGS_SECTIONS.filter((section) => section.id !== "runtime-connections") as section}
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

    {#if loading}
      <p class="status-message">Loading workspace summary…</p>
    {:else if error}
      <p class="status-message error">Workspace summary unavailable: {error}</p>
    {/if}
  </main>
</div>

{#snippet RuntimeConnectionCard({ connection }: { connection: RuntimeConnectionSummary | RemoteRuntimeConnectionSummary })}
  <article class="settings-runtime-card embedded" class:inactive={!connection.active}>
    <header>
      <div>
        <h3>{connection.display_name}</h3>
        <p><code>{connection.runtime_id}</code></p>
      </div>
      <span class="badge" class:success={connection.active} class:warning={!connection.active}>{connection.status}</span>
    </header>
    <dl class="settings-identity-list compact">
      <div>
        <dt>Kind</dt>
        <dd>{connection.kind}</dd>
      </div>
      <div>
        <dt>Built in</dt>
        <dd>{connection.built_in ? "yes" : "no"}</dd>
      </div>
      <div>
        <dt>Config managed</dt>
        <dd>{connection.config_managed ? "yes" : "no"}</dd>
      </div>
      <div>
        <dt>Spawn</dt>
        <dd>{connection.can_spawn_worker ? "available" : "unavailable"}</dd>
      </div>
      <div>
        <dt>Restart required</dt>
        <dd>{connection.restart_required ? "yes" : "no"}</dd>
      </div>
    </dl>
    {@render DiagnosticsList({ diagnostics: connection.diagnostics })}
  </article>
{/snippet}

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
