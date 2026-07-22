<script lang="ts">
  import { formatDate, workspaceRoute } from '$lib/workspace/api/http';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();
</script>

<svelte:head>
  <title>{data.ticket.data?.title ?? data.ticketId} · Tickets · Yoi Workspace</title>
  <meta name="description" content="Workspace Ticket detail" />
</svelte:head>

<section class="card">
  <p class="breadcrumb"><a href={workspaceRoute(data.workspaceId, '/tickets')}>Tickets</a> / {data.ticketId}</p>

  {#if data.ticket.data}
    <div class="detail-heading">
      <div>
        <p class="eyebrow">{data.ticket.data.id}</p>
        <h2>{data.ticket.data.title}</h2>
      </div>
      <span class="state-pill">{data.ticket.data.state}</span>
    </div>

    <dl class="ticket-detail-grid">
      <div>
        <dt>Priority</dt>
        <dd>{data.ticket.data.priority ?? 'unspecified'}</dd>
      </div>
      <div>
        <dt>Updated</dt>
        <dd>{data.ticket.data.updated_at ? formatDate(data.ticket.data.updated_at) : 'unknown'}</dd>
      </div>
      <div>
        <dt>Created</dt>
        <dd>{data.ticket.data.created_at ? formatDate(data.ticket.data.created_at) : 'unknown'}</dd>
      </div>
      <div>
        <dt>Events</dt>
        <dd>{data.ticket.data.event_count}</dd>
      </div>
      <div>
        <dt>Artifacts</dt>
        <dd>{data.ticket.data.artifact_count}</dd>
      </div>
      <div>
        <dt>Source</dt>
        <dd>{data.ticket.data.record_source}</dd>
      </div>
      {#if data.ticket.data.queued_at || data.ticket.data.queued_by}
        <div>
          <dt>Queued</dt>
          <dd>
            {data.ticket.data.queued_at ? formatDate(data.ticket.data.queued_at) : 'queued'}{data.ticket.data.queued_by ? ` by ${data.ticket.data.queued_by}` : ''}
          </dd>
        </div>
      {/if}
    </dl>

    {#if data.ticket.data.risk_flags.length > 0}
      <div class="risk-flags" aria-label="Risk flags">
        {#each data.ticket.data.risk_flags as flag}
          <span>{flag}</span>
        {/each}
      </div>
    {/if}

    <section class="ticket-body" aria-labelledby="ticket-body-heading">
      <div class="detail-heading compact">
        <h3 id="ticket-body-heading">Body</h3>
        {#if data.ticket.data.body_truncated}
          <span class="warning-pill">truncated</span>
        {/if}
      </div>
      <pre>{data.ticket.data.body || 'No body text is available.'}</pre>
    </section>
  {:else if data.ticket.error}
    <p class="error">{data.ticket.error}</p>
  {:else}
    <p>Waiting for <code>/api/w/{data.workspaceId}/tickets/{data.ticketId}</code>…</p>
  {/if}
</section>
