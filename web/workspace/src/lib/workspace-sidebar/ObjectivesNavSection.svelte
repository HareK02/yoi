<script lang="ts">
  type Props = {
    currentPath?: string;
  };

  let { currentPath = '/' }: Props = $props();
</script>

<section class="nav-section">
  <header class="section-header">
    <span>Objectives</span>
  </header>

  <a class="objective-link" class:active={currentPath.startsWith('/objectives')} href="/objectives">
    <span class="item-title">Open Objectives</span>
    <span class="item-meta">workspace objectives</span>
  </a>
</section>
