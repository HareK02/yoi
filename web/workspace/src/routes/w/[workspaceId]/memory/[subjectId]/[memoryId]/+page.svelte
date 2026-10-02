<script lang="ts">
  import DocumentMarkdown from '$lib/workspace/markdown/DocumentMarkdown.svelte';
  import { formatDate, workspaceRoute } from '$lib/workspace/api/http';
  import type {
    MemoryEvidenceOrigin,
    SubjektivMemoryRevisionRef,
    SubjektivMemorySourceEvidenceRef,
  } from '$lib/generated/memory-api';
  import type { PageProps } from './$types';

  let { data }: PageProps = $props();

  const memory = $derived(data.memory.data);
  const revisions = $derived(data.revisions.data?.items ?? []);

  function subjectHref(): string {
    return workspaceRoute(data.workspaceId, `/memory/${encodeURIComponent(data.subjectId)}`);
  }

  function memoryPath(memoryId = data.memoryId): string {
    return workspaceRoute(
      data.workspaceId,
      `/memory/${encodeURIComponent(data.subjectId)}/${encodeURIComponent(memoryId)}`,
    );
  }

  function revisionHref(revision: number): string {
    return `${memoryPath()}?revision=${revision}`;
  }

  function derivedHref(reference: SubjektivMemoryRevisionRef): string {
    return `${memoryPath(reference.memory_id)}?revision=${reference.revision}`;
  }

  function bodyContinuationHref(): string | null {
    if (!memory?.body_truncated || memory.body_next_offset == null || memory.body_next_byte_offset == null) return null;
    const query = new URLSearchParams({
      revision: String(memory.revision),
      offset: String(memory.body_next_offset),
      byte_offset: String(memory.body_next_byte_offset),
    });
    return `${memoryPath()}?${query}`;
  }

  function evidenceContinuationHref(): string | null {
    if (!memory?.evidence_has_more || !memory.evidence_next_cursor) return null;
    const query = new URLSearchParams({
      revision: String(memory.revision),
      evidence_cursor: memory.evidence_next_cursor,
    });
    return `${memoryPath()}?${query}`;
  }

  function label(value: string): string {
    return value.replaceAll('_', ' ');
  }

  function rangeLabel(range: [number, number] | null | undefined): string {
    return range ? `${range[0]}–${range[1]}` : 'None';
  }

  function originLabel(origin: MemoryEvidenceOrigin | null | undefined): string {
    if (!origin) return 'Origin unavailable';
    const identity = [
      origin.account_id && `account ${origin.account_id}`,
      origin.workspace_id && `workspace ${origin.workspace_id}`,
      origin.runtime_id && `runtime ${origin.runtime_id}`,
      origin.worker_id && `worker ${origin.worker_id}`,
      origin.flow_selector && `flow ${origin.flow_selector}`,
      origin.flow_definition_id && `definition ${origin.flow_definition_id}`,
      origin.flow_definition_revision != null && `definition revision ${origin.flow_definition_revision}`,
    ].filter(Boolean);
    return `${label(origin.kind)}${identity.length ? ` · ${identity.join(' · ')}` : ''}`;
  }

  function sourceRefTitle(reference: SubjektivMemorySourceEvidenceRef): string {
    return reference.label || reference.summary || reference.evidence_id || reference.segment_id || 'Source reference';
  }
</script>

<svelte:head>
  <title>{memory?.claim ?? data.memoryId} · Memory · Yoi Workspace</title>
  <meta name="description" content="Committed Memory detail, immutable revisions, and provenance" />
</svelte:head>

<section class="memory-page memory-detail-page" aria-labelledby="memory-detail-heading" data-memory-view="detail" data-memory-state={data.memory.error ? 'error' : memory ? memory.state : 'unavailable'}>
  <a class="memory-back-link" href={subjectHref()}>← {data.subject.data?.role ?? 'Subject'}</a>

  {#if memory}
    <header class="memory-page-header">
      <div>
        <div class="pill-row">
          <span class="memory-pill is-{memory.state}">{memory.state}</span>
          <span class="memory-kind">{label(memory.kind)}</span>
          {#if memory.revision !== memory.current_revision}<span class="historical-pill">Historical revision</span>{/if}
        </div>
        <h1 id="memory-detail-heading">{memory.claim}</h1>
        <code class="memory-id" title={memory.memory_id}>{memory.memory_id}</code>
      </div>
      <span class="read-only-label">Read-only</span>
    </header>

    <dl class="memory-facts" aria-label="Memory details">
      <div><dt>Revision</dt><dd>{memory.revision} of {memory.current_revision}</dd></div>
      <div><dt>State</dt><dd>{memory.state}</dd></div>
      <div><dt>Kind</dt><dd>{label(memory.kind)}</dd></div>
      <div><dt>Created</dt><dd><time datetime={memory.created_at}>{formatDate(memory.created_at)}</time></dd></div>
      <div><dt>Updated</dt><dd><time datetime={memory.updated_at}>{formatDate(memory.updated_at)}</time></dd></div>
    </dl>

    <section class="detail-section memory-body-section" aria-labelledby="memory-body-heading">
      <header class="section-heading"><h2 id="memory-body-heading">Memory</h2></header>
      <p class="why-useful"><strong>Why useful:</strong> {memory.why_useful}</p>
      {#if memory.staleness}<p class="staleness"><strong>Staleness:</strong> {memory.staleness}</p>{/if}
      <p class="change-reason"><strong>Change reason:</strong> {memory.change_reason}</p>

      {#if memory.body_md.trim().length === 0}
        <div class="memory-state" role="status"><strong>This Memory body is empty.</strong></div>
      {:else}
        <article class="memory-document" aria-label="Committed Memory body">
          <DocumentMarkdown text={memory.body_md} />
        </article>
      {/if}
      {#if bodyContinuationHref()}
        <div class="continuation-note" role="status">
          <p>This bounded body segment continues at line {memory.body_next_offset}, byte {memory.body_next_byte_offset}.</p>
          <a href={bodyContinuationHref() ?? undefined}>View next body segment</a>
        </div>
      {/if}
    </section>

    <section class="detail-section provenance-section" aria-labelledby="provenance-heading">
      <header class="section-heading">
        <div><p class="memory-eyebrow">Audit context</p><h2 id="provenance-heading">Candidate provenance</h2></div>
        <span>{memory.source_candidates.length} candidate{memory.source_candidates.length === 1 ? '' : 's'}</span>
      </header>

      {#if memory.source_candidates.length === 0}
        <div class="memory-state" role="status"><p>No candidate evidence is included on this page.</p></div>
      {:else}
        <div class="candidate-list">
          {#each memory.source_candidates as candidate, candidateIndex (candidate.candidate_id)}
            <article class="candidate-record">
              <header>
                <h3>Candidate</h3>
                <code title={candidate.candidate_id}>{candidate.candidate_id}</code>
              </header>

              <section aria-labelledby={`evidence-${candidateIndex}`}>
                <h4 id={`evidence-${candidateIndex}`}>Evidence <span>{candidate.evidence.length} of {candidate.evidence_total}</span></h4>
                {#if candidate.evidence.length === 0}
                  <p class="empty-copy">No evidence anchors on this page.</p>
                {:else}
                  <ul class="evidence-list">
                    {#each candidate.evidence as evidence (evidence.id)}
                      <li>
                        <div class="evidence-heading"><strong>{evidence.id}</strong><span>{label(evidence.kind)}</span></div>
                        <dl>
                          <div><dt>Entry range</dt><dd>{rangeLabel(evidence.entry_range)}</dd></div>
                          <div><dt>Origin</dt><dd>{originLabel(evidence.origin)}</dd></div>
                        </dl>
                        {#if evidence.summary}<p>{evidence.summary}</p>{/if}
                        {#if evidence.excerpt}<blockquote>{evidence.excerpt}</blockquote>{/if}
                      </li>
                    {/each}
                  </ul>
                {/if}
                {#if candidate.evidence_truncated}<p class="bounded-note">Additional evidence anchors are available.</p>{/if}
              </section>

              <section aria-labelledby={`sources-${candidateIndex}`}>
                <h4 id={`sources-${candidateIndex}`}>Source refs <span>{candidate.source_refs.length} of {candidate.source_refs_total}</span></h4>
                {#if candidate.source_refs.length === 0}
                  <p class="empty-copy">No source refs on this page.</p>
                {:else}
                  <ul class="source-list">
                    {#each candidate.source_refs as reference, index (`${candidate.candidate_id}:source:${index}`)}
                      <li>
                        <strong>{sourceRefTitle(reference)}</strong>
                        <dl>
                          <div><dt>Session</dt><dd><code>{reference.session_id ?? 'None'}</code></dd></div>
                          <div><dt>Segment</dt><dd><code>{reference.segment_id ?? 'None'}</code></dd></div>
                          <div><dt>Entry range</dt><dd>{rangeLabel(reference.entry_range)}</dd></div>
                          <div><dt>Evidence id</dt><dd><code>{reference.evidence_id ?? 'None'}</code></dd></div>
                          <div><dt>Evidence kind</dt><dd>{reference.evidence_kind ? label(reference.evidence_kind) : 'None'}</dd></div>
                          <div><dt>Origin</dt><dd>{originLabel(reference.origin)}</dd></div>
                        </dl>
                        {#if reference.summary}<p>{reference.summary}</p>{/if}
                      </li>
                    {/each}
                  </ul>
                {/if}
                {#if candidate.source_refs_truncated}<p class="bounded-note">Additional source refs are available.</p>{/if}
              </section>
            </article>
          {/each}
        </div>
      {/if}
      {#if evidenceContinuationHref()}
        <div class="continuation-note" role="status">
          <p>Candidate evidence continues on another bounded page.</p>
          <a href={evidenceContinuationHref() ?? undefined}>View next evidence page</a>
        </div>
      {/if}
    </section>

    <section class="detail-section derived-section" aria-labelledby="derived-heading">
      <header class="section-heading"><h2 id="derived-heading">Derived from</h2><span>{memory.derived_from.length}</span></header>
      {#if memory.derived_from.length === 0}
        <p class="empty-copy">No derivation references.</p>
      {:else}
        <ul class="derived-list">
          {#each memory.derived_from as reference (`${reference.memory_id}:${reference.revision}`)}
            <li><a href={derivedHref(reference)}><code>{reference.memory_id}</code><span>Revision {reference.revision}</span></a></li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="detail-section revision-section" aria-labelledby="revision-heading">
      <header class="section-heading"><div><p class="memory-eyebrow">Immutable record</p><h2 id="revision-heading">Revision history</h2></div>{#if data.revisions.data}<span>{revisions.length}{data.revisions.data.has_more ? '+' : ''}</span>{/if}</header>
      {#if data.revisions.data}
        {#if revisions.length === 0}
          <p class="empty-copy">No revision history is available.</p>
        {:else}
          <ol class="revision-list">
            {#each revisions as revision (revision.revision)}
              <li class:current={revision.revision === memory.revision}>
                <a href={revisionHref(revision.revision)} aria-current={revision.revision === memory.revision ? 'page' : undefined}>
                  <div><strong>Revision {revision.revision}</strong><span class="memory-pill is-{revision.state}">{revision.state}</span><span class="memory-kind">{label(revision.kind)}</span></div>
                  <p>{revision.claim}</p>
                  <small>{revision.change_reason} · <time datetime={revision.updated_at}>{formatDate(revision.updated_at)}</time></small>
                </a>
              </li>
            {/each}
          </ol>
          {#if data.revisions.data.has_more}<p class="bounded-note">More immutable revisions are available after this bounded page.</p>{/if}
        {/if}
      {:else if data.revisions.error}
        <div class="memory-state is-error" role="alert"><strong>Revision history unavailable.</strong><p>{data.revisions.error}</p></div>
      {:else}
        <div class="memory-state" role="status"><p>Revision history data is unavailable.</p></div>
      {/if}
    </section>
  {:else if data.memory.error}
    <header class="memory-page-header">
      <div><p class="memory-eyebrow">Committed Memory</p><h1 id="memory-detail-heading">{data.memoryId}</h1></div>
      <span class="read-only-label">Read-only</span>
    </header>
    <div class="memory-state is-error" role="alert">
      <strong>Memory unavailable.</strong>
      <p>{data.memory.error}</p>
    </div>
  {:else}
    <header class="memory-page-header"><h1 id="memory-detail-heading">{data.memoryId}</h1></header>
    <div class="memory-state" role="status"><p>Memory data is unavailable.</p></div>
  {/if}
</section>

<style>
  .memory-page {
    display: grid;
    flex: 0 0 auto;
    gap: var(--space-5);
    width: 100%;
    min-width: 0;
    max-width: 78rem;
    margin-inline: auto;
  }

  .memory-back-link {
    width: fit-content;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .memory-page-header,
  .section-heading,
  .candidate-record > header {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--space-4);
    min-width: 0;
  }

  .memory-page-header {
    padding-bottom: var(--space-4);
    border-bottom: 1px solid var(--line);
  }

  .memory-page-header > div,
  .section-heading > div {
    min-width: 0;
  }

  .memory-page-header h1,
  .section-heading h2,
  .candidate-record h3,
  .candidate-record h4 {
    margin: 0;
    color: var(--text-strong);
    overflow-wrap: anywhere;
  }

  .memory-page-header h1 {
    margin-top: var(--space-2);
    font-size: var(--font-size-title);
  }

  .section-heading h2 {
    font-size: var(--font-size-body);
  }

  .section-heading > span,
  .candidate-record h4 span,
  .read-only-label {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .pill-row,
  .revision-list a > div,
  .evidence-heading {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-2);
  }

  .memory-pill,
  .memory-kind,
  .historical-pill,
  .memory-eyebrow {
    font-size: var(--font-size-compact);
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .memory-pill.is-active {
    color: var(--success);
  }

  .memory-pill.is-resolved,
  .historical-pill {
    color: var(--warning);
  }

  .memory-pill.is-retracted {
    color: var(--text-faint);
  }

  .memory-kind,
  .memory-eyebrow {
    color: var(--accent);
  }

  .memory-eyebrow {
    margin: 0 0 var(--space-1);
  }

  .memory-id {
    display: block;
    max-width: 100%;
    margin-top: var(--space-2);
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }

  .memory-facts {
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: var(--space-3);
  }

  .memory-facts > div {
    display: block;
  }

  .memory-facts dt {
    white-space: normal;
  }

  .memory-facts dd {
    margin-top: var(--space-1);
    overflow-wrap: anywhere;
  }

  .detail-section {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
    padding-top: var(--space-4);
    border-top: 1px solid var(--line);
  }

  .why-useful,
  .staleness,
  .change-reason,
  .empty-copy,
  .bounded-note {
    margin: 0;
    color: var(--text-muted);
  }

  .why-useful strong,
  .change-reason strong {
    color: var(--text);
  }

  .staleness,
  .staleness strong {
    color: var(--warning);
  }

  .memory-document {
    min-width: 0;
  }

  .continuation-note {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-3);
    border-left: 3px solid var(--warning);
    background: var(--bg-raised);
    color: var(--text-muted);
  }

  .continuation-note p {
    margin: 0;
  }

  .candidate-list {
    display: grid;
    gap: var(--space-4);
  }

  .candidate-record {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
    padding: var(--space-4);
    border: 1px solid var(--line);
    background: var(--bg-raised);
  }

  .candidate-record > header {
    align-items: baseline;
    padding-bottom: var(--space-3);
    border-bottom: 1px solid var(--line);
  }

  .candidate-record > header code {
    min-width: 0;
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }

  .candidate-record section {
    display: grid;
    gap: var(--space-3);
    min-width: 0;
  }

  .evidence-list,
  .source-list,
  .derived-list,
  .revision-list {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .evidence-list li,
  .source-list li {
    min-width: 0;
    padding: var(--space-3);
    border: 1px solid var(--line);
    background: var(--bg-subtle);
  }

  .evidence-heading span {
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .evidence-list dl,
  .source-list dl {
    margin-top: var(--space-2);
  }

  .evidence-list dd,
  .source-list dd,
  .source-list code {
    overflow-wrap: anywhere;
  }

  .evidence-list p,
  .source-list p {
    margin: var(--space-2) 0 0;
    color: var(--text-muted);
  }

  .evidence-list blockquote {
    margin: var(--space-2) 0 0;
    padding-left: var(--space-3);
    border-left: 3px solid var(--line-strong);
    color: var(--text-muted);
  }

  .derived-list {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .derived-list a {
    display: grid;
    gap: var(--space-1);
    min-width: 0;
    padding: var(--space-3);
    border: 1px solid var(--line);
    color: inherit;
    text-decoration: none;
  }

  .derived-list a:hover,
  .derived-list a:focus-visible,
  .revision-list a:hover,
  .revision-list a:focus-visible {
    background: var(--interactive-hover);
  }

  .derived-list code {
    overflow-wrap: anywhere;
  }

  .derived-list span {
    color: var(--text-muted);
    font-size: var(--font-size-compact);
  }

  .revision-list {
    border-top: 1px solid var(--line);
  }

  .revision-list li {
    border-bottom: 1px solid var(--line);
  }

  .revision-list li.current {
    border-left: 3px solid var(--accent);
  }

  .revision-list a {
    display: grid;
    gap: var(--space-1);
    min-width: 0;
    padding: var(--space-3) var(--space-4);
    color: inherit;
    text-decoration: none;
  }

  .revision-list p,
  .revision-list small {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .revision-list small {
    color: var(--text-muted);
  }

  .memory-state {
    padding: var(--space-4) 0;
    color: var(--text-muted);
  }

  .memory-state strong {
    color: var(--text-strong);
  }

  .memory-state p {
    margin: var(--space-1) 0 0;
  }

  .memory-state.is-error,
  .memory-state.is-error strong {
    color: var(--danger);
  }

  @media (max-width: 900px) {
    .memory-facts {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  @media (max-width: 600px) {
    .memory-page-header,
    .section-heading,
    .candidate-record > header,
    .continuation-note {
      display: grid;
      gap: var(--space-2);
    }

    .memory-facts,
    .derived-list {
      grid-template-columns: minmax(0, 1fr);
    }

    .memory-facts > div,
    .evidence-list dl > div,
    .source-list dl > div {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-1);
    }

    .candidate-record {
      padding-inline: var(--space-3);
    }
  }
</style>
