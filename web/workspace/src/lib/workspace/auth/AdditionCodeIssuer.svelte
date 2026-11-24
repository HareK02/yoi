<script lang="ts">
  import { onMount } from 'svelte';
  import type { AuthUser } from './model.ts';
  import { AdditionCodeError, issueAdditionCode, loadAdditionCodeStatus, revokeAdditionCode, type CodeStatus, type IssuedCode } from './addition-code.ts';
  let { owner, disabled = false, verify }: { owner: AuthUser; disabled?: boolean; verify: () => Promise<void> } = $props();
  let codeStatus = $state<CodeStatus | null>(null);
  let issued = $state<IssuedCode | null>(null);
  let destination = $state('');
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let show = $state(false);
  let needsVerification = $state(false);
  let controller: AbortController | null = null;
  let serial = 0;
  async function run(action: 'load' | 'issue' | 'revoke') {
    if (busy || (disabled && action !== 'load')) return;
    const current = ++serial;
    const abort = new AbortController(); controller = abort;
    busy = true; error = ''; message = ''; issued = null; show = false;
    try {
      if (action === 'load') {
        codeStatus = await loadAdditionCodeStatus(owner, abort.signal);
        if (!codeStatus.public_urls.includes(destination)) destination = '';
      } else if (action === 'issue') {
        const result = await issueAdditionCode(owner, destination, abort.signal);
        if (current !== serial) return;
        issued = result;
        if (codeStatus) codeStatus = { ...codeStatus, active: { issuance_id: result.issuance_id, target_origin: result.target_origin, expires_at: result.expires_at } };
        needsVerification = false;
      } else if (codeStatus?.active) {
        const revoked = await revokeAdditionCode(owner, codeStatus.active.issuance_id, abort.signal);
        if (current !== serial) return;
        codeStatus = await loadAdditionCodeStatus(owner, abort.signal);
        message = revoked ? 'Code revoked. Existing passkeys and sessions are unchanged.' : 'That code was no longer active. Current status refreshed.';
      }
    } catch (e) {
      if (current !== serial) return;
      error = e instanceof AdditionCodeError ? e.message : 'The result could not be confirmed. Refresh code status before continuing.';
      needsVerification = e instanceof AdditionCodeError && e.kind === 'recent-auth';
      codeStatus = null;
    } finally { if (current === serial) { busy = false; controller = null; } }
  }
  function clear() { ++serial; controller?.abort(); controller = null; issued = null; show = false; busy = false; codeStatus = null; }
  async function verifyIdentity() { clear(); await verify(); }
  onMount(() => {
    void run('load');
    const hide = () => clear();
    window.addEventListener('pagehide', hide);
    return () => { clear(); window.removeEventListener('pagehide', hide); };
  });
</script>

<details class="code-issuer">
  <summary>Add a passkey on another domain or device</summary>
  <div class="contents">
    <p>This code lets its holder add a login key to <strong>{owner.handle}</strong>. Use it only yourself, on the selected destination. Existing keys and sessions stay active.</p>
    {#if error}<p role="alert" class="error">{error}</p>{/if}
    {#if message}<p role="status">{message}</p>{/if}
    {#if busy}<p role="status">Checking code operation…</p>{/if}
    {#if needsVerification || !issued}
      <button type="button" disabled={busy || disabled} onclick={verifyIdentity}>Verify identity with passkey</button>
    {/if}
    {#if codeStatus}
      <form onsubmit={(event) => { event.preventDefault(); void run('issue'); }}>
        <label for="addition-destination">Destination URL</label>
        <select id="addition-destination" bind:value={destination} disabled={busy || disabled} required>
          <option value="" disabled>Select the domain where you will save the passkey</option>
          {#each codeStatus.public_urls as url}<option value={url}>{url}</option>{/each}
        </select>
        {#if codeStatus.active}
          <p>Active code for <strong>{codeStatus.active.target_origin}</strong>, expires {new Date(codeStatus.active.expires_at).toLocaleString()}. Generating another code invalidates the old code and any unfinished registration.</p>
        {/if}
        <button type="submit" disabled={busy || disabled || !destination}>{codeStatus.active ? 'Generate replacement code' : 'Generate addition code'}</button>
      </form>
      {#if codeStatus.active}
        <button type="button" disabled={busy || disabled} onclick={() => run('revoke')}>Revoke active code</button>
      {/if}
    {/if}
    {#if issued}
      <div class="issued" role="status">
        <label for="addition-code-issued">One-time addition code</label>
        <input id="addition-code-issued" type={show ? 'text' : 'password'} value={issued.code} readonly autocomplete="off" spellcheck="false" />
        <button type="button" onclick={() => show = !show}>{show ? 'Hide code' : 'Show code'}</button>
        <p>Shown only here until you refresh or leave. Expires {new Date(issued.expires_at).toLocaleString()}.</p>
        <p>Open <a href={`${issued.target_origin}/account/add-passkey`} target="_blank" rel="noopener noreferrer">{issued.target_origin}/account/add-passkey</a> and enter account ID <strong>{owner.handle}</strong> and this code. The link does not contain the code.</p>
        <p>After saving the new key, test normal ID-free login on that domain before retiring any old key.</p>
      </div>
    {/if}
    <button type="button" disabled={busy || disabled} onclick={() => run('load')}>Refresh code status</button>
    <p class="muted">If an operation’s result is uncertain, refresh status instead of retrying automatically. The code cannot be recovered; explicitly generate a replacement if needed.</p>
  </div>
</details>

<style>
  .contents, form, .issued { display: grid; gap: var(--space-3); min-width: 0; }
  .contents { padding-block-start: var(--space-4); }
  p { margin: 0; overflow-wrap: anywhere; }
  .muted { color: var(--text-muted); }
  .error { color: var(--danger); }
  input, select { width: 100%; min-width: 0; padding: var(--space-2); border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: var(--bg); color: var(--text-strong); font: inherit; }
  input { font-family: var(--font-mono); }
  button { justify-self: start; padding: var(--space-2) var(--space-3); min-height: 2.25rem; border: 1px solid var(--line-strong); border-radius: var(--radius-soft); background: transparent; color: var(--text-strong); font: inherit; cursor: pointer; }
  button:hover:not(:disabled), button:active:not(:disabled) { background: var(--interactive-hover); }
  button:disabled { opacity: .55; cursor: not-allowed; }
  a { color: var(--text-strong); overflow-wrap: anywhere; }
  a:hover, a:active { color: var(--text); }
  button:focus-visible, input:focus-visible, select:focus-visible, summary:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
