<script lang="ts">
  import { observeDeferred } from './observe-deferred.svelte.ts';
  import type { DeferredResult } from './deferred.ts';

  let { identity, read }: {
    identity: string | undefined;
    read: () => Promise<DeferredResult<string>>;
  } = $props();
  const loaded = observeDeferred(() => read(), () => identity);
</script>

<p data-testid="failure">{loaded.failure?.message ?? 'no failure'}</p>
<p data-testid="data">{loaded.data ?? 'no data'}</p>
<p data-testid="loading">{String(loaded.loading)}</p>
