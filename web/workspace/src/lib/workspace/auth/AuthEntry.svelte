<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { loginWithPasskey, registerPasskey } from './api.ts';
  import { BrowserAuth } from './browser-session.svelte.ts';
  import { loadRegistrationPolicy } from './hosting.ts';
  import { authEntryUrl, safeReturnTo } from './routes.ts';
  import './auth-page.css';

  let { mode }: { mode: 'login' | 'register' } = $props();
  const auth = new BrowserAuth();
  let handle = $state('');
  let displayName = $state('');
  let policy = $state<boolean | null>(null);
  let policyLoading = $state(true);
  let policyError = $state('');
  let policyRequest = 0;
  const destination = $derived(safeReturnTo(page.url.searchParams.get('returnTo')));

  async function refreshPolicy() {
    if (mode !== 'register') return;
    const request = ++policyRequest;
    policyLoading = true; policy = null; policyError = '';
    try {
      const result = await loadRegistrationPolicy();
      if (request === policyRequest) policy = result.self_registration_enabled;
    } catch {
      if (request === policyRequest) policyError = 'Registration availability could not be loaded.';
    } finally {
      if (request === policyRequest) policyLoading = false;
    }
  }

  async function submit() {
    if (auth.busy || auth.session.kind !== 'ready' || auth.actor || !handle.trim() ||
        (mode === 'register' && (policy !== true || !displayName.trim()))) return;
    const registering = mode === 'register';
    const id = handle.trim();
    const name = displayName.trim();
    const confirmed = await auth.run(
      registering ? 'Waiting for new account registration…' : 'Waiting for passkey login…',
      registering ? 'New account registered.' : 'Logged in.',
      (signal) => registering ? registerPasskey(id, name, signal) : loginWithPasskey(id, signal),
    );
    if (confirmed && auth.actor) {
      await goto(destination, { replaceState: true });
    } else {
      if (confirmed) {
        auth.status = '';
        auth.error = 'A normal browser session was not confirmed. Log in again.';
      }
      if (registering) void refreshPolicy();
    }
  }

  onMount(() => {
    void auth.refresh();
    void refreshPolicy();
    window.addEventListener('focus', refreshPolicy);
    return () => { ++policyRequest; auth.dispose(); window.removeEventListener('focus', refreshPolicy); };
  });
</script>

<svelte:head><title>{mode === 'login' ? 'Log in' : 'Create account'} · Yoi</title></svelte:head>
<div class="auth-page" data-sveltekit-preload-data="off">
  <h1>{mode === 'login' ? 'Log in' : 'Create account'}</h1>
  {#if auth.session.kind === 'loading'}
    <p role="status" class="muted">Loading session…</p>
  {:else if auth.session.kind === 'error'}
    <p role="alert" class="error">Your session could not be loaded. Login status is unknown.</p>
    <button type="button" disabled={auth.busy} onclick={() => auth.refresh()}>Retry session</button>
  {:else if auth.actor}
    <p>Logged in as <strong>{auth.actor.display_name}</strong> <span class="muted">@{auth.actor.handle}</span>.</p>
    <a href={destination}>Continue to {destination === '/' ? 'dashboard / Workspaces' : 'requested page'}</a>
    <a href="/account">Open Account</a>
  {:else}
    {#if mode === 'register'}
      {#if policyLoading}
        <p role="status" class="muted">Checking registration availability…</p>
      {:else if policyError}
        <p role="alert" class="error">{policyError}</p>
        <button type="button" onclick={refreshPolicy}>Retry registration availability</button>
      {:else if policy === false}
        <p>New account registration is disabled. Existing users can still log in.</p>
      {/if}
    {/if}
    {#if mode === 'login' || policy === true}
      <form onsubmit={(event) => { event.preventDefault(); void submit(); }}>
        <label>Account ID (handle)
          <input bind:value={handle} required maxlength="128" autocomplete="username webauthn" autocapitalize="off" spellcheck="false" disabled={auth.busy} aria-describedby="id-help" />
        </label>
        <p id="id-help" class="muted">{mode === 'login' ? 'Use your account ID, not your display name.' : 'Your unique sign-in ID. This is separate from your display name.'}</p>
        {#if mode === 'register'}
          <label>Display name
            <input bind:value={displayName} required maxlength="128" autocomplete="name" disabled={auth.busy} />
          </label>
        {/if}
        <button class="primary" type="submit" disabled={auth.busy || !handle.trim() || (mode === 'register' && !displayName.trim())}>{mode === 'login' ? 'Log in with passkey' : 'Register new account'}</button>
      </form>
    {/if}
  {/if}
  {#if auth.status}<p role="status">{auth.status}</p>{/if}
  {#if auth.error}<p role="alert" class="error">{auth.error}</p>{/if}
  <nav aria-label="Authentication">
    {#if mode === 'login'}
      <a href={authEntryUrl('/register', destination)}>Create a new account</a>
    {:else}
      <a href={authEntryUrl('/login', destination)}>Log in with an existing account</a>
    {/if}
    <a href="/activate">Activate an account with a code</a>
    <a href="/">Dashboard / Workspaces</a>
  </nav>
</div>
