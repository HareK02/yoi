<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { addPasskey, logout } from '#lib/workspace/auth/api.ts';
  import { BrowserAuth } from '#lib/workspace/auth/browser-session.svelte.ts';
  import { loginUrl } from '#lib/workspace/auth/routes.ts';
  import '#lib/workspace/auth/auth-page.css';

  const auth = new BrowserAuth();
  async function refresh() {
    await auth.refresh();
    if (auth.session.kind === 'ready' && !auth.actor) {
      await goto(loginUrl('/account'), { replaceState: true });
    }
  }
  async function add() {
    if (!auth.actor) return;
    await auth.run('Waiting for passkey addition…', 'Passkey added to your account.', addPasskey);
  }
  async function signOut() {
    if (!auth.actor) return;
    const confirmed = await auth.run('Logging out…', 'Logged out.', logout);
    if (confirmed && !auth.actor) await goto(loginUrl('/account'), { replaceState: true });
  }
  onMount(() => { void refresh(); return () => auth.dispose(); });
</script>

<svelte:head><title>Account · Yoi</title></svelte:head>
<div class="auth-page" data-sveltekit-preload-data="off">
  <h1>Account</h1>
  {#if auth.session.kind === 'loading'}
    <p class="muted" role="status">Loading session…</p>
  {:else if auth.session.kind === 'error'}
    <p role="alert" class="error">Your session could not be loaded. Login status is unknown.</p>
    <button type="button" disabled={auth.busy} onclick={refresh}>Retry session</button>
  {:else if auth.actor}
    <dl>
      <div><dt>Account ID (handle)</dt><dd>{auth.actor.handle}</dd></div>
      <div><dt>Display name</dt><dd>{auth.actor.display_name}</dd></div>
    </dl>
    <section aria-label="Your Passkeys">
      <h2>Passkeys</h2>
      <button type="button" disabled={auth.busy} onclick={add}>Add passkey to {auth.actor.handle}</button>
    </section>
    <div class="actions">
      <button type="button" disabled={auth.busy} onclick={signOut}>Log out</button>
      <button type="button" disabled={auth.busy} onclick={refresh}>Refresh session</button>
    </div>
  {:else}
    <p>No browser session is active.</p>
    <a href={loginUrl('/account')}>Log in to your account</a>
  {/if}
  {#if auth.status}<p role="status">{auth.status}</p>{/if}
  {#if auth.error}<p role="alert" class="error">{auth.error}</p>{/if}
  <a href="/dashboard">Dashboard / Workspaces</a>
</div>
