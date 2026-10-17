<script lang="ts">
  import type { SourceStatus } from './journal-api.ts';
  let { source }: { source: SourceStatus } = $props();
</script>
{#if source.state === 'missing'}
  <p>元の会話は削除済み</p>
{:else if source.history_href}
  <p>原資料の本文は未確認です。</p>
  <p><a href={source.history_href}>元の会話を確認する →</a></p>
{:else}
  <p>原資料は取得不能（存在・閲覧可否を確認できません）</p>
{/if}
<style>p { margin: 0; color: var(--text-muted); overflow-wrap: anywhere; }</style>
