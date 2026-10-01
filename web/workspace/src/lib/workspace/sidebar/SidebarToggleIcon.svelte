<script lang="ts">
  let { open }: { open: boolean } = $props();
</script>

<!-- Lucide panel-right-close / panel-right-open; ISC license: /licenses/lucide.txt -->
<svg
  xmlns="http://www.w3.org/2000/svg"
  width="18"
  height="18"
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  data-icon={open ? 'panel-right-close' : 'panel-right-open'}
>
  <rect width="18" height="18" x="3" y="3" rx="2" />
  <path d="M15 3v18" />
  <path d={open ? 'm8 9 3 3-3 3' : 'm10 15-3-3 3-3'} />
</svg>
