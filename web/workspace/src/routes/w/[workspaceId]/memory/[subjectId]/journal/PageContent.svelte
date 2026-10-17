<script lang="ts">
  import DocumentMarkdown from '#lib/workspace/markdown/DocumentMarkdown.svelte';
  import OriginalSource from '#lib/workspace/memory/OriginalSource.svelte';
  import { journalHref, type JournalEntry, type TextPage } from '#lib/workspace/memory/journal-api.ts';
  import { formatDate, workspaceRoute, type ApiResult } from '#lib/workspace/api/http.ts';
  import type { ResolvedPageData } from '#lib/workspace/navigation/deferred.ts';
  import type { PageData as RouteData } from './$types';
  let { data }: { data: ResolvedPageData<RouteData> } = $props();
  const detail = $derived(data.detail);
  const record = $derived(detail?.entry.data);
  const entry = $derived(record?.entry);
  const base = $derived(journalHref(data.workspaceId, data.subjectId));
  function href(values: Record<string, string | null | undefined>) {
    const query = new URLSearchParams(data.urlQuery);
    for (const [key, value] of Object.entries(values)) value == null ? query.delete(key) : query.set(key, value);
    return `${base}?${query}`;
  }
  function entryHref(e: JournalEntry) { return journalHref(data.workspaceId, data.subjectId, { journal: e.journal_id, entry: e.id }); }
  function nextText(key: string, page: TextPage) { return href({ [`${key}_byte`]: String(page.next_byte), [`${key}_token`]: page.content_token }); }
  function memoryHref(memoryId: string, changeId: string) {
    return workspaceRoute(data.workspaceId, `/memory/${encodeURIComponent(data.subjectId)}/${encodeURIComponent(memoryId)}?change_id=${encodeURIComponent(changeId)}`);
  }
</script>
<svelte:head><title>{entry?.title ?? 'Journal'} · Yoi Workspace</title></svelte:head>

{#snippet textSection(title: string, key: string, result: ApiResult<TextPage> | null | undefined)}
  <section class="text-section" aria-label={title}>
    <h2>{title}</h2>
    {#if result?.data}
      {#if result.data.text.trim()}<DocumentMarkdown text={result.data.text} />{:else}<p class="muted">{title}は空です。</p>{/if}
      {#if result.data.next_byte !== null}<a href={nextText(key, result.data)}>次の本文区間 →</a>{/if}
      {#if new URLSearchParams(data.urlQuery).has(`${key}_byte`)}<a href={href({ [`${key}_byte`]: null, [`${key}_token`]: null })}>本文の先頭へ</a>{/if}
    {:else if result?.error}
      <div role="alert" class="error"><p>{title}を取得できません。{result.error}</p><a href={href({ [`${key}_byte`]: null, [`${key}_token`]: null })}>先頭から読み直す</a></div>
    {/if}
  </section>
{/snippet}
{#snippet entryList(items: JournalEntry[])}
  <ol class="entry-list">
    {#each items as item (`${item.journal_id}:${item.id}`)}
      <li>
        <a href={entryHref(item)}><time datetime={item.created_at}>{formatDate(item.created_at)}</time><strong>{item.title}</strong><span aria-hidden="true">→</span></a>
        <p class="muted">{item.state} · 出来事 {item.position}{#if item.correction_refs.length} · 訂正あり{/if}</p>
      </li>
    {/each}
  </ol>
{/snippet}

<section class="journal-page" aria-labelledby="journal-title" data-journal-view={data.mode}>
  <a href={workspaceRoute(data.workspaceId, `/memory/${encodeURIComponent(data.subjectId)}`)}>← SubjectのMemory</a>
  <header><h1 id="journal-title">{entry?.title ?? 'Journal'}</h1>{#if entry}<p>{entry.state} · 出来事 {entry.position} · <time datetime={entry.created_at}>{formatDate(entry.created_at)}</time></p>{/if}</header>
  <p class="muted">Journalは活動の経緯を記録した文章です。原発言・tool結果そのものではなく、Memoryの直接の抽出元や主張の証明ではありません。</p>
  {#if data.mode === 'detail'}
    {#if record && entry && detail}
      <nav aria-label="Journal内の移動"><a href={journalHref(data.workspaceId, data.subjectId, { journal: entry.journal_id })}>{record.journal.title}の経緯 →</a><a href={base}>最近の出来事 →</a></nav>
      {@render textSection('出来事の本文', 'body', detail.body)}
      {#if entry.state === 'open'}{@render textSection('継続中の経緯', 'ongoing', detail.ongoing)}{/if}
      <section aria-labelledby="correction-title"><h2 id="correction-title">訂正と対象</h2>
        {#if entry.correction_refs.length === 0}<p class="muted">この出来事からの訂正対象はありません。</p>{/if}
        {#each detail.targets as target}
          <p>訂正対象: <a href={entryHref(target)}>{target.title} →</a></p>
        {/each}
        {#if detail.targetNext !== null}<a href={href({ target_offset: String(detail.targetNext) })}>すべて見る（訂正対象の続き） →</a>{/if}
        {#if detail.corrections?.data}
          {#if detail.corrections.data.items.length}<h3>この出来事を訂正する記録</h3>{@render entryList(detail.corrections.data.items)}{:else}<p class="muted">{new URLSearchParams(data.urlQuery).has('corrections_after') ? 'このページに訂正はありません。' : 'この出来事への訂正はありません。'}</p>{/if}
          {#if detail.corrections.data.next}<a href={href({ corrections_after: detail.corrections.data.next })}>訂正の続き →</a>{/if}
        {:else if detail.corrections?.error}<p class="error" role="alert">訂正一覧を取得できません。{detail.corrections.error}</p>{/if}
      </section>
      <section aria-labelledby="related-memory-title"><h2 id="related-memory-title">関連するMemory</h2>
        {#if detail.related?.data}
          {#if detail.related.data.items.length === 0}<p class="muted">{new URLSearchParams(data.urlQuery).has('related_after') ? 'このページに関連Memoryはありません。' : '関連するMemoryは未接続です。'}</p>{/if}
          <ul class="related-list">{#each detail.related.data.items as item}<li><a href={memoryHref(item.related.selector.memory_id, item.related.selector.change_id)}>{item.claim} →</a>{#if item.related.reconsideration}<p class="muted">訂正あり・Memoryの再検討が必要</p>{/if}</li>{/each}</ul>
          {#if detail.related.data.next}<a href={href({ related: 'all', related_after: JSON.stringify(detail.related.data.next) })}>すべて見る（関連Memoryの続き） →</a>{/if}
          {#if new URLSearchParams(data.urlQuery).has('related_after')}<a href={href({ related_after: null })}>関連Memoryの先頭へ</a>{/if}
        {:else if detail.related?.error}<p class="error" role="alert">関連するMemoryを取得できません。{detail.related.error}</p>{/if}
      </section>
      <section aria-label="原資料"><h2>原資料</h2><OriginalSource source={record.original_source} /></section>
      {@render textSection('Sessionの現在地', 'position', detail.position)}
    {:else}
      <div role="alert" class="error"><h2>Journalエントリを取得できません</h2><p>{detail?.entry.error}</p></div>
    {/if}
  {:else}
    <nav aria-label="Journalの一覧"><a href={base} aria-current={data.view === 'entries' && !data.journalId ? 'page' : undefined}>最近の出来事</a><a href={`${base}?view=sessions`} aria-current={data.view === 'sessions' ? 'page' : undefined}>SessionごとのJournal</a></nav>
    {#if data.view === 'entries'}
      <form action={base} method="GET" role="search">
        {#if data.journalId}<input type="hidden" name="journal" value={data.journalId} />{/if}
        <label for="journal-query">出来事を検索</label><div class="search-row"><input id="journal-query" type="search" name="q" value={data.query ?? ''} maxlength="1024" /><button type="submit">検索</button></div>
      </form>
      {#if data.journalId}{@render textSection('Sessionの現在地', 'position', data.position)}<h2>出来事の経緯</h2>{/if}
      {#if data.entries?.data}
        {#if data.entries.data.items.length}{@render entryList(data.entries.data.items)}
        {:else if data.query}<p role="status">検索結果は0件です。</p>
        {:else if data.after}<p role="status">このページの出来事はありません。</p>
        {:else}<p role="status">保存済みの出来事はありません。<a href={`${base}?view=sessions`}>SessionのJournal・記録状態を見る →</a></p>{/if}
        {#if data.entries.data.next}<a href={href({ after: data.entries.data.next })}>次のページ →</a>{/if}
      {:else if data.entries?.error}<p class="error" role="alert">出来事一覧を取得できません。{data.entries.error}</p>{/if}
    {:else}
      {#if data.sessions?.data}
        {#if data.sessions.data.items.length === 0}<p role="status">{data.after ? 'このページのJournalはありません。' : 'Journalはまだありません。'}</p>{/if}
        <ul class="session-list">{#each data.sessions.data.items as session (session.journal.id)}<li>
          <h2><a href={journalHref(data.workspaceId, data.subjectId, { journal: session.journal.id })}>{session.journal.title} →</a></h2>
          <p><time datetime={session.journal.created_at}>{formatDate(session.journal.created_at)}</time></p>
          {#if session.unreflected_sources}<p class="attention">記録待ち・未反映の原資料があります。</p>{:else}<p class="muted">保存された原資料に未反映は確認されていません。</p>{/if}
          <details><summary>原資料とSession識別情報</summary><code>{session.journal.source_session_id}</code><OriginalSource source={session.original_source} /></details>
        </li>{/each}</ul>
        {#if data.sessions.data.next}<a href={href({ after: data.sessions.data.next })}>次のページ →</a>{/if}
      {:else if data.sessions?.error}<p class="error" role="alert">SessionのJournalを取得できません。{data.sessions.error}</p>{/if}
    {/if}
    {#if data.after}<a href={href({ after: null })}>一覧の先頭へ</a>{/if}
  {/if}
</section>
<style>
  .journal-page { display: grid; gap: var(--space-5); flex: 0 0 auto; width: 100%; min-width: 0; max-width: 78rem; margin-inline: auto; }
  h1, h2, h3, p { margin: 0; overflow-wrap: anywhere; }
  h1 { font-size: var(--font-size-title); line-height: var(--line-height-title); }
  h2, h3 { font-size: var(--font-size-body); }
  header { padding-bottom: var(--space-4); border-bottom: 1px solid var(--line); }
  header p { margin-top: var(--space-2); color: var(--text-muted); }
  section section, form { display: grid; gap: var(--space-3); min-width: 0; }
  nav { display: flex; gap: var(--space-4); flex-wrap: wrap; }
  a { overflow-wrap: anywhere; }
  [aria-current='page'] { color: var(--accent); font-weight: 700; }
  .muted, time { color: var(--text-muted); }
  .attention { color: var(--warning); }
  .error { color: var(--danger); }
  .entry-list, .related-list, .session-list { list-style: none; padding: 0; margin: 0; display: grid; min-width: 0; }
  .entry-list li, .session-list li { padding-block: var(--space-4); border-bottom: 1px solid var(--line); min-width: 0; }
  .entry-list li a { display: flex; gap: var(--space-3); align-items: baseline; }
  .entry-list strong { flex: 1; }
  .entry-list time { font-size: var(--font-size-compact); }
  .entry-list li p { margin-top: var(--space-1); font-size: var(--font-size-compact); }
  .related-list { gap: var(--space-3); }
  .session-list li { display: grid; gap: var(--space-2); }
  .search-row { display: flex; gap: var(--space-2); }
  input[type='search'] { flex: 1; min-width: 0; }
  details code { display: block; overflow-wrap: anywhere; }
  @media(max-width: 600px) { .entry-list li a { display: grid; gap: var(--space-1); grid-template-columns: 1fr auto; } .entry-list time { grid-column: 1 / -1; } }
</style>
