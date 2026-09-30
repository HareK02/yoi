<script lang="ts">
  let {
    lang = "",
    text,
  }: {
    lang?: string;
    text: string;
    codeBlockStyle?: "indented";
  } = $props();

  const label = $derived(lang ? `${lang} code block` : "Code block");
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex (horizontal scroll regions must be keyboard-focusable) -->
<div class="markdown-code-region" role="region" aria-label={label} tabindex="0">
  <pre><code>{text}</code></pre>
</div>
