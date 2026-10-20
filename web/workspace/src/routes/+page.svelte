<script lang="ts">
  import { onMount } from 'svelte';
  import { BrowserAuth } from '#lib/workspace/auth/browser-session.svelte.ts';
  import { AUTHENTICATED_HOME } from '#lib/workspace/auth/routes.ts';
  import RegistrationLink from '#lib/workspace/auth/RegistrationLink.svelte';
  import AccountNavigation from '#lib/workspace/auth/AccountNavigation.svelte';
  import '#lib/workspace/auth/auth-page.css';

  const auth = new BrowserAuth();
  onMount(() => {
    void auth.refresh();
    return () => auth.dispose();
  });
</script>

<svelte:head><title>Yoi · AIと進める開発</title></svelte:head>
<div class="auth-page public-home" data-sveltekit-preload-data="off">
  <h1>Yoi</h1>
  <p>AIとともに、開発を前へ。</p>
  <p class="muted">Ticketで目的を共有し、AI Workerと実装を進める。Workspaceでコード・レビュー・作業の記録をまとめます。</p>
  <section aria-label="Get started">
    {#if auth.session.kind === 'loading'}
      <p class="muted" role="status">Loading session…</p>
    {:else if auth.session.kind === 'error'}
      <p class="error" role="alert">Your session could not be loaded. Login status is unknown.</p>
      <button type="button" onclick={() => auth.refresh()}>Retry session</button>
      <a href="/login">Open login</a>
    {:else if auth.actor}
      <p>Logged in as <strong>{auth.actor.display_name}</strong>.</p>
      <a class="entry-primary" href={AUTHENTICATED_HOME}>Open dashboard</a>
      {#key auth.actor.account_id}<AccountNavigation />{/key}
    {:else}
      <a class="entry-primary" href="/login">Log in</a>
      <RegistrationLink />
      <a href="/activate">Activate an account with a code</a>
    {/if}
  </section>
</div>

<style>
  .public-home { padding-block: var(--space-6); }
  .public-home > p { max-width: 40rem; }
</style>
