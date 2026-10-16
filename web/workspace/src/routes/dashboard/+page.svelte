<script lang="ts">
  import { onMount } from 'svelte';
  import { BrowserAuth } from '#lib/workspace/auth/browser-session.svelte.ts';
  import { SESSION_CHANGED_EVENT } from '#lib/workspace/auth/hosting.ts';
  import { loginUrl } from '#lib/workspace/auth/routes.ts';
  import AccountNavigation from '#lib/workspace/auth/AccountNavigation.svelte';
  import WorkspaceCatalog from './WorkspaceCatalog.svelte';
  import { WorkspaceCreation } from './workspace-creation.svelte.ts';
  import '#lib/workspace/auth/auth-page.css';

  const auth = new BrowserAuth();
  let creation = $state<WorkspaceCreation | null>(null);
  let mounted = false;
  $effect(() => {
    if (auth.session.kind !== 'ready') return;
    const actor = auth.session.actor;
    if (!actor) { creation = null; return; }
    if (creation?.accountId !== actor.account_id) {
      const draft: WorkspaceCreation = new WorkspaceCreation(actor.account_id, () =>
        mounted && creation === draft && auth.actor?.account_id === draft.accountId);
      creation = draft;
    }
  });
  onMount(() => {
    mounted = true;
    const refresh = () => { void auth.refresh(); };
    refresh();
    window.addEventListener('focus', refresh);
    window.addEventListener(SESSION_CHANGED_EVENT, refresh);
    return () => {
      mounted = false;
      auth.dispose();
      window.removeEventListener('focus', refresh);
      window.removeEventListener(SESSION_CHANGED_EVENT, refresh);
    };
  });
</script>

<svelte:head><title>Dashboard · Yoi</title></svelte:head>
<div class="auth-page" data-sveltekit-preload-data="off">
  <header><a href="/">Yoi</a><h1>Dashboard</h1></header>
  {#if auth.session.kind === 'loading'}
    <p class="muted" role="status">Loading session…</p>
  {:else if auth.session.kind === 'error'}
    <p class="error" role="alert">Your session could not be loaded. Login status is unknown.</p>
    <button type="button" onclick={() => auth.refresh()}>Retry session</button>
    <a href={loginUrl('/dashboard')}>Open login</a>
  {:else if auth.actor}
    {#key auth.actor.account_id}
      <AccountNavigation />
      {#if creation?.accountId === auth.actor.account_id}
        <WorkspaceCatalog {creation} />
      {/if}
    {/key}
  {:else}
    <p>Log in to select or create a Workspace.</p>
    <a class="entry-primary" href={loginUrl('/dashboard')}>Log in</a>
  {/if}
</div>

<style>header { display: grid; gap: var(--space-4); }</style>
