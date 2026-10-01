<script lang="ts">
  import SvelteMarkdown, {
    buildUnsupportedHTML,
    type Renderers,
    type SvelteMarkdownProps,
  } from "@humanspeak/svelte-markdown";
  import MarkdownLink from "#lib/workspace/markdown/MarkdownLink.svelte";

  type Props = {
    text: string;
    streamId?: string | number;
    class?: string;
    streaming?: boolean;
    renderers?: Partial<Renderers>;
  };

  let {
    text,
    streamId = "static",
    class: className = "",
    streaming = false,
    renderers = {},
  }: Props = $props();

  const options = {
    breaks: true,
    gfm: true,
  } satisfies NonNullable<SvelteMarkdownProps["options"]>;

  const safeRenderers = $derived({
    ...renderers,
    html: buildUnsupportedHTML(),
    link: MarkdownLink,
  } satisfies Partial<Renderers>);
</script>

<div class={className}>
  <SvelteMarkdown source={text} {streamId} {options} renderers={safeRenderers} {streaming} />
</div>
