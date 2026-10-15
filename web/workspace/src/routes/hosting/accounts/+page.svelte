<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { ActivationCodeResponse } from '#lib/generated/auth-api.ts';
  import { loadWhoami } from '#lib/workspace/auth/api.ts';
  import { HostingRequestError, loadHostingAccess, SESSION_CHANGED_EVENT } from '#lib/workspace/auth/hosting.ts';
  import { issueActivationCode, normalizeActivationHandle } from '#lib/workspace/auth/hosting-accounts.ts';

  let viewState = $state<'loading' | 'anonymous' | 'denied' | 'error' | 'ready'>('loading');
  let handle = $state('');
  let operation = $state<'create' | 'reissue'>('create');
  let issued = $state<ActivationCodeResponse | null>(null);
  let busy = $state(false);
  let error = $state('');
  let generation = 0;
  let controller = new AbortController();
  let expiryTimer: ReturnType<typeof setTimeout> | undefined;
  function clearIssued() { issued = null; clearTimeout(expiryTimer); }
  async function load() {
    const current = ++generation;
    controller.abort(); controller = new AbortController();
    clearIssued(); busy = false; viewState = 'loading'; error = '';
    try {
      const { actor } = await loadWhoami();
      if (current !== generation) return;
      if (!actor) { viewState = 'anonymous'; return; }
      const access = await loadHostingAccess();
      if (current !== generation) return;
      viewState = access.is_admin ? 'ready' : 'denied';
    } catch { if (current === generation) viewState = 'error'; }
  }
  async function issue() {
    if (busy || viewState !== 'ready') return;
    clearIssued(); error = '';
    try { handle = normalizeActivationHandle(handle); }
    catch { error = 'Use 1–64 ASCII letters, numbers, hyphens or underscores for the account ID.'; return; }
    const current = generation;
    busy = true;
    try {
      // 表示済みaccessを永続権限と見なさず、操作直前にもlive確認する。
      const access = await loadHostingAccess();
      if (current !== generation) return;
      if (!access.is_admin) { viewState = 'denied'; return; }
      const result = await issueActivationCode(handle, operation === 'reissue', controller.signal);
      if (current !== generation) return;
      issued = result;
      expiryTimer = setTimeout(clearIssued, Math.max(0, Math.min(900000, Date.parse(result.expires_at) - Date.now())));
    } catch (err) {
      if (current !== generation) return;
      if (err instanceof HostingRequestError && (err.status === 401 || err.status === 403)) {
        viewState = err.status === 401 ? 'anonymous' : 'denied';
      } else {
        error = 'Code issuance could not be confirmed. Do not repeat account creation automatically. If the recipient completed registration, check Passkey login first. Otherwise verify the ID and explicitly reissue a code for the pending account. Active accounts cannot be reissued.';
      }
    } finally { if (current === generation) busy = false; }
  }
  onMount(() => {
    void load();
    const refresh = () => { void load(); };
    const clearPage = () => { ++generation; controller.abort(); clearIssued(); busy = false; };
    const restorePage = (event: PageTransitionEvent) => { if (event.persisted) void load(); };
    window.addEventListener(SESSION_CHANGED_EVENT, refresh);
    window.addEventListener('pagehide', clearPage);
    window.addEventListener('pageshow', restorePage);
    return () => { window.removeEventListener(SESSION_CHANGED_EVENT, refresh); window.removeEventListener('pagehide', clearPage); window.removeEventListener('pageshow', restorePage); };
  });
  onDestroy(() => { ++generation; controller.abort(); clearIssued(); });
</script>

<svelte:head><title>Hosting accounts · Yoi</title></svelte:head>
<div class="accounts" data-sveltekit-preload-data="off">
  <h1>Hosting accounts</h1>
  {#if viewState === 'loading'}
    <p role="status">Checking hosting access…</p>
  {:else if viewState === 'anonymous'}
    <p>Sign in with a hosting administrator account to issue activation codes.</p>
    <a href="/account">Open Account</a>
  {:else if viewState === 'denied'}
    <p>Hosting administrator access is required.</p>
  {:else if viewState === 'error'}
    <p role="alert" class="error">Hosting access could not be checked.</p>
    <button type="button" onclick={load}>Retry access check</button>
  {:else}
    <p>Create a pending, ordinary user with an ID only. No email or existing Passkey is needed, even when self-registration is disabled.</p>
    {#if issued}
      <section aria-labelledby="issued-heading">
        <h2 id="issued-heading">Activation code issued</h2>
        <dl><dt>Account ID (handle)</dt><dd>{issued.user.handle}</dd><dt>Expires</dt><dd>{new Date(issued.expires_at).toLocaleString()}</dd><dt>One-time code</dt><dd class="secret">{issued.code}</dd></dl>
        <p>This code is shown only here and disappears when hidden, expired or you leave. Privately give it and the ID to the recipient. Do not put it in a URL, screenshot, log or support message.</p>
        <p>Ask the recipient to open <a href="/activate">/activate</a> on this Server and register a Passkey within 15 minutes. The pending account is not active until registration completes.</p>
        <button type="button" onclick={clearIssued}>Hide code</button>
      </section>
    {:else}
      <form onsubmit={(event) => { event.preventDefault(); void issue(); }} autocomplete="off">
        <label for="account-operation">Operation</label>
        <select id="account-operation" bind:value={operation} disabled={busy}><option value="create">Create account and issue code</option><option value="reissue">Reissue for a pending account</option></select>
        <label for="account-handle">Account ID (handle)</label>
        <input id="account-handle" bind:value={handle} required maxlength="128" autocomplete="off" autocapitalize="off" spellcheck="false" disabled={busy} aria-describedby="handle-help" />
        <p id="handle-help" class="muted">1–64 ASCII letters, numbers, hyphens or underscores. Surrounding spaces are removed and letters become lowercase.</p>
        {#if operation === 'reissue'}<p>Reissuing invalidates the old code and any in-progress registration. It cannot replace a Passkey or recover an active account.</p>{/if}
        <button class="primary" type="submit" disabled={busy || !handle.trim()}>{busy ? 'Issuing code…' : operation === 'create' ? 'Create account and issue code' : 'Reissue activation code'}</button>
      </form>
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <details><summary>Issuance and recovery steps</summary><p>Create a new ID, deliver the code privately, then ask the recipient to register at /activate. Codes expire after 15 minutes or five incorrect attempts. Options alone do not consume a correct code. After a cancelled prompt or reload, re-enter ID and code. If expired, locked or lost, choose reissue for the same pending ID and deliver only the new code. Ask the recipient to check Passkey login first if completion was uncertain. After activation use Passkey login, not the code.</p></details>
    <a href="/hosting/settings">Hosting settings</a>
  {/if}
</div>

<style>
  .accounts, section, form, details[open] { display: grid; align-content: start; gap: var(--space-4); min-width: 0; }
  .accounts { max-width: 44rem; }
  h1, h2, p, dl, dd { margin: 0; }
  h1 { font-size: var(--font-size-title); line-height: var(--line-height-title); }
  h2 { font-size: var(--font-size-body); line-height: var(--line-height-body); }
  dl { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: var(--space-2) var(--space-4); }
  dd { overflow-wrap: anywhere; font-family: var(--font-mono); }
  .muted { color: var(--text-muted); }
  .error { color: var(--danger); }
  input, select { width: 100%; min-width: 0; padding: var(--space-2); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: var(--bg); color: var(--text-strong); font: inherit; }
  button { justify-self: start; min-height: 2.25rem; padding: var(--space-2) var(--space-3); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: transparent; color: var(--text-strong); font: inherit; cursor: pointer; }
  button.primary { background: var(--text-strong); color: var(--bg); }
  button:hover:not(:disabled) { background: var(--interactive-hover); color: var(--text-strong); }
  button:disabled { opacity: .55; cursor: not-allowed; }
  a { color: var(--text-strong); text-decoration: underline; text-underline-offset: .15em; }
  a:hover, a:active { color: var(--text); }
  a:focus-visible, button:focus-visible, input:focus-visible, select:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
