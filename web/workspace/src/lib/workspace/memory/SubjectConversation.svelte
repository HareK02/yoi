<script lang="ts">
  import { untrack } from 'svelte';
  import { CHAT_LIMITS, type ChatConversation, type ChatDelivery, type ChatMessage } from '#lib/generated/chat-api.ts';
  import { browserSession } from '#lib/workspace/auth/session-state.ts';
  import { ConversationApiError, appendConversationMessages, getConversationPage, openSubjectConversation, sendConversationInput } from './conversation-api.ts';
  import { clearConversationAttempt, readConversationAttempt, saveConversationAttempt, type ConversationAttempt, type ConversationScope } from './conversation-attempt.ts';

  // 本体の現在状態・起動停止はroute側の共有SubjectBodyが所有する。会話ACKから推測しない。
  let { workspaceId, subjectId, accountId, subjectName }: {
    workspaceId: string;
    subjectId: string;
    accountId: string;
    subjectName: string;
  } = $props();
  let conversation = $state<ChatConversation | null>(null);
  let messages = $state<ChatMessage[]>([]);
  let hasMore = $state(false);
  let reading = $state(false);
  let sending = $state(false);
  let draft = $state('');
  let attempt = $state<ConversationAttempt | null>(null);
  let delivery = $state<ChatDelivery | null>(null);
  let historyError = $state<string | null>(null);
  let sendError = $state<string | null>(null);
  let storageBlocked = $state(false);
  let inaccessible = $state(false);
  let active: { scope: ConversationScope; controller: AbortController; authGeneration: number; authStamp: string | null } | null = null;
  const draftBytes = $derived(new TextEncoder().encode(draft).length);

  function current(context: NonNullable<typeof active>): boolean {
    const snapshot = browserSession.snapshot;
    return active === context && !context.controller.signal.aborted &&
      workspaceId === context.scope.workspaceId && subjectId === context.scope.subjectId && accountId === context.scope.accountId &&
      browserSession.authenticationGeneration === context.authGeneration && browserSession.authenticationStamp === context.authStamp &&
      snapshot.kind === 'ready' && snapshot.value.session.kind === 'authenticated' && snapshot.value.actor?.account_id === context.scope.accountId;
  }

  function withdrawAccess(context: NonNullable<typeof active>): void {
    context.controller.abort();
    inaccessible = true;
    conversation = null;
    messages = [];
    attempt = null;
    delivery = null;
    draft = '';
    sending = false;
    reading = false;
    storageBlocked = false;
  }

  $effect(() => {
    const scope: ConversationScope = { workspaceId, subjectId, accountId };
    return untrack(() => {
      const context = { scope, controller: new AbortController(), authGeneration: browserSession.authenticationGeneration, authStamp: browserSession.authenticationStamp };
      active = context;
      conversation = null;
      messages = [];
      hasMore = false;
      reading = false;
      sending = false;
      draft = '';
      attempt = null;
      delivery = null;
      historyError = null;
      sendError = null;
      storageBlocked = false;
      inaccessible = false;
      try { attempt = readConversationAttempt(sessionStorage, scope); }
      catch { storageBlocked = true; sendError = 'Saved send input cannot be read. No new send will be started.'; }
      void refresh(context);
      // 公開履歴だけを再取得する。内部Session購読やモデルのpollingはしない。
      const timer = setInterval(() => { if (!document.hidden) void refresh(context); }, 4000);
      const reconnect = () => { if (!document.hidden) void refresh(context); };
      window.addEventListener('online', reconnect);
      document.addEventListener('visibilitychange', reconnect);
      const unsubscribe = browserSession.subscribe(() => {
        if (!current(context)) {
          withdrawAccess(context);
          sendError = null;
          historyError = 'The conversation account changed or could not be confirmed. Reopen this page after confirming your session.';
        }
      });
      return () => {
        context.controller.abort();
        if (active === context) active = null;
        clearInterval(timer);
        window.removeEventListener('online', reconnect);
        document.removeEventListener('visibilitychange', reconnect);
        unsubscribe();
      };
    });
  });

  async function refresh(context = active): Promise<void> {
    if (!context || !current(context) || reading || inaccessible) return;
    reading = true;
    try {
      const opened = conversation ?? await openSubjectConversation(fetch, context.scope, context.controller.signal);
      if (!current(context)) return;
      if (attempt && attempt.conversationId !== opened.id) {
        storageBlocked = true;
        sendError = 'The saved send belongs to another conversation. It has not been replaced.';
      }
      const after = messages.at(-1)?.sequence ?? 0;
      const page = await getConversationPage(fetch, context.scope, opened.id, after, context.controller.signal);
      if (!current(context)) return;
      // POSTのmessageをcursorにしない。先行する未読の公開返信を飛ばさない。
      conversation = page.conversation;
      messages = appendConversationMessages(messages, page.messages);
      hasMore = page.has_more;
      historyError = null;
    } catch (error) {
      if (!current(context)) return;
      historyError = error instanceof ConversationApiError ? error.message : 'Conversation history could not be loaded.';
      if (error instanceof ConversationApiError && [401, 403, 404].includes(error.status ?? 0)) {
        withdrawAccess(context);
      }
    } finally { if (current(context)) reading = false; }
  }

  async function send(retry = false): Promise<void> {
    const context = active;
    if (!context || !current(context) || sending || inaccessible || storageBlocked || !conversation) return;
    if (!retry && (attempt || !draft.trim() || draftBytes > CHAT_LIMITS.maxTextBytes)) return;
    const request = retry ? attempt : Object.freeze({ conversationId: conversation.id, operationId: crypto.randomUUID(), text: draft, replyTo: null, workRef: null });
    if (!request || request.conversationId !== conversation.id) return;
    // 同期保存が成功するまでPOSTしない。unknownでの編集や再訪でも同じ全入力を使う。
    try { saveConversationAttempt(sessionStorage, context.scope, request); }
    catch { storageBlocked = true; sendError = 'Send input could not be retained safely. Nothing new was dispatched.'; return; }
    attempt = request;
    if (!retry) draft = '';
    sending = true;
    delivery = null;
    sendError = null;
    try {
      const response = await sendConversationInput(fetch, context.scope, request, context.controller.signal);
      if (!current(context)) return;
      delivery = response.delivery;
      if (response.delivery.state === 'accepted') {
        try {
          clearConversationAttempt(sessionStorage, context.scope, request);
          attempt = null;
        } catch {
          storageBlocked = true;
          sendError = 'Runtime accepted this input, but its local retry record could not be cleared. No new send will be started.';
        }
      }
      void refresh(context);
    } catch (error) {
      if (!current(context)) return;
      sendError = error instanceof ConversationApiError ? error.message : 'Send outcome unknown. Keep the saved request.';
      if (error instanceof ConversationApiError && [401, 403, 404].includes(error.status ?? 0)) {
        withdrawAccess(context);
      }
    } finally { if (current(context)) sending = false; }
  }

  function recheckStorage(): void {
    const context = active;
    if (!context || !current(context) || sending) return;
    try {
      const saved = readConversationAttempt(sessionStorage, context.scope);
      if (attempt) {
        // clear失敗後の再確認でも新しい配送は行わない。
        if (delivery?.state === 'accepted') {
          clearConversationAttempt(sessionStorage, context.scope, attempt);
          attempt = null;
        } else {
          saveConversationAttempt(sessionStorage, context.scope, attempt);
        }
      } else attempt = saved;
      if (attempt && conversation && attempt.conversationId !== conversation.id) throw new Error();
      storageBlocked = false;
      sendError = null;
    } catch { sendError = 'The original send input still cannot be retained or reconciled safely. No new send was started.'; }
  }

  function deliveryLabel(value: ChatDelivery): string {
    switch (value.state) {
      case 'accepted': return 'Runtime accepted this input. Processing and replies are not confirmed.';
      case 'no_current_body': return 'Message saved; no current body was available. Start the Subject, then retry this same input.';
      case 'rejected': return 'Message saved; Runtime did not accept this input. Check the body state before retrying.';
      case 'unknown': return 'Message saved; Runtime acceptance is unknown. Retry only this same input.';
      case 'unavailable': return 'Message saved; delivery is unavailable. Retry only this same input.';
    }
  }
</script>

<section class="subject-conversation" aria-label="Conversation with {subjectName}">
  <header>
    <h2>Conversation</h2>
    {#if !inaccessible}<button class="workspace-secondary-button" type="button" onclick={() => refresh()} disabled={reading}>{reading ? 'Loading history…' : 'Refresh history'}</button>{/if}
  </header>
  {#if historyError}<p class="error" role="alert">{historyError}</p>{/if}
  {#if !conversation && !historyError && !inaccessible}<p role="status">Loading conversation…</p>{/if}
  <ol aria-label="Saved public messages">
    {#each messages as message (message.id)}
      <li>
        <strong>{message.sender.kind === 'user' ? 'You' : subjectName}</strong>
        <p class="message-text">{message.text}</p>
      </li>
    {/each}
  </ol>
  {#if conversation && messages.length === 0}<p>No public messages yet.</p>{/if}
  {#if hasMore && !inaccessible}<button class="workspace-secondary-button" type="button" onclick={() => refresh()} disabled={reading}>Load newer messages</button>{/if}

  <div class="send-status" aria-live="polite">
    {#if sending}<p>Sending saved input…</p>
    {:else if delivery}<p>{deliveryLabel(delivery)}</p>
    {:else if attempt}<p>Previous send outcome is unknown. Its original input is retained.</p>{/if}
  </div>
  {#if sendError}<p class="error" role="alert">{sendError}</p>{/if}
  {#if storageBlocked}<button class="workspace-secondary-button" type="button" onclick={recheckStorage} disabled={sending}>Recheck retry storage</button>{/if}
  {#if attempt}
    <details><summary>Retained send input</summary><p class="message-text">{attempt.text}</p></details>
    <p>Retry uses the same ID and full input. Runtime duplicate suppression has bounded receipt retention; it is not an indefinite delivery guarantee.</p>
    {#if !inaccessible && !storageBlocked}<button class="workspace-secondary-button" type="button" onclick={() => send(true)} disabled={sending || !conversation}>Retry same input</button>{/if}
  {/if}
  {#if !inaccessible}
    <form onsubmit={(event) => { event.preventDefault(); void send(); }}>
      <label for="subject-conversation-input">Message to {subjectName}</label>
      <textarea id="subject-conversation-input" bind:value={draft} rows="4" aria-describedby="conversation-constraint"></textarea>
      <p id="conversation-constraint">Text only, up to {CHAT_LIMITS.maxTextBytes} UTF-8 bytes. Attachments are not supported.</p>
      {#if draftBytes > CHAT_LIMITS.maxTextBytes}<p class="error">The message exceeds the text limit.</p>{/if}
      {#if attempt}<p>Resolve the retained send before sending another message. Your next draft remains editable.</p>{/if}
      <button class="workspace-secondary-button" type="submit" disabled={sending || !!attempt || storageBlocked || !conversation || !draft.trim() || draftBytes > CHAT_LIMITS.maxTextBytes}>Send message</button>
    </form>
  {/if}
</section>

<style>
  .subject-conversation { display: grid; gap: var(--space-4); font-size: var(--font-size-body); line-height: var(--line-height-body); min-width: 0; }
  header { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: var(--space-3); }
  h2, p { margin: 0; }
  h2 { font-size: var(--font-size-body); }
  ol { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--space-5); }
  li { display: grid; gap: var(--space-1); }
  .message-text { white-space: pre-wrap; overflow-wrap: anywhere; }
  form { display: grid; gap: var(--space-2); }
  textarea { width: 100%; box-sizing: border-box; resize: vertical; font: inherit; color: var(--text); background: var(--bg); border: 1px solid var(--line-strong); padding: var(--space-2); }
  button { justify-self: start; font-size: var(--font-size-body); line-height: var(--line-height-body); border-radius: var(--radius-soft); }
  button:hover:not(:disabled) { background: var(--interactive-hover); }
  button:active:not(:disabled) { background: var(--interactive-selected); }
  button:focus-visible, textarea:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .error { color: var(--danger); }
</style>
