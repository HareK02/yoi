<script lang="ts">
  import { onDestroy } from 'svelte';
  import { beforeNavigate } from '$app/navigation';
  import { createDriveClient, driveDownloadUrl, DRIVE_FILE_MAX_BYTES, DRIVE_RESPONSE_MAX_BYTES, type DriveEntry, parseDriveDecimal } from '#lib/workspace/drive/api.ts';
  import { DriveController } from '#lib/workspace/drive/controller.svelte.ts';
  import { driveHref } from '#lib/workspace/resource-links.ts';
  import { workspaceRoute, formatDate } from '#lib/workspace/api/http.ts';
  import DriveDocument from './DriveDocument.svelte';
  import { previewKind, formatBytes, DRIVE_TEXT_MAX_BYTES } from './preview-policy.ts';

  let { workspaceId, nodeId }: { workspaceId: string; nodeId?: string } = $props();
  const client = createDriveClient();
  const controller = new DriveController(client);
  let form = $state<'folder' | 'text' | 'upload' | 'relocate' | 'replace' | null>(null);
  let formEntry = $state<DriveEntry | null>(null);
  let name = $state('');
  let parentId = $state('');
  let newText = $state('');
  let file = $state<File | null>(null);
  let editing = $state(false);
  let localError = $state<string | null>(null);
  let ancestors = $state<DriveEntry[]>([]);
  let breadcrumbError = $state<string | null>(null);
  let imageUrl = $state<string | null>(null);
  let imageFailed = $state(false);
  let searchText = $state('');
  let includeText = $state(false);
  let copyState = $state('');
  let lifetime = 0;
  let breadcrumbEpoch = 0;
  let entry = $derived($controller.entry);
  let draft = $derived($controller.draft);
  let dirty = $derived(Boolean(draft && draft.text !== draft.baseText));
  let pending = $derived($controller.receipts.some((r) => r.state === 'pending'));
  let unresolved = $derived($controller.receipts.some((r) => r.state === 'pending' || r.state === 'unknown'));
  let denied = $derived($controller.error?.includes('denied') === true || $controller.receipts.some((r) => r.error?.includes('denied')));
  let kind = $derived(entry ? previewKind(entry) : null);
  let canWrite = $derived(Boolean(entry && !denied && !$controller.loading && !unresolved));
  let canEdit = $derived(canWrite && Boolean(draft) && !$controller.truncated && kind !== 'download');

  $effect(() => {
    const ws = workspaceId;
    const id = nodeId;
    ++lifetime;
    form = null; formEntry = null; editing = false; localError = null; file = null; searchText = ''; copyState = '';
    try {
      void controller.select(ws, id ? { workspace_id: ws, node_id: parseDriveDecimal(id) } : undefined);
    } catch { void controller.select(null); localError = 'Invalid Drive node ID.'; }
  });
  $effect(() => {
    const current = $controller.entry;
    const ws = $controller.workspaceId;
    const parent = current?.parent;
    const epoch = ++breadcrumbEpoch;
    ancestors = []; breadcrumbError = null;
    if (!ws || !parent) return;
    const abort = new AbortController();
    void (async () => {
      const chain: DriveEntry[] = [];
      const seen = new Set<string>();
      let next: typeof parent | null = parent;
      try {
        while (next) {
          if (seen.has(next.node_id) || chain.length >= 64) throw new Error('Drive ancestry is unavailable');
          seen.add(next.node_id);
          const ancestor = await client.metadata(ws, next, abort.signal);
          if (epoch !== breadcrumbEpoch) return;
          chain.unshift(ancestor); next = ancestor.parent;
        }
        if (epoch === breadcrumbEpoch) ancestors = chain;
      } catch {
        if (epoch === breadcrumbEpoch && !abort.signal.aborted) breadcrumbError = 'Folder breadcrumb unavailable. Use Drive root or retry.';
      }
    })();
    return () => { ++breadcrumbEpoch; abort.abort(); };
  });
  $effect(() => {
    const blob = $controller.image;
    imageFailed = false;
    if (!blob) { imageUrl = null; return; }
    const url = URL.createObjectURL(blob);
    imageUrl = url;
    return () => { URL.revokeObjectURL(url); };
  });
  onDestroy(() => { ++lifetime; ++breadcrumbEpoch; controller.dispose(); });
  beforeNavigate(({ to, cancel }) => {
    // Within the optional Drive route the controller keeps per-identity drafts.
    // Leaving it destroys this session store; make that loss an explicit decision.
    const isDrive = to?.route.id?.endsWith('/drive/[[nodeId]]') === true;
    if (!isDrive && (dirty || unresolved) && !confirm('Leave Drive? Unsaved drafts and unresolved request IDs are kept only in this page session. Copy them before leaving.')) cancel();
  });
  function openForm(next: typeof form): void {
    if (!entry || !canWrite) return;
    form = next; formEntry = entry; localError = null; file = null; newText = '';
    name = next === 'relocate' ? entry.name : next === 'text' ? 'document.md' : '';
    parentId = entry.parent?.node_id ?? entry.entry.node_id;
  }
  async function action(run: () => Promise<unknown>): Promise<void> {
    const epoch = lifetime;
    localError = null;
    try { await run(); } catch (error) {
      if (epoch === lifetime) localError = error instanceof Error ? error.message : 'Drive operation failed.';
    }
  }
  async function submitForm(): Promise<void> {
    const current = formEntry;
    const selectedForm = form;
    if (!current || !selectedForm || !canWrite || current.entry.workspace_id !== workspaceId || current.entry.node_id !== entry?.entry.node_id) return;
    const epoch = lifetime;
    await action(async () => {
      let requestId: string;
      if (selectedForm === 'upload' || selectedForm === 'replace') {
        if (!file) throw new Error('Choose a file.');
        if (file.size > DRIVE_FILE_MAX_BYTES) throw new Error('Upload exceeds 16 MiB.');
        requestId = await controller.upload(file, selectedForm === 'replace' ? {
          operation: 'update', id: current.entry, expected_revision: current.revision, content_type: file.type || 'application/octet-stream',
        } : {
          operation: 'create', parent: current.entry, name: file.name, content_type: file.type || 'application/octet-stream',
        });
      } else if (selectedForm === 'relocate') {
        requestId = await controller.mutate({ operation: 'relocate', id: current.entry, expected_revision: current.revision, parent: { workspace_id: workspaceId, node_id: parseDriveDecimal(parentId) }, name });
      } else {
        requestId = await controller.mutate(selectedForm === 'folder' ? { operation: 'create_folder', parent: current.entry, name } : { operation: 'create_text', parent: current.entry, name, text: newText, content_type: 'text/markdown' });
      }
      if (epoch !== lifetime) return;
      const receipt = controller.state.receipts.find((r) => r.requestId === requestId);
      if (receipt?.state === 'committed') {
        form = null; file = null;
        await controller.refresh();
      }
    });
  }
  async function remove(): Promise<void> {
    const current = entry;
    if (!current || !canWrite || !current.parent) return;
    if (!confirm(`Delete “${current.name}”? This cannot be undone. Folders must be empty; contents are never recursively deleted.`)) return;
    await action(async () => {
      await controller.mutate({ operation: 'delete', id: current.entry, expected_revision: current.revision });
      // Stay on this stable URL; a subsequent lookup correctly reports NotFound.
      if (controller.state.receipts.at(-1)?.state === 'committed') await controller.refresh();
    });
  }
  async function reconcileReceipt(requestId: string): Promise<void> {
    const epoch = lifetime;
    await action(async () => {
      await controller.reconcile(requestId);
      if (epoch !== lifetime) return;
      if (controller.state.receipts.find((r) => r.requestId === requestId)?.state === 'committed') {
        form = null; file = null;
        await controller.refresh();
      }
    });
  }
  async function copyUrl(): Promise<void> {
    const current = entry;
    const epoch = lifetime;
    if (!current) return;
    try {
      await navigator.clipboard.writeText(new URL(driveHref(current.entry), location.origin).href);
      if (epoch === lifetime) copyState = 'URL copied';
    } catch { if (epoch === lifetime) copyState = 'Copy the latest URL below.'; }
  }
</script>

<section class="drive-page" aria-labelledby="drive-heading" data-drive-ready={!$controller.loading}>
  <header class="drive-heading">
    <div>
      <h1 id="drive-heading">{entry?.parent ? entry.name : 'Drive'}</h1>
      <p class="drive-note">Shared files · latest only. Saving replaces the current content; there is no version history.</p>
    </div>
    <button type="button" disabled={$controller.loading || pending} onclick={() => action(() => entry ? controller.refresh() : controller.select(workspaceId, nodeId ? { workspace_id: workspaceId, node_id: nodeId } : undefined))}>Refresh</button>
  </header>
  <nav class="drive-breadcrumb" aria-label="Drive folders">
    <a href={workspaceRoute(workspaceId, '/drive')}>Drive</a>
    {#each ancestors.filter((a) => a.parent !== null) as ancestor (ancestor.entry.node_id)}
      <span aria-hidden="true">/</span><a href={driveHref(ancestor.entry)}>{ancestor.name}</a>
    {/each}
    {#if entry?.parent}<span aria-hidden="true">/</span><span aria-current="page">{entry.name}</span>{/if}
  </nav>
  {#if breadcrumbError}<p class="drive-error" role="status">{breadcrumbError}</p>{/if}
  {#if localError}<p class="drive-error" role="alert">{localError}</p>{/if}
  {#if $controller.error}<p class="drive-error" role="alert">{$controller.error}. No empty content or success is inferred from this failure.</p>{/if}
  {#if denied}<p role="status">Read only / access denied by Backend. Editing and upload are unavailable.</p>{/if}
  {#if $controller.loading}<p class="drive-state" role="status">Loading Drive…</p>{/if}
  {#if entry}
    <div class="drive-metadata">
      <span>{entry.kind} · {formatBytes(entry.size)}</span>
      <span>Updated {formatDate(entry.updated_at)} by {entry.updated_by}</span>
    </div>
    {#if entry.parent}
      <div class="drive-toolbar">
        {#if entry.kind === 'file'}<a class="drive-download" href={driveDownloadUrl(workspaceId, entry.entry, entry.revision)} download={entry.name}>Download</a>{/if}
        <button type="button" onclick={copyUrl}>Copy latest URL</button>
        {#if canEdit}<button type="button" onclick={() => editing = !editing}>{editing ? 'View' : 'Edit text'}</button>{/if}
        {#if canWrite}
          <button type="button" onclick={() => openForm('relocate')}>Rename / move</button>
          {#if entry.kind === 'file'}<button type="button" onclick={() => openForm('replace')}>Replace file</button>{/if}
          <button type="button" class="drive-danger" onclick={remove}>Delete</button>
        {/if}
      </div>
      <p class="drive-url"><a href={driveHref(entry.entry)}>{driveHref(entry.entry)}</a> <span role="status">{copyState}</span></p>
    {/if}
    {#if kind === 'folder'}
      <div class="drive-toolbar">
        {#if canWrite}
          <button type="button" onclick={() => openForm('folder')}>New folder</button>
          <button type="button" onclick={() => openForm('text')}>New Markdown</button>
          <button type="button" onclick={() => openForm('upload')}>Upload</button>
        {/if}
      </div>
      <form class="drive-search" onsubmit={(event) => { event.preventDefault(); void action(() => controller.search(searchText, includeText)); }}>
        <label>Search Drive <input type="search" bind:value={searchText} maxlength="256" /></label>
        <label class="drive-checkbox"><input type="checkbox" bind:checked={includeText} /> Include bounded file text</label>
        <button type="submit" disabled={$controller.loading || !searchText}>Search</button>
        {#if $controller.search}<button type="button" onclick={() => action(() => controller.refresh())}>Show folder</button>{/if}
      </form>
      {#if $controller.search}<p class="drive-note">Search results across this Drive: {$controller.search.query}</p>{/if}
      {#if !$controller.loading && !$controller.error && !$controller.entries.length}<p class="drive-state">{$controller.search ? 'No matching files.' : 'This folder is empty.'}</p>{/if}
      {#if $controller.entries.length}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard access to the bounded horizontal scroll region.) -->
        <div class="drive-table-region" tabindex="0" role="region" aria-label="Drive entries">
          <table class="drive-table">
            <thead><tr><th>Name</th><th>Kind / size</th><th>Updated</th></tr></thead>
            <tbody>
              {#each $controller.entries as item (item.entry.node_id)}
                <tr><td><a href={driveHref(item.entry)}>{item.name}</a></td><td>{item.kind} · {formatBytes(item.size)}</td><td>{formatDate(item.updated_at)}<br />{item.updated_by}</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
      {#if $controller.nextAfter}<button type="button" disabled={$controller.loading} onclick={() => action(() => controller.loadMore())}>Load more</button>{/if}
    {:else if kind === 'markdown' || kind === 'text'}
      {#if $controller.truncated}<p role="status">Preview truncated at 64 KiB. Download the full file; partial content cannot be saved.</p>{/if}
      {#if draft?.conflict}
        <div class="drive-conflict" role="alert"><strong>Revision conflict — your draft is retained.</strong><p>Another writer changed this file. Refresh displays the latest content without changing your draft or its expected revision. Copy your draft before explicitly discarding it; it is never automatically retried.</p></div>
      {/if}
      {#if editing && draft}
        <div class="drive-editor">
          <label>Draft (expected revision {draft.expectedRevision})
            <textarea value={draft.text} oninput={(event) => controller.edit(event.currentTarget.value)} readonly={denied} spellcheck="false"></textarea>
          </label>
          <div class="drive-toolbar">
            {#if canWrite}<button type="button" disabled={!dirty || draft.conflict || new TextEncoder().encode(draft.text).length > DRIVE_TEXT_MAX_BYTES} onclick={() => action(() => controller.save())}>Save</button>{/if}
            <button type="button" disabled={unresolved} onclick={() => { if (confirm('Discard your draft and load the current Backend revision?')) void action(() => controller.discardDraft()); }}>Discard draft / load latest</button>
            <span role="status">{dirty ? 'Unsaved draft' : 'Saved content'} · {new TextEncoder().encode(draft.text).length} / 65536 bytes</span>
          </div>
        </div>
      {/if}
      {#if $controller.text !== null}
        <article class="drive-preview" aria-label="Current saved content">
          {#if $controller.text === ''}<p class="drive-note">Empty file.</p>{:else if kind === 'markdown'}<DriveDocument text={$controller.text} {workspaceId} />{:else}<pre>{$controller.text}</pre>{/if}
        </article>
      {/if}
    {:else if kind === 'image'}
      {#if (entry.size ?? Infinity) > DRIVE_RESPONSE_MAX_BYTES}<p role="status">Image exceeds the 256 KiB preview limit. Download to view.</p>
      {:else if imageFailed}<p class="drive-error" role="alert">Image could not be decoded. Download is available; no image is assumed.</p>
      {:else if imageUrl}
        {#key imageUrl}<img class="drive-image" src={imageUrl} alt={entry.name} onerror={(event) => { if (event.currentTarget.getAttribute('src') === imageUrl) imageFailed = true; }} />{/key}
      {/if}
    {:else if kind === 'download'}<p class="drive-state">This file type is download-only. HTML, SVG and other active content are never rendered here.</p>{/if}
  {/if}
  {#if form && entry}
    <form class="drive-form" onsubmit={(event) => { event.preventDefault(); void submitForm(); }}>
      <h2>{form === 'relocate' ? 'Rename / move' : form === 'replace' ? 'Replace file' : form === 'upload' ? 'Upload file' : form === 'folder' ? 'New folder' : 'New Markdown'}</h2>
      {#if form === 'replace' || form === 'relocate'}<p class="drive-note">Expected revision {formEntry?.revision}. Refresh does not retarget this form to a newer revision.</p>{/if}
      {#if form === 'upload' || form === 'replace'}
        <label>File (maximum 16 MiB)<input type="file" onchange={(event) => file = event.currentTarget.files?.[0] ?? null} disabled={pending} /></label>
        {#if file}<p>{file.name} · {formatBytes(file.size)}</p>{/if}
      {:else}
        <label>Name<input bind:value={name} required maxlength="255" disabled={pending} /></label>
        {#if form === 'relocate'}
          <label>Destination folder node ID<input bind:value={parentId} required inputmode="numeric" pattern="[1-9][0-9]*" disabled={pending} /></label>
          <button type="button" disabled={pending} onclick={() => { const epoch = lifetime; const previous = parentId; void action(async () => { const root = await client.root(workspaceId); if (epoch === lifetime && form === 'relocate' && parentId === previous) parentId = root.entry.node_id; }); }}>Use Drive root</button>
          <p class="drive-note">Use the folder's stable URL ID. Rename / move updates DB metadata, not a copy of the file.</p>
        {/if}
        {#if form === 'text'}<label>Markdown<textarea bind:value={newText} disabled={pending}></textarea></label>{/if}
      {/if}
      <div class="drive-toolbar">
        <button type="submit" disabled={!canWrite || (file !== null && file.size > DRIVE_FILE_MAX_BYTES)}>{pending ? 'Transferring / awaiting DB publication…' : form === 'relocate' ? 'Apply rename / move' : form === 'replace' ? 'Replace' : form === 'upload' ? 'Upload file' : 'Create'}</button>
        {#if pending}<button type="button" onclick={() => controller.cancelPending()}>Cancel transfer / check outcome</button>{/if}
        <button type="button" disabled={pending} onclick={() => form = null}>Cancel form</button>
      </div>
    </form>
  {/if}
  {#if $controller.receipts.length}
    <section class="drive-receipts" aria-label="Drive mutation outcomes">
      {#each $controller.receipts as receipt (receipt.requestId)}
        <div role="status">
          <span>{receipt.operation}: {receipt.state === 'committed' ? 'Published — DB commit confirmed' : receipt.state === 'pending' ? 'Transferring / awaiting DB publication' : receipt.state === 'unknown' ? 'Outcome unknown — do not reupload' : 'Not committed'}</span>
          {#if receipt.error}<p class="drive-error">{receipt.error}</p>{/if}
          {#if receipt.state === 'unknown'}<button type="button" onclick={() => reconcileReceipt(receipt.requestId)}>Check request result</button>{/if}
          <details><summary>Request ID</summary><code>{receipt.requestId}</code></details>
        </div>
      {/each}
    </section>
  {/if}
</section>

<style>
  .drive-page { display: grid; gap: var(--space-4); min-width: 0; max-width: 100%; }
  .drive-heading { display: flex; align-items: start; justify-content: space-between; gap: var(--space-4); }
  .drive-heading > div { min-width: 0; }
  h1 { margin: 0; font-size: var(--font-size-title); line-height: var(--line-height-title); overflow-wrap: anywhere; }
  h2 { margin: 0; font-size: var(--font-size-body); }
  p { margin: 0; overflow-wrap: anywhere; }
  .drive-note, .drive-metadata, .drive-state { color: var(--text-muted); }
  .drive-note { margin-top: var(--space-2); }
  .drive-metadata { display: flex; flex-wrap: wrap; gap: var(--space-2) var(--space-4); font-size: var(--font-size-compact); }
  .drive-toolbar, .drive-breadcrumb { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .drive-breadcrumb { font-size: var(--font-size-compact); }
  .drive-breadcrumb a, .drive-breadcrumb span, .drive-url { min-width: 0; overflow-wrap: anywhere; }
  button, .drive-download { padding: var(--space-2) var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); color: var(--text-strong); font-size: var(--font-size-body); cursor: pointer; }
  button:hover:not(:disabled), .drive-download:hover { background: var(--interactive-hover); }
  button:disabled { cursor: not-allowed; opacity: 0.55; }
  button:focus-visible, input:focus-visible, textarea:focus-visible, a:focus-visible, [tabindex]:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  a { color: var(--accent); }
  .drive-error, .drive-danger { color: var(--danger); }
  .drive-conflict { display: grid; gap: var(--space-2); color: var(--warning); }
  .drive-search { display: flex; flex-wrap: wrap; gap: var(--space-3); align-items: end; }
  label { display: grid; gap: var(--space-1); min-width: 0; }
  input, textarea { min-width: 0; max-width: 100%; padding: var(--space-2); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg); color: var(--text); font: inherit; }
  .drive-checkbox { display: flex; align-items: center; gap: var(--space-2); padding-block: var(--space-2); }
  .drive-table-region { max-width: 100%; overflow-x: auto; }
  .drive-table { width: 100%; border-collapse: collapse; font-size: var(--font-size-compact); }
  th, td { padding: var(--space-3) var(--space-2); text-align: left; vertical-align: top; border-bottom: 1px solid var(--line); }
  td:first-child { width: 50%; min-width: 10rem; overflow-wrap: anywhere; }
  td:last-child { overflow-wrap: anywhere; }
  .drive-form, .drive-editor { display: grid; gap: var(--space-3); padding-block: var(--space-4); border-top: 1px solid var(--line); }
  textarea { width: 100%; min-height: 12rem; box-sizing: border-box; resize: vertical; font-family: var(--font-mono); }
  .drive-preview { min-width: 0; }
  .drive-preview pre { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0; }
  .drive-image { display: block; max-width: 100%; max-height: 60vh; object-fit: contain; }
  .drive-receipts { display: grid; gap: var(--space-3); font-size: var(--font-size-compact); }
  .drive-receipts code { overflow-wrap: anywhere; }
  @media (max-width: 600px) { .drive-heading { flex-wrap: wrap; } .drive-search > label:first-child { width: 100%; } }
</style>
