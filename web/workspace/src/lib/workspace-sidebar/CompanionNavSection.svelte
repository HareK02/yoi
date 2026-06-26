<script lang="ts">
  type Props = {
    currentPath?: string;
  };

  let { currentPath = '/' }: Props = $props();

  const active = $derived(currentPath === '/console');
</script>

<section class="sidebar-section" aria-labelledby="companion-console-heading">
  <div class="section-heading-row">
    <h2 id="companion-console-heading">Console</h2>
    <span class="section-count">MVP</span>
  </div>
  <a class:active class="nav-item" href="/console" aria-current={active ? 'page' : undefined}>
    <span class="item-title">Companion Console</span>
    <span class="item-meta">status · transcript · send</span>
  </a>
</section>
