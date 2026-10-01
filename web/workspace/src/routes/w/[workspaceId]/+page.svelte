<script lang="ts">
  import { invalidate } from '$app/navigation';
  import DashboardSection from '#lib/workspace/home/DashboardSection.svelte';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
  let refreshing = $state(false);
  let refreshError = $state<string | null>(null);
  async function refresh() {
    if (refreshing) return;
    refreshing = true;
    refreshError = null;
    try {
      await invalidate('workspace:home');
      await Promise.all(Object.values(data.dashboard));
    } catch {
      refreshError = 'Could not refresh. Try again.';
    } finally {
      refreshing = false;
    }
  }
</script>

<svelte:head><title>{data.workspace?.display_name ?? 'Workspace'} · Yoi</title></svelte:head>

<div class="workspace-home">
  <header class="home-header">
    <h1>Activity</h1>
    <button type="button" onclick={refresh} disabled={refreshing}>{refreshing ? 'Refreshing…' : 'Refresh'}</button>
  </header>
  {#if refreshError}<p class="home-error" role="alert">{refreshError}</p>{/if}
  <div class="home-columns">
    <DashboardSection id="home-attention" title="Needs attention" empty="No open Merge Requests." result={data.dashboard.reviews} />
    <DashboardSection id="home-active" title="In progress" empty="No in-progress or queued tickets." result={data.dashboard.active} />
    <DashboardSection id="home-recent" title="Recent updates" empty="No done / closed tickets or Objectives yet." result={data.dashboard.recent} />
  </div>
</div>

<style>
  .workspace-home { container-type: inline-size; min-width: 0; }
  .home-header { display: flex; align-items: center; justify-content: space-between; gap: var(--space-4); margin-bottom: var(--space-5); }
  h1 { margin: 0; font-size: var(--font-size-title); line-height: var(--line-height-title); color: var(--text-strong); }
  button { flex: 0 0 auto; padding: var(--space-2) var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); color: var(--text); background: transparent; cursor: pointer; }
  button:hover:not(:disabled), button:focus-visible { background: var(--interactive-hover); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button:active:not(:disabled) { background: var(--interactive-selected); }
  button:disabled { opacity: 0.6; cursor: wait; }
  .home-columns { display: grid; gap: var(--space-6); align-items: start; }
  .home-error { color: var(--danger); overflow-wrap: anywhere; }
  @container (min-width: 52rem) {
    .home-columns { grid-template-columns: repeat(3, minmax(0, 1fr)); }
  }
</style>
