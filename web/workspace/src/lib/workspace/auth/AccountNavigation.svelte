<script lang="ts">
  import { onMount } from 'svelte';
  import { loadHostingAccess } from '#lib/workspace/auth/hosting.ts';
  let isAdmin = $state(false);
  // 親の確認済みsessionが変わるとこのcomponentも破棄する。Workspace所有から管理権限を推測しない。
  onMount(() => {
    let disposed = false;
    void loadHostingAccess().then((access) => { if (!disposed) isAdmin = access.is_admin; }).catch(() => {});
    return () => { disposed = true; };
  });
</script>

<nav aria-label="Your account">
  <a href="/account">Account</a>
  {#if isAdmin}<a href="/hosting/settings">Hosting settings</a>{/if}
</nav>
