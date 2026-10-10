<script lang="ts">
  import { onDestroy } from 'svelte';
  import { beforeNavigate } from '$app/navigation';
  import { createDriveClient, driveDownloadUrl, DRIVE_FILE_MAX_BYTES, DRIVE_RESPONSE_MAX_BYTES, type DriveEntry, parseDriveDecimal } from '#lib/workspace/drive/api.ts';
  import { DriveController } from '#lib/workspace/drive/controller.svelte.ts';
  import { driveHref } from '#lib/workspace/resource-links.ts';
  import { workspaceRoute, formatDate } from '#lib/workspace/api/http.ts';
  import DriveDocument from './DriveDocument.svelte';
  import DriveIcon from './DriveIcon.svelte';
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
  let createMenu = $state(false);
  let actionsMenu = $state(false);
  let formDialog: HTMLDialogElement;
  let returnFocus: HTMLElement | null = null;
  let destination = $state<DriveEntry | null>(null);
  let destinationFolders = $state<DriveEntry[]>([]);
  let destinationNext = $state<string | null>(null);
  let destinationBusy = $state(false);
  let destinationError = $state<string | null>(null);
  let destinationEpoch = 0;
  let destinationAbort: AbortController | null = null;
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
    ++destinationEpoch; destinationAbort?.abort();
    form = null; formEntry = null; createMenu = false; actionsMenu = false; editing = false; localError = null; file = null; searchText = ''; copyState = '';
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
  $effect(() => {
    if (!formDialog) return;
    if (form && !formDialog.open) {
      formDialog.showModal();
      formDialog.querySelector<HTMLInputElement>('input')?.focus();
    } else if (!form && formDialog.open) formDialog.close();
  });
  onDestroy(() => { ++lifetime; ++breadcrumbEpoch; ++destinationEpoch; destinationAbort?.abort(); controller.dispose(); });
  beforeNavigate(({ to, cancel }) => {
    // Within the optional Drive route the controller keeps per-identity drafts.
    // Leaving it destroys this session store; make that loss an explicit decision.
    const isDrive = to?.route.id?.endsWith('/drive/[[nodeId]]') === true;
    if (!isDrive && (dirty || unresolved) && !confirm('Leave Drive? Unsaved drafts and unresolved request IDs are kept only in this page session. Copy them before leaving.')) cancel();
  });
  function openForm(next: typeof form): void {
    if (!entry || !canWrite) return;
    const active = document.activeElement;
    returnFocus = active instanceof HTMLElement ? active.closest('details')?.querySelector('summary') ?? active : null;
    createMenu = false; actionsMenu = false;
    form = next; formEntry = entry; localError = null; file = null; newText = '';
    name = next === 'relocate' ? entry.name : next === 'text' ? 'document.md' : '';
    parentId = entry.parent?.node_id ?? entry.entry.node_id;
    destination = null; destinationFolders = []; destinationNext = null; destinationError = null;
    if (next === 'relocate') void loadDestination(entry.parent ?? undefined);
  }
  function closeForm(): void {
    if (pending) return;
    ++destinationEpoch; destinationAbort?.abort();
    form = null;
  }
  async function loadDestination(ref?: DriveEntry['entry'], append = false): Promise<void> {
    const epoch = ++destinationEpoch;
    const ws = workspaceId;
    const snapshot = formEntry;
    destinationAbort?.abort();
    destinationAbort = new AbortController();
    const signal = destinationAbort.signal;
    destinationBusy = true; destinationError = null;
    try {
      const folder = ref ? await client.metadata(ws, ref, signal) : await client.root(ws, signal);
      if (folder.kind !== 'folder') throw new Error('Choose a folder.');
      const result = await client.list(ws, folder.entry, { limit: 100, after: append ? destinationNext ?? undefined : undefined }, signal);
      if (epoch !== destinationEpoch || ws !== workspaceId || form !== 'relocate' || snapshot !== formEntry) return;
      destination = folder; parentId = folder.entry.node_id;
      const folders = result.entries.filter((item) => item.kind === 'folder' && item.entry.node_id !== snapshot?.entry.node_id);
      destinationFolders = append ? [...destinationFolders, ...folders] : folders;
      destinationNext = result.next_after;
    } catch {
      if (epoch === destinationEpoch && !signal.aborted) destinationError = 'Folders could not be loaded. Retry before moving this item.';
    } finally {
      if (epoch === destinationEpoch) destinationBusy = false;
    }
  }
  function typeLabel(item: DriveEntry): string {
    const type = previewKind(item);
    return type === 'folder' ? 'Folder' : type === 'markdown' ? 'Markdown' : type === 'text' ? 'Text document' : type === 'image' ? 'Image' : 'File';
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
          operation: 'update', id: current.entry, expected_mutation_id: current.last_mutation_id, content_type: file.type || 'application/octet-stream',
        } : {
          operation: 'create', parent: current.entry, name: file.name, content_type: file.type || 'application/octet-stream',
        });
      } else if (selectedForm === 'relocate') {
        if (!destination || destinationBusy || destinationError) throw new Error('Choose an available destination folder.');
        requestId = await controller.mutate({ operation: 'relocate', id: current.entry, expected_mutation_id: current.last_mutation_id, parent: { workspace_id: workspaceId, node_id: parseDriveDecimal(parentId) }, name });
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
      await controller.mutate({ operation: 'delete', id: current.entry, expected_mutation_id: current.last_mutation_id });
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

<svelte:window onclick={(event) => {
  if (!(event.target instanceof Element) || !event.target.closest('.drive-menu')) {
    createMenu = false; actionsMenu = false;
  }
}} onkeydown={(event) => {
  if (event.key === 'Escape' && !form) {
    const menu = document.activeElement?.closest<HTMLDetailsElement>('details.drive-menu');
    if (menu?.open) {
      menu.open = false;
      menu.querySelector('summary')?.focus();
      event.preventDefault();
    }
    createMenu = false; actionsMenu = false;
  }
}} />

{#snippet notices()}
  {#if localError}<p class="drive-error" role="alert">{localError}</p>{/if}
  {#if $controller.error}<p class="drive-error" role="alert">{$controller.error}</p>{/if}
  {#if denied}<p class="drive-note" role="status">Read only / access denied by Backend. Editing and upload are unavailable.</p>{/if}
{/snippet}

{#snippet receipts()}
  {#if $controller.receipts.length}
    <section class="drive-receipts" aria-label="Drive mutation outcomes">
      {#each $controller.receipts as receipt (receipt.requestId)}
        <div class="drive-receipt" role="status">
          <span>{receipt.state === 'committed' ? 'Published — DB commit confirmed' : receipt.state === 'pending' ? 'Transferring / awaiting DB publication' : receipt.state === 'unknown' ? 'Outcome unknown — do not reupload' : 'Not committed'}</span>
          {#if receipt.error}<p class="drive-error">{receipt.error}</p>{/if}
          {#if receipt.state === 'unknown'}<button type="button" onclick={() => reconcileReceipt(receipt.requestId)}>Check request result</button>{/if}
          <details><summary>Request ID</summary><code>{receipt.requestId}</code></details>
        </div>
      {/each}
    </section>
  {/if}
{/snippet}

<section class="drive-page" aria-labelledby="drive-heading" data-drive-ready={!$controller.loading}>
  <header class="drive-heading">
    <div class="drive-location">
      <nav class="drive-breadcrumb" aria-label="Drive folders">
        <a href={workspaceRoute(workspaceId, '/drive')}>Drive</a>
        {#each ancestors.filter((a) => a.parent !== null) as ancestor (ancestor.entry.node_id)}
          <span aria-hidden="true">/</span><a href={driveHref(ancestor.entry)}>{ancestor.name}</a>
        {/each}
        {#if entry?.parent}<span aria-hidden="true">/</span><span aria-current="page">{entry.name}</span>{/if}
      </nav>
      <h1 id="drive-heading">{entry?.parent ? entry.name : 'Drive'}</h1>
      <p class="drive-note">{kind === 'folder' || !entry ? 'Files shared with your workspace.' : `${typeLabel(entry)} · ${formatBytes(entry.size)}`}</p>
    </div>
    <div class="drive-toolbar">
      <button class="icon-button" type="button" aria-label="Refresh" title="Refresh" disabled={$controller.loading || pending} onclick={() => action(() => entry ? controller.refresh() : controller.select(workspaceId, nodeId ? { workspace_id: workspaceId, node_id: nodeId } : undefined))}><DriveIcon name="refresh" /></button>
      {#if entry && kind === 'folder' && canWrite}
        <details class="drive-menu drive-create-menu" bind:open={createMenu}>
          <summary><DriveIcon name="plus" />New</summary>
          <div class="drive-menu-panel">
            <button type="button" onclick={() => openForm('folder')}><DriveIcon name="folder" />New folder</button>
            <button type="button" onclick={() => openForm('text')}><DriveIcon name="markdown" />New Markdown</button>
          </div>
        </details>
        <button class="primary" type="button" onclick={() => openForm('upload')}><DriveIcon name="upload" />Upload</button>
      {/if}
      {#if entry?.parent}
        {#if entry.kind === 'file'}<a class="drive-button" href={driveDownloadUrl(workspaceId, entry.entry, entry.last_mutation_id)} download={entry.name}><DriveIcon name="download" />Download</a>{/if}
        {#if canEdit}<button class="primary" type="button" onclick={() => editing = !editing}><DriveIcon name="edit" />{editing ? 'View' : 'Edit text'}</button>{/if}
        <details class="drive-menu" bind:open={actionsMenu}>
          <summary class="icon-button" aria-label="More actions" title="More actions"><DriveIcon name="more" /></summary>
          <div class="drive-menu-panel">
            <button type="button" onclick={() => { actionsMenu = false; void copyUrl(); }}><DriveIcon name="link" />Copy latest URL</button>
            {#if canWrite}
              <button type="button" onclick={() => openForm('relocate')}><DriveIcon name="edit" />Rename / move</button>
              {#if entry.kind === 'file'}<button type="button" onclick={() => openForm('replace')}><DriveIcon name="upload" />Replace file</button>{/if}
              <button type="button" class="drive-danger" onclick={() => { actionsMenu = false; void remove(); }}><DriveIcon name="trash" />Delete</button>
            {/if}
          </div>
        </details>
      {/if}
    </div>
  </header>
  {#if breadcrumbError}<p class="drive-error" role="status">{breadcrumbError}</p>{/if}
  {#if !form}{@render notices()}{/if}
  {#if copyState}<p class="drive-note" role="status">{copyState} {#if entry}<a href={driveHref(entry.entry)}>Latest file link</a>{/if}</p>{/if}
  {#if $controller.loading}<p class="drive-state" role="status">Loading Drive…</p>{/if}
  {#if entry}
    {#if kind === 'folder'}
      <div class="drive-browser">
        <form class="drive-search" role="search" onsubmit={(event) => { event.preventDefault(); void action(() => controller.search(searchText, includeText)); }}>
          <div class="drive-search-field">
            <DriveIcon name="search" />
            <input type="search" aria-label="Search Drive" placeholder="Search files and folders…" bind:value={searchText} maxlength="256" />
            <button type="submit" class="icon-button" aria-label="Search" title="Search" disabled={$controller.loading || !searchText}><DriveIcon name="arrow" /></button>
          </div>
          <details class="drive-search-options"><summary>Search options</summary><label class="drive-checkbox"><input type="checkbox" bind:checked={includeText} />Search file contents</label></details>
          {#if $controller.search}<button type="button" onclick={() => action(() => controller.refresh())}>Show folder</button>{/if}
        </form>
        <div class="drive-list-heading">
          <span>{$controller.search ? `Results for “${$controller.search.query}”` : 'Files and folders'}</span>
          <span>{$controller.entries.length}{$controller.nextAfter ? '+' : ''} items</span>
        </div>
        {#if !$controller.loading && !$controller.error && !$controller.entries.length}
          <div class="drive-empty">
            <span class="drive-empty-icon"><DriveIcon name={$controller.search ? 'search' : 'folder'} /></span>
            <h2>{$controller.search ? 'No matching files.' : 'This folder is empty.'}</h2>
            <p class="drive-note">{$controller.search ? 'Try a different name, or include file contents in your search.' : 'Upload a file or create a document to start sharing.'}</p>
          </div>
        {/if}
        {#if $controller.entries.length}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard access to the bounded horizontal scroll region.) -->
          <div class="drive-table-region" tabindex="0" role="region" aria-label="Drive entries">
            <table class="drive-table">
              <thead><tr><th>Name</th><th>Size</th><th>Modified</th><th><span class="sr-only">Open</span></th></tr></thead>
              <tbody>
                {#each $controller.entries as item (item.entry.node_id)}
                  <tr>
                    <td><a class="drive-file" href={driveHref(item.entry)} aria-label={item.name}>
                      <span class="drive-file-icon" data-kind={previewKind(item)}><DriveIcon name={previewKind(item)} /></span>
                      <span class="drive-file-label"><span class="drive-file-name" title={item.name}>{item.name}</span><small>{typeLabel(item)}</small></span>
                    </a></td>
                    <td class="drive-file-size">{item.kind === 'folder' ? '—' : formatBytes(item.size)}</td>
                    <td class="drive-file-date">{formatDate(item.updated_at)}</td>
                    <td class="drive-row-arrow" aria-hidden="true"><DriveIcon name="arrow" /></td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
        {#if $controller.nextAfter}<div class="drive-list-footer"><button type="button" disabled={$controller.loading} onclick={() => action(() => controller.loadMore())}>Load more</button></div>{/if}
      </div>
    {:else if kind === 'markdown' || kind === 'text'}
      {#if $controller.truncated}<p class="drive-state" role="status">Preview truncated at 64 KiB. Download the full file; partial content cannot be saved.</p>{/if}
      {#if draft?.conflict}
        <div class="drive-conflict" role="alert"><strong>Content changed — your draft is retained.</strong><p>Another writer changed this file. Refresh displays the latest content without changing your draft or the saved content it was based on. Copy your draft before explicitly discarding it; it is never automatically retried.</p></div>
      {/if}
      <div class="drive-document" class:is-editing={editing && draft !== null}>
        {#if editing && draft}
          <div class="drive-editor">
            <label>Draft
              <textarea value={draft.text} oninput={(event) => controller.edit(event.currentTarget.value)} readonly={denied} spellcheck="false"></textarea>
            </label>
            <div class="drive-toolbar">
              {#if canWrite}<button class="primary" type="button" disabled={!dirty || draft.conflict || new TextEncoder().encode(draft.text).length > DRIVE_TEXT_MAX_BYTES} onclick={() => action(() => controller.save())}>Save</button>{/if}
              <button type="button" disabled={unresolved} onclick={() => { if (confirm('Discard your draft and load the latest saved content?')) void action(() => controller.discardDraft()); }}>Discard draft / load latest</button>
              <span class="drive-note" role="status">{dirty ? 'Unsaved draft' : 'Saved content'} · {formatBytes(new TextEncoder().encode(draft.text).length)} / 64 KiB</span>
            </div>
          </div>
        {/if}
        {#if $controller.text !== null}
          <article class="drive-preview" aria-label="Current saved content">
            {#if editing}<p class="drive-preview-label">Saved version</p>{/if}
            {#if $controller.text === ''}<p class="drive-note">Empty file.</p>{:else if kind === 'markdown'}<DriveDocument text={$controller.text} {workspaceId} />{:else}<pre>{$controller.text}</pre>{/if}
          </article>
        {/if}
      </div>
    {:else if kind === 'image'}
      <div class="drive-image-stage">
        {#if (entry.size ?? Infinity) > DRIVE_RESPONSE_MAX_BYTES}<p class="drive-state" role="status">Image exceeds the 256 KiB preview limit. Download to view.</p>
        {:else if imageFailed}<p class="drive-error" role="alert">Image could not be decoded. Download is available; no image is assumed.</p>
        {:else if imageUrl}
          {#key imageUrl}<img class="drive-image" src={imageUrl} alt={entry.name} onerror={(event) => { if (event.currentTarget.getAttribute('src') === imageUrl) imageFailed = true; }} />{/key}
        {/if}
      </div>
    {:else if kind === 'download'}
      <div class="drive-empty drive-download-preview"><span class="drive-empty-icon"><DriveIcon name="download" /></span><h2>No preview available</h2><p class="drive-note">This file type is download-only. HTML, SVG and other active content are never rendered here.</p><a class="drive-button" href={driveDownloadUrl(workspaceId, entry.entry, entry.last_mutation_id)} download={entry.name}>Download file</a></div>
    {/if}
    {#if entry.parent}
      <details class="drive-details"><summary><DriveIcon name="info" />Details</summary>
        <dl><div><dt>Updated</dt><dd>{formatDate(entry.updated_at)}</dd></div><div><dt>By</dt><dd>{entry.updated_by}</dd></div><div><dt>Last change request</dt><dd>{entry.last_mutation_id}</dd></div><div><dt>Latest URL</dt><dd><a href={driveHref(entry.entry)}>{driveHref(entry.entry)}</a></dd></div></dl>
        <p class="drive-note">Saving replaces the current content. There is no version history.</p>
      </details>
    {/if}
  {/if}
  {#if !form}{@render receipts()}{/if}
</section>

<dialog class="drive-dialog" bind:this={formDialog} aria-labelledby="drive-form-heading" oncancel={(event) => { event.preventDefault(); closeForm(); }} onclose={() => { form = null; if (returnFocus?.isConnected) returnFocus.focus(); }}>
  {#if form && entry}
    <form class="drive-form" onsubmit={(event) => { event.preventDefault(); void submitForm(); }}>
      <header class="drive-dialog-heading">
        <div><h2 id="drive-form-heading">{form === 'relocate' ? 'Rename / move' : form === 'replace' ? 'Replace file' : form === 'upload' ? 'Upload file' : form === 'folder' ? 'New folder' : 'New Markdown'}</h2><p class="drive-note">{form === 'replace' || form === 'relocate' ? formEntry?.name : `In ${entry.parent ? entry.name : 'Drive'}`}</p></div>
        <button class="icon-button" type="button" aria-label="Close dialog" title="Close" disabled={pending} onclick={closeForm}><DriveIcon name="close" /></button>
      </header>
      {@render notices()}
      {#if form === 'upload' || form === 'replace'}
        <label class="drive-upload-picker"><span class="drive-empty-icon"><DriveIcon name="upload" /></span><strong>{file ? file.name : 'Choose a file to upload'}</strong><span class="drive-note">{file ? formatBytes(file.size) : 'Maximum file size: 16 MiB'}</span><input aria-label="File (maximum 16 MiB)" type="file" onchange={(event) => file = event.currentTarget.files?.[0] ?? null} disabled={pending} /></label>
        {#if file && file.size > DRIVE_FILE_MAX_BYTES}<p class="drive-error" role="alert">Upload exceeds 16 MiB. Choose a smaller file.</p>{/if}
      {:else}
        <label>Name<input bind:value={name} required maxlength="255" disabled={pending} autocomplete="off" /></label>
        {#if form === 'relocate'}
          <fieldset class="drive-folder-picker" disabled={pending}>
            <legend>Destination folder</legend>
            <div class="drive-folder-location"><DriveIcon name="folder" /><strong>{destination ? destination.parent ? destination.name : 'Drive' : 'Loading…'}</strong>
              <button type="button" class="icon-button" aria-label="Use Drive root" title="Drive root" disabled={destinationBusy} onclick={() => loadDestination()}><DriveIcon name="folder" /></button>
              {#if destination?.parent}<button type="button" class="icon-button" aria-label="Parent folder" title="Parent folder" disabled={destinationBusy} onclick={() => loadDestination(destination?.parent ?? undefined)}><DriveIcon name="back" /></button>{/if}
            </div>
            {#if destinationError}<p class="drive-error" role="alert">{destinationError}</p><button type="button" onclick={() => loadDestination(destination?.entry)}>Retry folders</button>{/if}
            {#if destinationBusy}<p class="drive-note" role="status">Loading folders…</p>{/if}
            <div class="drive-folder-list">
              {#each destinationFolders as folder (folder.entry.node_id)}<button type="button" disabled={destinationBusy} onclick={() => loadDestination(folder.entry)}><DriveIcon name="folder" /><span>{folder.name}</span><DriveIcon name="arrow" /></button>{/each}
              {#if !destinationBusy && !destinationError && !destinationFolders.length}<p class="drive-note">No subfolders. This folder will be used.</p>{/if}
              {#if destinationNext}<button type="button" disabled={destinationBusy} onclick={() => loadDestination(destination?.entry, true)}>More folders</button>{/if}
            </div>
          </fieldset>
        {/if}
        {#if form === 'text'}<label>Markdown<textarea bind:value={newText} disabled={pending} placeholder="Write your document…"></textarea></label>{/if}
      {/if}
      {#if form === 'replace' || form === 'relocate'}<p class="drive-note">Changes by another writer will not be overwritten.</p>{/if}
      {#if form === 'replace'}<p class="drive-note">This replaces the current file. There is no version history.</p>{/if}
      <footer class="drive-dialog-actions">
        <button type="button" disabled={pending} onclick={closeForm}>Cancel form</button>
        {#if pending}<button type="button" onclick={() => controller.cancelPending()}>Cancel transfer / check outcome</button>{/if}
        <button class="primary" type="submit" disabled={!canWrite || ((form === 'upload' || form === 'replace') && (!file || file.size > DRIVE_FILE_MAX_BYTES)) || (form === 'relocate' && (!destination || destinationBusy || destinationError !== null))}>{pending ? 'Transferring…' : form === 'relocate' ? 'Apply rename / move' : form === 'replace' ? 'Replace' : form === 'upload' ? 'Upload file' : 'Create'}</button>
      </footer>
      {@render receipts()}
    </form>
  {/if}
</dialog>

<style>
  .drive-page { display: grid; gap: var(--space-4); min-width: 0; max-width: 100%; }
  .drive-heading { display: flex; align-items: center; justify-content: space-between; gap: var(--space-4); flex-wrap: wrap; }
  .drive-location { min-width: 0; flex: 1 1 16rem; }
  h1 { margin: var(--space-2) 0 0; font-size: var(--font-size-title); line-height: var(--line-height-title); color: var(--text-strong); overflow-wrap: anywhere; }
  h2 { margin: 0; color: var(--text-strong); font-size: var(--font-size-body); }
  p { margin: 0; overflow-wrap: anywhere; }
  .drive-note, .drive-state { color: var(--text-muted); font-size: var(--font-size-compact); }
  .drive-location > .drive-note, .drive-dialog-heading .drive-note { margin-top: var(--space-1); }
  .drive-toolbar, .drive-breadcrumb { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .drive-breadcrumb { font-size: var(--font-size-compact); color: var(--text-muted); }
  .drive-breadcrumb a { color: inherit; }
  .drive-breadcrumb a, .drive-breadcrumb span { min-width: 0; overflow-wrap: anywhere; }
  button, .drive-button, .drive-menu > summary { display: inline-flex; align-items: center; justify-content: center; gap: var(--space-2); min-height: 2.25rem; padding: var(--space-2) var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); color: var(--text-strong); font: inherit; font-size: var(--font-size-compact); cursor: pointer; text-decoration: none; }
  button:hover:not(:disabled), .drive-button:hover, .drive-menu > summary:hover { background: var(--interactive-hover); }
  button.primary { background: var(--accent); color: var(--bg); border-color: var(--accent); }
  button.primary:hover:not(:disabled) { filter: brightness(1.08); }
  button:disabled { cursor: not-allowed; opacity: 0.5; }
  button:focus-visible, input:focus-visible, textarea:focus-visible, a:focus-visible, summary:focus-visible, [tabindex]:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .icon-button, .drive-menu > summary.icon-button { width: 2.25rem; padding: var(--space-2); flex: none; }
  a { color: var(--accent); }
  .drive-menu { position: relative; }
  .drive-menu > summary { list-style: none; }
  .drive-menu > summary::-webkit-details-marker { display: none; }
  .drive-menu-panel { position: absolute; z-index: 5; inset-block-start: calc(100% + var(--space-2)); inset-inline-end: 0; min-width: 13rem; display: grid; padding: var(--space-1); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); box-shadow: 0 8px 24px rgb(0 0 0 / 0.12); }
  .drive-menu-panel button { justify-content: flex-start; border-color: transparent; background: transparent; }
  .drive-error, button.drive-danger { color: var(--danger); }
  .drive-error, .drive-conflict { padding: var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); font-size: var(--font-size-compact); }
  .drive-conflict { display: grid; gap: var(--space-2); color: var(--warning); }
  .drive-browser { min-width: 0; border: 1px solid var(--line); border-radius: var(--radius-soft); overflow: hidden; }
  .drive-search { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); padding: var(--space-3); border-bottom: 1px solid var(--line); }
  .drive-search-field { display: flex; align-items: center; gap: var(--space-2); color: var(--text-muted); flex: 1 1 16rem; min-width: 0; }
  .drive-search-field input { border: 0; padding: var(--space-2) 0; background: transparent; flex: 1; width: 0; }
  .drive-search-options { font-size: var(--font-size-compact); color: var(--text-muted); }
  summary { cursor: pointer; }
  .drive-checkbox { display: flex; align-items: center; gap: var(--space-2); padding-top: var(--space-2); }
  .drive-list-heading { display: flex; justify-content: space-between; gap: var(--space-2); padding: var(--space-3); color: var(--text-muted); font-size: var(--font-size-compact); }
  .drive-table-region { max-width: 100%; overflow-x: auto; }
  .drive-table { width: 100%; border-collapse: collapse; font-size: var(--font-size-compact); table-layout: fixed; }
  th, td { padding: var(--space-3); text-align: left; border-bottom: 1px solid var(--line); }
  th { color: var(--text-muted); font-weight: 500; font-size: var(--font-size-compact); }
  th:first-child { width: 50%; }
  th:nth-child(2) { width: 15%; }
  th:last-child { width: 2rem; }
  tbody tr:last-child td { border-bottom: 0; }
  tbody tr:hover, tbody tr:focus-within { background: var(--interactive-hover); }
  .drive-file { display: flex; gap: var(--space-3); align-items: center; color: var(--text-strong); text-decoration: none; min-width: 0; }
  .drive-file-icon { display: grid; place-items: center; width: 2.5rem; height: 2.5rem; flex: none; border-radius: var(--radius-soft); background: var(--bg-raised); color: var(--text-muted); }
  .drive-file-icon :global(svg) { width: 1.4rem; height: 1.4rem; }
  .drive-file-icon[data-kind='folder'] { color: var(--warning); }
  .drive-file-icon[data-kind='markdown'], .drive-file-icon[data-kind='image'] { color: var(--accent); }
  .drive-file-label { min-width: 0; display: grid; gap: var(--space-1); }
  .drive-file-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .drive-file-label small, .drive-file-size, .drive-file-date, .drive-row-arrow { color: var(--text-muted); }
  .drive-file-date { overflow-wrap: anywhere; }
  .drive-list-footer { display: flex; justify-content: center; padding: var(--space-3); border-top: 1px solid var(--line); }
  .drive-empty { display: grid; justify-items: center; gap: var(--space-3); padding: var(--space-6) var(--space-4); text-align: center; }
  .drive-empty-icon { color: var(--text-muted); }
  .drive-empty-icon :global(svg) { width: 2.5rem; height: 2.5rem; }
  .drive-document { min-width: 0; border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); }
  .drive-preview { padding: clamp(1rem, 3vw, 2.5rem); min-width: 0; max-width: 64rem; margin-inline: auto; }
  .drive-preview pre { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0; font-size: var(--font-size-body); }
  .drive-preview-label { margin-bottom: var(--space-4); color: var(--text-muted); font-size: var(--font-size-compact); }
  .drive-editor { display: grid; gap: var(--space-3); padding: var(--space-4); border-bottom: 1px solid var(--line); }
  .drive-image-stage { display: grid; place-items: center; padding: var(--space-4); min-width: 0; min-height: 12rem; border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); }
  .drive-image { display: block; max-width: 100%; max-height: 60vh; object-fit: contain; }
  .drive-download-preview { border: 1px solid var(--line); border-radius: var(--radius-soft); }
  .drive-details { font-size: var(--font-size-compact); color: var(--text-muted); }
  .drive-details > summary { display: flex; align-items: center; gap: var(--space-2); width: fit-content; }
  .drive-details dl { display: grid; gap: var(--space-2); margin-block: var(--space-3); }
  .drive-details dl > div { display: grid; grid-template-columns: 5rem minmax(0, 1fr); gap: var(--space-3); }
  .drive-details dd { margin: 0; overflow-wrap: anywhere; }
  .drive-dialog { width: min(34rem, calc(100vw - 2rem)); max-width: none; max-height: calc(100dvh - 2rem); padding: 0; box-sizing: border-box; border: 1px solid var(--line); border-radius: var(--radius-soft); color: var(--text); background: var(--bg); box-shadow: 0 16px 64px rgb(0 0 0 / 0.25); }
  .drive-dialog::backdrop { background: rgb(0 0 0 / 0.35); }
  .drive-form { display: grid; gap: var(--space-4); padding: var(--space-5); min-width: 0; }
  .drive-dialog-heading { display: flex; justify-content: space-between; gap: var(--space-3); align-items: start; }
  .drive-dialog-heading > div { min-width: 0; }
  .drive-dialog-heading h2 { font-size: var(--font-size-title); }
  .drive-dialog-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--space-2); padding-top: var(--space-3); border-top: 1px solid var(--line); }
  label { display: grid; gap: var(--space-2); min-width: 0; font-size: var(--font-size-compact); }
  input, textarea { min-width: 0; max-width: 100%; box-sizing: border-box; padding: var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); background: var(--bg-raised); color: var(--text); font: inherit; }
  textarea { width: 100%; min-height: 16rem; resize: vertical; font-family: var(--font-mono); }
  .drive-upload-picker { justify-items: center; text-align: center; padding: var(--space-5) var(--space-3); border: 1px dashed var(--line); border-radius: var(--radius-soft); }
  .drive-upload-picker strong { max-width: 100%; overflow-wrap: anywhere; }
  input[type='file'] { width: 100%; border: 0; background: transparent; padding: var(--space-2); font-size: var(--font-size-compact); }
  .drive-folder-picker { min-width: 0; margin: 0; padding: var(--space-3); border: 1px solid var(--line); border-radius: var(--radius-soft); }
  .drive-folder-picker legend { font-size: var(--font-size-compact); padding-inline: var(--space-1); }
  .drive-folder-location { display: flex; align-items: center; gap: var(--space-2); margin-bottom: var(--space-2); }
  .drive-folder-location strong { flex: 1; min-width: 0; overflow-wrap: anywhere; font-size: var(--font-size-compact); }
  .drive-folder-list { display: grid; gap: var(--space-1); max-height: 13rem; overflow-y: auto; }
  .drive-folder-list > button { border-color: transparent; justify-content: flex-start; }
  .drive-folder-list > button > span { flex: 1; text-align: left; overflow-wrap: anywhere; }
  .drive-receipts { display: grid; gap: var(--space-2); font-size: var(--font-size-compact); }
  .drive-receipt { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2) var(--space-3); padding-block: var(--space-2); border-top: 1px solid var(--line); }
  .drive-receipt > span { color: var(--text-muted); }
  .drive-receipt code { overflow-wrap: anywhere; }
  .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }
  @media (max-width: 600px) {
    .drive-heading { gap: var(--space-3); }
    .drive-location { flex-basis: 100%; }
    .drive-toolbar { width: 100%; }
    .drive-toolbar > .drive-menu:last-child { margin-inline-start: auto; }
    .drive-create-menu .drive-menu-panel { inset-inline-start: 0; inset-inline-end: auto; }
    .drive-search { gap: var(--space-2); }
    .drive-search-field { flex-basis: 100%; }
    .drive-file-date, th:nth-child(3), .drive-row-arrow, th:last-child { display: none; }
    th:first-child { width: 72%; }
    th:nth-child(2) { width: 28%; }
    .drive-file { gap: var(--space-2); }
    th, td { padding: var(--space-3) var(--space-2); }
    .drive-form { padding: var(--space-4); }
    .drive-editor .drive-toolbar { align-items: flex-start; }
  }
</style>
