<script lang="ts">
  import './sidebar.css';

  type Props = {
    currentPath: string;
  };

  const { currentPath }: Props = $props();

  const items = [
    { href: '/account', label: 'Account' },
    { href: '/login/device', label: 'Device Login' },
  ];
</script>

<aside class="global-sidebar" aria-label="Global navigation">
  <div class="global-sidebar-section">
    <p class="sidebar-section-label">Global</p>
    <nav class="sidebar-list" aria-label="Global pages">
      {#each items as item}
        <a
          class="sidebar-link"
          class:active={currentPath === item.href}
          href={item.href}
          aria-current={currentPath === item.href ? 'page' : undefined}
        >
          <span>{item.label}</span>
        </a>
      {/each}
    </nav>
  </div>
</aside>
