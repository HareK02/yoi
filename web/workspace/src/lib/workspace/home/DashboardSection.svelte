<script lang="ts">
  import BevelLine from '$lib/workspace/ui/BevelLine.svelte';
  import type { DashboardFeed } from './dashboard';

  let { id, title, empty, result }: { id: string; title: string; empty: string; result: Promise<DashboardFeed> } = $props();
  function timestamp(value?: string | null): string | null {
    return value && Number.isFinite(Date.parse(value)) ? new Date(value).toISOString() : null;
  }
  function displayDate(value: string): string {
    return new Date(value).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  }
</script>

<section aria-labelledby={id} class="dashboard-section">
  <h2 {id}>{title}</h2>
  {#await result}
    <p class="feed-state" role="status">Loading {title.toLowerCase()}…</p>
  {:then feed}
    <div data-feed-ready={id}>
      {#each feed.errors as error}<p class="feed-error" role="alert">{error}</p>{/each}
      {#if feed.rows.length}
        <ul>
          {#each feed.rows as row, index (row.key)}
            {@const date = timestamp(row.updatedAt)}
            <li>
              <a href={row.href} class:attention={row.attention}>
                <div class="row-meta"><span>{row.kind}</span><code>{row.reference}</code></div>
                <strong class:machine-title={row.kind === 'Merge Request'}>{row.title}</strong>
                <span class="row-status">{row.status}</span>
                {#if row.workerKey}<span class="row-detail">Coder <code>{row.workerKey}</code></span>{/if}
                {#if row.detail}<span class="row-detail">{row.detail}</span>{/if}
                {#if date}<time datetime={date}>Updated {displayDate(date)}</time>{:else}<span class="row-detail">Update time unavailable</span>{/if}
              </a>
              {#if index < feed.rows.length - 1}<BevelLine decorative />{/if}
            </li>
          {/each}
        </ul>
      {:else if !feed.errors.length}
        <p class="feed-state">{empty}</p>
      {/if}
      {#if feed.notice}<p class="feed-notice">{feed.notice}</p>{/if}
    </div>
  {:catch}
    <p class="feed-error" role="alert">Could not load {title.toLowerCase()}. Refresh to try again.</p>
  {/await}
</section>

<style>
  section { min-width: 0; }
  h2 { margin: 0 0 var(--space-2); font-size: var(--font-size-body); line-height: var(--line-height-body); color: var(--text-strong); }
  ul { list-style: none; margin: 0; padding: 0; }
  a { display: grid; gap: var(--space-1); min-width: 0; padding-block: var(--space-3); text-decoration: none; color: var(--text); overflow-wrap: anywhere; }
  a:hover, a:focus-visible { background: var(--interactive-hover); }
  a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  a:active { background: var(--interactive-selected); }
  strong { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2; overflow: hidden; color: var(--text-strong); font-size: var(--font-size-body); line-height: var(--line-height-body); font-weight: 600; }
  .machine-title { font-family: var(--font-mono); }
  .row-meta { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .row-meta, .row-detail, time, .row-status, .feed-notice { font-size: var(--font-size-compact); line-height: var(--line-height-compact); }
  .row-meta, .row-detail, time, .feed-state, .feed-notice { color: var(--text-muted); }
  code { font-family: var(--font-mono); }
  .row-status { font-weight: 600; }
  .attention .row-status { color: var(--warning); }
  .feed-state { margin: var(--space-3) 0; }
  .feed-error { margin: var(--space-3) 0; color: var(--danger); overflow-wrap: anywhere; }
  .feed-notice { margin: var(--space-3) 0 0; }
</style>
