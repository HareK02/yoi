<script lang="ts">
  import { onMount } from 'svelte';
  import BevelLine from '#lib/workspace/ui/BevelLine.svelte';
  import { PermissionHttpError, readPermissions, referenceLabel } from './permissions.ts';
  import type { Lane, PermissionView } from './permissions.ts';
  let { workspaceId, subjectId }: { workspaceId: string; subjectId: string } = $props();
  let view = $state<PermissionView | null>(null);
  let busy = $state(false);
  let error = $state('');
  const lanes: { key: Lane; label: string }[] = [
    { key: 'execution', label: 'Direct operations' },
    { key: 'delegation', label: 'Delegation ceiling' },
  ];
  const sourceHref = $derived(`/w/${encodeURIComponent(workspaceId)}/settings/configuration`);
  async function reload() {
    if (busy) return;
    busy = true; error = '';
    // Old observations are not current authorization after a failed refresh.
    view = null;
    try { view = await readPermissions(fetch, workspaceId, subjectId); }
    catch (e) {
      error = e instanceof PermissionHttpError && [401, 403].includes(e.status)
        ? 'Configuration permissions are available to the Workspace owner only.'
        : e instanceof PermissionHttpError && e.status === 404
        ? 'Configuration source unavailable. The Subject may have been deleted.'
        : 'Source and permissions could not be read. No effective permission is inferred.';
    } finally { busy = false; }
  }
  onMount(() => { void reload(); });
</script>

<section class="permissions" aria-labelledby="permissions-heading" aria-busy={busy} data-permission-state={busy ? 'loading' : error ? 'error' : 'ready'}>
  <header>
    <h2 id="permissions-heading">Operation permissions</h2>
    <button type="button" class="workspace-secondary-button" disabled={busy} onclick={reload}>Reload permissions</button>
  </header>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if busy}<p role="status">Reading configuration source and permissions…</p>{/if}
  {#if view}
    <div class="source-state">
      <span>{view.source.source_stored ? 'Adopted configuration source' : 'Migration draft · not adopted'}</span>
      <a href={sourceHref}>Edit configuration sources</a>
    </div>
    {#if !view.source.source_stored}<p class="attention">These assignments are a migration preview, not an adopted policy. Review diagnostics and adopt the source in Configuration Sources.</p>{/if}
    <p class="context">Operation keys do not replace target, assignment or provider restrictions. Delegation is a ceiling, not a grant to a child or Job. Saved changes apply at the next current-policy check, not as proof that running work stopped.</p>
    {#if !view.policy}
      <p>No assignment for this Subject in this source snapshot. No default or effective permission is inferred.</p>
    {:else}
      <div class="lanes">
        {#each lanes as lane}
          {@const assignment = view.policy[lane.key]}
          {@const resolution = view.policy[lane.key === 'execution' ? 'execution_resolution' : 'delegation_resolution']}
          <section class="lane" aria-label={lane.label}>
            <BevelLine />
            <h3>{lane.label}</h3>
            <p class="assignment-state">{assignment.revoked ? 'Revoked' : assignment.grants.length ? 'Configured assignment' : 'Explicitly empty assignment'}</p>
            <h4>Grants</h4>
            {#if assignment.grants.length}<ul>{#each assignment.grants as ref}<li><code>{referenceLabel(ref)}</code></li>{/each}</ul>{:else}<p class="muted">No grants.</p>{/if}
            {#if assignment.exclusions.length}<h4>Exclusions</h4><ul>{#each assignment.exclusions as ref}<li><code>{referenceLabel(ref)}</code></li>{/each}</ul>{/if}
            <h4>{view.source.source_stored ? 'Resolved operation keys' : 'Draft operation key resolution'}</h4>
              {#if Object.keys(resolution.allowed).length === 0}<p class="muted">No allowed operation keys.</p>{/if}
              <ul class="resolved">{#each Object.entries(resolution.allowed) as [key, paths]}<li>
                <code>{key}</code>
                <details><summary>Permission origin</summary><ul>{#each paths as path}<li>{#each path as step, index}{#if index}{' → '}{/if}<span>{step.kind} · <code>{step.value}</code></span>{/each}</li>{/each}</ul></details>
              </li>{/each}</ul>
              {#if resolution.diagnostics.length}<h4>Resolution diagnostics</h4><ul>{#each resolution.diagnostics as diagnostic}<li>{diagnostic.kind}{#if 'value' in diagnostic} · <code>{diagnostic.value}</code>{/if}</li>{/each}</ul>{/if}
          </section>
        {/each}
      </div>
      {#if view.policy.legacy_constraints}
        <details class="legacy"><summary>Retained domain restrictions</summary>
          <p>Read-only migration evidence. Individual targets and provider limits are not editable data on permission keys.</p>
          <pre>{JSON.stringify(view.policy.legacy_constraints, null, 2)}</pre>
        </details>
      {/if}
    {/if}
    {#if view.source.migration_diagnostics.some((diagnostic) => diagnostic.subject_id === subjectId)}
      <details class="diagnostics"><summary>Subject migration diagnostics</summary><ul>{#each view.source.migration_diagnostics.filter((diagnostic) => diagnostic.subject_id === subjectId) as diagnostic}<li>{diagnostic.kind} · <code>{'operation' in diagnostic ? diagnostic.operation : diagnostic.permission.operation}</code>{#if diagnostic.kind === 'explicit_configuration_required'} — explicit configuration required; no broader grant is inferred.{/if}</li>{/each}</ul></details>
    {/if}
    <details class="catalog"><summary>Feature declaration catalog</summary>
      <p>Only explicit children are included. A shared dot prefix does not imply permission.</p>
      {#if !view.source.catalog.namespaces.length}<p>No Feature declarations.</p>{/if}
      {#each view.source.catalog.namespaces as namespace}
        <section class="namespace">
          <h3>Namespace · <code>{namespace.namespace}</code></h3>
          <p>Feature · <code>{namespace.feature}</code> · {namespace.enabled ? 'Enabled' : 'Disabled'}</p>
          <ul>{#each namespace.declaration.permissions as permission}<li><code>{permission.key}</code> — {permission.description}{#if permission.children.length}<div>Explicit children: <code>{permission.children.join(', ')}</code></div>{/if}</li>{/each}</ul>
          {#if namespace.declaration.sets.length}<h4>Set contributions</h4><ul>{#each namespace.declaration.sets as set}<li>Set · <code>{set.set}</code><ul>{#each set.children as ref}<li><code>{referenceLabel(ref)}</code></li>{/each}</ul></li>{/each}</ul>{/if}
        </section>
      {/each}
    </details>
    <details class="snapshot"><summary>Source snapshot identity</summary><dl><dt>History head</dt><dd><code>{view.source.history_head ?? 'Not yet adopted'}</code></dd><dt>Catalog digest</dt><dd><code>{view.source.catalog_digest}</code></dd>{#if view.policy}<dt>Policy change</dt><dd><code>{view.policy.change_id}</code></dd>{/if}</dl></details>
  {/if}
</section>
<style>
  .permissions { min-width: 0; }
  header, .source-state { display: flex; flex-wrap: wrap; justify-content: space-between; align-items: center; gap: 0.75rem; }
  header { margin-bottom: 1rem; }
  h2 { font-size: var(--font-size-body); line-height: var(--line-height-body); margin: 0; }
  h3 { font-size: var(--font-size-body); margin: 0 0 0.5rem; }
  h4 { font-size: var(--font-size-body); margin: 1rem 0 0.5rem; font-weight: 500; }
  p { margin: 0.75rem 0; }
  .context, .muted { color: var(--text-muted); }
  .attention, [role='alert'] { color: var(--warning); }
  .lanes { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 1.5rem; margin-top: 1.5rem; }
  .lane { min-width: 0; }
  .lane h3 { margin-top: 1rem; }
  ul { padding-left: 1.25rem; }
  li { margin: 0.5rem 0; }
  code, li, dd, summary { overflow-wrap: anywhere; }
  details { margin-top: 1rem; }
  .resolved details { margin: 0.35rem 0; color: var(--text-muted); }
  summary { cursor: pointer; }
  .legacy, .diagnostics, .catalog, .snapshot { margin-top: 1.5rem; }
  button { font-size: var(--font-size-body); line-height: var(--line-height-body); border-radius: var(--radius-soft); }
  button:hover:not(:disabled) { background: var(--interactive-hover); }
  button:active:not(:disabled) { background: var(--interactive-selected); }
  button:focus-visible, summary:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .namespace { padding: 1rem 0; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; font-size: var(--font-size-compact); }
  dd { margin: 0.25rem 0 0.75rem; }
  @media (max-width: 900px) { .lanes { grid-template-columns: minmax(0, 1fr); } }
</style>
