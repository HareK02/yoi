<script lang="ts">
  import { onMount } from 'svelte';
  import { loadRegistrationPolicy, SESSION_CHANGED_EVENT } from './hosting.ts';
  import { authEntryUrl, AUTHENTICATED_HOME } from './routes.ts';

  let { returnTo = AUTHENTICATED_HOME }: { returnTo?: string } = $props();
  let policy = $state<'loading' | 'error' | 'enabled' | 'disabled'>('loading');
  let request = 0;
  async function refresh() {
    const current = ++request;
    policy = 'loading';
    try {
      const result = await loadRegistrationPolicy();
      if (current === request) policy = result.self_registration_enabled ? 'enabled' : 'disabled';
    } catch {
      if (current === request) policy = 'error';
    }
  }
  onMount(() => {
    void refresh();
    window.addEventListener('focus', refresh);
    window.addEventListener(SESSION_CHANGED_EVENT, refresh);
    return () => {
      ++request;
      window.removeEventListener('focus', refresh);
      window.removeEventListener(SESSION_CHANGED_EVENT, refresh);
    };
  });
</script>

{#if policy === 'enabled'}
  <a href={authEntryUrl('/register', returnTo)}>Create a new account</a>
{:else if policy === 'disabled'}
  <p class="muted">New account registration is disabled. Existing users can still log in.</p>
{:else if policy === 'loading'}
  <p class="muted" role="status">Checking registration availability…</p>
{:else}
  <p class="muted">Registration availability is unknown. Existing users can still log in.</p>
  <button type="button" onclick={refresh}>Retry registration availability</button>
{/if}
