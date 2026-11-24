<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { AdditionCodeError, redeemAdditionCode } from '#lib/workspace/auth/addition-code.ts';
  import { notifySessionChanged } from '#lib/workspace/auth/hosting.ts';
  import type { AuthUser } from '#lib/workspace/auth/model.ts';
  import '#lib/workspace/auth/auth-page.css';
  let handle = $state('');
  let code = $state('');
  let busy = $state(false);
  let error = $state('');
  let user = $state<AuthUser | null>(null);
  let origin = $state('');
  let controller: AbortController | null = null;
  let serial = 0;
  async function enroll() {
    if (busy || user || !code) return;
    const current = ++serial;
    const abort = new AbortController(); controller = abort;
    busy = true; error = '';
    const pending = redeemAdditionCode(handle, code, abort.signal); code = '';
    try {
      const confirmed = await pending;
      if (current !== serial) return;
      user = confirmed;
      notifySessionChanged();
    } catch (e) {
      if (current !== serial) return;
      error = e instanceof AdditionCodeError ? e.message : 'Registration could not be confirmed.';
    } finally { if (current === serial) { busy = false; controller = null; code = ''; } }
  }
  function cancel() {
    ++serial; controller?.abort(); controller = null; code = ''; busy = false;
    error = 'Stopped waiting; completion is not confirmed. If you saved a key, try passkey login first. Otherwise re-enter your ID and code to restart.';
  }
  onMount(() => {
    origin = window.location.origin;
    const clear = () => { ++serial; controller?.abort(); code = ''; busy = false; };
    window.addEventListener('pagehide', clear);
    return () => window.removeEventListener('pagehide', clear);
  });
  onDestroy(() => { ++serial; controller?.abort(); code = ''; });
</script>

<svelte:head><title>Add passkey with a code · Yoi</title></svelte:head>
<div class="auth-page" data-sveltekit-preload-data="off">
  <h1>Add passkey with a code</h1>
  {#if user}
    <p role="status">Passkey added to {user.handle}. You are signed in on this domain.</p>
    <p>Your existing keys and sessions are unchanged. This code is now consumed. Test normal ID-free login with the new key before retiring any old key.</p>
    <a href="/account">Open Account</a>
    <a href="/login" target="_blank" rel="noopener noreferrer">Open passkey login</a>
  {:else}
    <p>Use the addition code you generated from your Account on a domain where you can already sign in. This adds a key to that same account; it does not create or reactivate an account.</p>
    <p>Destination: <strong>{origin}</strong>. It must match the destination selected when you generated the code.</p>
    <form onsubmit={(event) => { event.preventDefault(); void enroll(); }} autocomplete="off">
      <label for="addition-handle">Account ID (handle)</label>
      <input id="addition-handle" bind:value={handle} required maxlength="64" autocomplete="off" autocapitalize="off" spellcheck="false" disabled={busy} />
      <label for="addition-code">Addition code</label>
      <input id="addition-code" type="password" bind:value={code} required maxlength="4096" autocomplete="off" autocapitalize="off" spellcheck="false" disabled={busy} />
      <p class="muted">Valid for 15 minutes and one successful addition. Keep it private; it authorizes a new login key. Use a device or provider that saves passkeys and verifies you with a PIN or biometrics.</p>
      <button type="submit" class="primary" disabled={busy || !handle.trim() || !code}>{busy ? 'Adding passkey…' : 'Add passkey to existing account'}</button>
      {#if busy}<button type="button" onclick={cancel}>Stop waiting</button>{/if}
    </form>
    {#if error}<p role="alert" class="error">{error}</p>{/if}
    <p class="muted">After cancellation, re-enter the ID and code. If completion is uncertain, try normal passkey login first. For an expired, revoked or replaced code, return to the original Account page to generate another. If this browser is signed in to another account, use a separate profile or private window.</p>
    <a href="/login">Already saved a key? Log in with a passkey</a>
  {/if}
</div>
<style>
  strong { overflow-wrap: anywhere; }
  form { display: grid; gap: var(--space-3); min-width: 0; }
</style>
