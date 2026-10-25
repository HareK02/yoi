<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { approveDeviceLogin } from '#lib/workspace/auth/api.ts';
  import { BrowserAuth } from '#lib/workspace/auth/browser-session.svelte.ts';
  import { loginUrl } from '#lib/workspace/auth/routes.ts';
  import '#lib/workspace/auth/auth-page.css';

  const auth = new BrowserAuth();
  let userCode = $state('');
  let approved = $state(false);
  const login = $derived(loginUrl(`/login/device?${new URLSearchParams({ user_code: userCode.trim() })}`));
  async function approve() {
    if (auth.busy || !auth.actor || approved || !userCode.trim()) return;
    const code = userCode.trim().toUpperCase();
    const confirmed = await auth.run('Approving device login…', 'Device login approved. You can return to the CLI.', async (signal) => {
      const result = await approveDeviceLogin(code, signal);
      if (result.status !== 'approved') throw new Error('Device approval not confirmed');
      return result;
    }, 'device-approval');
    if (confirmed) approved = true;
  }
  onMount(() => {
    userCode = page.url.searchParams.get('user_code') ?? page.url.searchParams.get('code') ?? '';
    void auth.refresh();
    return () => auth.dispose();
  });
</script>

<svelte:head><title>Device Login · Yoi</title></svelte:head>
<div class="auth-page" data-sveltekit-preload-data="off">
  <h1>Approve CLI login</h1>
  <p>Only approve a code shown by a CLI login you started. Logging in to the browser does not approve the device.</p>
  {#if auth.session.kind === 'loading'}
    <p class="muted" role="status">Loading session…</p>
  {:else if auth.session.kind === 'error'}
    <p class="error" role="alert">Your session could not be loaded. Login status is unknown.</p>
    <button type="button" disabled={auth.busy} onclick={() => auth.refresh()}>Retry session</button>
  {:else if auth.actor}
    <p>Logged in as <strong>{auth.actor.display_name}</strong> <span class="muted">@{auth.actor.handle}</span>.</p>
  {:else}
    <p>You need to log in before approving a device login.</p>
    <a href={login}>Log in to continue</a>
  {/if}
  <form onsubmit={(event) => { event.preventDefault(); void approve(); }}>
    <label>User code
      <input bind:value={userCode} required maxlength="64" placeholder="ABCD-EFGH" autocapitalize="characters" disabled={auth.busy || approved} />
    </label>
    {#if auth.actor && !approved}
      <button type="submit" disabled={auth.busy || !userCode.trim()}>Approve device login</button>
    {/if}
  </form>
  {#if auth.status}<p role="status">{auth.status}</p>{/if}
  {#if auth.error}<p class="error" role="alert">{auth.error}</p>{/if}
  <a href="/account">Open Account</a>
</div>
