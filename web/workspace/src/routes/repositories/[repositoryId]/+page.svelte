<script lang="ts">
  import { formatDate } from '$lib/workspace-api/http';
  import RepositoryTicketKanban from '$lib/workspace-pages/RepositoryTicketKanban.svelte';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
</script>

<svelte:head>
  <title>{data.repository?.item.display_name ?? data.repositoryId} · Repository</title>
</svelte:head>

<section class="card repository-detail-card">
  <h2>Repository</h2>
  {#if data.repository}
    <div class="repository-detail-heading">
      <div>
        <h3>{data.repository.item.display_name}</h3>
        <p><code>{data.repository.item.id}</code></p>
      </div>
      <span class="status-pill" class:warn={data.repository.item.git?.status !== 'clean'}>{data.repository.item.git?.status ?? 'not observed'}</span>
    </div>
    <dl>
      <div>
        <dt>Kind</dt>
        <dd>{data.repository.item.kind}</dd>
      </div>
      <div>
        <dt>Provider</dt>
        <dd>{data.repository.item.provider}</dd>
      </div>
      <div>
        <dt>Record authority</dt>
        <dd>{data.repository.item.record_authority}</dd>
      </div>
      <div>
        <dt>Default selector</dt>
        <dd>{data.repository.item.default_selector ?? 'none configured'}</dd>
      </div>
      <div>
        <dt>Branch</dt>
        <dd>{data.repository.item.git?.branch ?? 'unknown'}</dd>
      </div>
      <div>
        <dt>HEAD</dt>
        <dd><code>{data.repository.item.git?.head ?? 'unknown'}</code></dd>
      </div>
      <div>
        <dt>Dirty</dt>
        <dd>{data.repository.item.git?.dirty ? 'yes' : 'no'}</dd>
      </div>
    </dl>
    {#if data.repository.item.diagnostics && data.repository.item.diagnostics.length > 0}
      <ul class="diagnostics" aria-label="Repository diagnostics">
        {#each data.repository.item.diagnostics as diagnostic}
          <li><code>{diagnostic.code}</code>: {diagnostic.message}</li>
        {/each}
      </ul>
    {/if}
  {:else if data.repositoryError}
    <p class="error">{data.repositoryError}</p>
  {:else}
    <p>Loading repository…</p>
  {/if}
</section>

<section class="card repository-log-card">
  <h2>Recent commits</h2>
  {#if data.repositoryLog}
    {#if data.repositoryLog.items.length === 0}
      <p>No recent commits are available.</p>
    {:else}
      <div class="commit-list">
        {#each data.repositoryLog.items as commit}
          <article class="commit-card">
            <strong>{commit.summary}</strong>
            <span><code>{commit.short_hash}</code> · {commit.author_name} · {formatDate(commit.author_date)}</span>
          </article>
        {/each}
      </div>
    {/if}
    {#if data.repositoryLog.diagnostics.length > 0}
      <ul class="diagnostics" aria-label="Repository log diagnostics">
        {#each data.repositoryLog.diagnostics as diagnostic}
          <li><code>{diagnostic.code}</code>: {diagnostic.message}</li>
        {/each}
      </ul>
    {/if}
  {:else if data.repositoryLogError}
    <p class="error">{data.repositoryLogError}</p>
  {:else}
    <p>Loading repository commits…</p>
  {/if}
</section>

<section class="card repository-tickets-card">
  <h2>Repository Tickets</h2>
  {#if data.repositoryTickets}
    <RepositoryTicketKanban tickets={data.repositoryTickets} />
  {:else if data.repositoryTicketsError}
    <p class="error">{data.repositoryTicketsError}</p>
  {:else}
    <p>Loading repository tickets…</p>
  {/if}
</section>
