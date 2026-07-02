<script lang="ts">
  import WorkspaceSidebar from "$lib/workspace-sidebar/WorkspaceSidebar.svelte";
  import type { WorkspaceResponse } from "$lib/workspace-sidebar/types";
  import {
    SETTINGS_PATTERNS,
    SETTINGS_PERMISSION_NOTICE,
    SETTINGS_SECTIONS,
    settingsSectionHref,
  } from "./model";

  let workspace = $state<WorkspaceResponse | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

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

    loadWorkspace();

    return () => {
      cancelled = true;
    };
  });
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
          Read-only shell for future local administration surfaces. This page creates
          navigation and operator context without adding mutation authority.
        </p>
      </div>
      <span class="badge warning">shell only</span>
    </section>

    <section class="card settings-notice" aria-labelledby="settings-boundary-title">
      <div>
        <p class="eyebrow">Authority boundary</p>
        <h2 id="settings-boundary-title">No browser admin permission model</h2>
        <p>{SETTINGS_PERMISSION_NOTICE}</p>
      </div>
      <div class="settings-diagnostic" role="note">
        <strong>Diagnostic pattern</strong>
        <span>Future controls must use typed Backend diagnostics and restart-required states.</span>
      </div>
    </section>

    <section class="settings-nav-card" aria-label="Settings sections">
      {#each SETTINGS_SECTIONS as section}
        <a class="settings-nav-link" href={settingsSectionHref(section.id)}>
          <span>{section.label}</span>
          <small>{section.status === "read-only" ? "Read-only" : "Placeholder"}</small>
        </a>
      {/each}
    </section>

    <div class="grid settings-grid">
      {#each SETTINGS_SECTIONS as section}
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
        <h2 id="settings-patterns-title">How future settings should appear</h2>
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
