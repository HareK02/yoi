<script lang="ts">
  export type DriveIconName = 'folder' | 'markdown' | 'text' | 'image' | 'download' | 'upload' | 'plus' | 'search' | 'refresh' | 'more' | 'close' | 'link' | 'edit' | 'trash' | 'arrow' | 'back' | 'info';
  let { name }: { name: DriveIconName } = $props();
  const paths: Record<DriveIconName, string> = {
    folder: 'M3 7V5h6l2 2h10v13H3Z',
    markdown: 'M5 3h10l4 4v14H5ZM14 3v5h5M8 16v-5l3 3 3-3v5',
    text: 'M5 3h10l4 4v14H5ZM14 3v5h5M8 12h8M8 16h6',
    image: 'M3 3h18v18H3ZM3 17l6-6 4 4 3-3 5 5M16 7h.01',
    download: 'M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5',
    upload: 'M12 16V4m-5 5 5-5 5 5M4 16v5h16v-5',
    plus: 'M12 5v14M5 12h14',
    search: 'M21 21l-6-6M17 10a7 7 0 1 1-14 0 7 7 0 0 1 14 0',
    refresh: 'M20 7v5h-5M4 17v-5h5M6 6a8 8 0 0 1 13 3M18 18A8 8 0 0 1 5 15',
    more: 'M5 12h.01M12 12h.01M19 12h.01',
    close: 'M6 6l12 12M6 18 18 6',
    link: 'm10 13 4-4M8 15l-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0m2 3 1-1a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0',
    edit: 'm15 4 5 5M4 20l5-1L21 7l-5-5L4 14Z',
    trash: 'M4 6h16M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7',
    arrow: 'm9 5 7 7-7 7',
    back: 'm15 5-7 7 7 7',
    info: 'M12 11v6M12 7h.01M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0',
  };
</script>

<svg aria-hidden="true" viewBox="0 0 24 24"><path d={paths[name]} /></svg>

<style>
  svg { width: 1.15em; height: 1.15em; flex: none; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
</style>
