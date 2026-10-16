<script lang="ts">
  import { onMount } from 'svelte';
  import { BrowserAuth } from '#lib/workspace/auth/browser-session.svelte.ts';
  import { SESSION_CHANGED_EVENT } from '#lib/workspace/auth/hosting.ts';
  import { loginUrl } from '#lib/workspace/auth/routes.ts';
  import AccountNavigation from '#lib/workspace/auth/AccountNavigation.svelte';
  import WorkspaceCatalog from './WorkspaceCatalog.svelte';
  import '#lib/workspace/auth/auth-page.css';

  const auth = new BrowserAuth();
  onMount(() => {
    const refresh = () => { void auth.refresh(); };
    refresh();
    window.addEventListener('focus', refresh);
    window.addEventListener(SESSION_CHANGED_EVENT, refresh);
    return () => {
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
      <WorkspaceCatalog />
    {/key}
  {:else}
    <p>Log in to select or create a Workspace.</p>
    <a class="entry-primary" href={loginUrl('/dashboard')}>Log in</a>
  {/if}
</div>

<style>header { display: grid; gap: var(--space-4); }</style>
