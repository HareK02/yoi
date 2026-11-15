<script lang="ts">
  import { untrack } from 'svelte';
  import ComposerInput from '#lib/workspace/console/ComposerInput.svelte';
  import ConversationHistory from './ConversationHistory.svelte';
  import { conversationDraftKey, readConversationDraft, saveConversationDraft } from './conversation-draft.ts';
  import { CHAT_LIMITS, chatHasText, type ChatConversation, type ChatDelivery, type ChatMessage } from '#lib/generated/chat-api.ts';
  import { browserSession } from '#lib/workspace/auth/session-state.ts';
  import { ConversationApiError } from './conversation-api.ts';
  import { ConversationMessagingClient } from './conversation-messaging.ts';
  import { workspaceMultiplexer } from '#lib/workspace/multiplexer.ts';
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
  let draftKey = $state<string | null>(null);
  let draftStorageError = $state<string | null>(null);
  let composer = $state<ReturnType<typeof ComposerInput> | null>(null);

  function editDraft(text: string): void {
    const context = active;
    if (!context || !current(context) || !draftKey || text === draft) return;
    draft = text;
    try {
      saveConversationDraft(sessionStorage, draftKey, text);
      draftStorageError = null;
    } catch { draftStorageError = 'Your draft is editable but could not be saved in this tab. Keep this page open or copy the text before leaving.'; }
  }
  let attempt = $state<ConversationAttempt | null>(null);
  let delivery = $state<ChatDelivery | null>(null);
  let historyError = $state<string | null>(null);
  let sendError = $state<string | null>(null);
  let storageBlocked = $state(false);
  let inaccessible = $state(false);
  let connectionState = $state<'connecting' | 'open' | 'closed'>('connecting');
  let active: { scope: ConversationScope; controller: AbortController; authGeneration: number; authStamp: string | null; client: ConversationMessagingClient | null } | null = null;
  const draftBytes = $derived(new TextEncoder().encode(draft).length);

  function current(context: NonNullable<typeof active>): boolean {
    const snapshot = browserSession.snapshot;
    return active === context && !context.controller.signal.aborted &&
      workspaceId === context.scope.workspaceId && subjectId === context.scope.subjectId && accountId === context.scope.accountId &&
      browserSession.authenticationGeneration === context.authGeneration && browserSession.authenticationStamp === context.authStamp &&
      snapshot.kind === 'ready' && snapshot.value.session.kind === 'authenticated' && snapshot.value.actor?.account_id === context.scope.accountId;
  }

  function withdrawAccess(context: NonNullable<typeof active>): void {
    context.client?.close();
    context.controller.abort();
    inaccessible = true;
    connectionState = 'closed';
    conversation = null;
    messages = [];
    attempt = null;
    delivery = null;
    draft = '';
    draftKey = null;

    draftStorageError = null;
    sending = false;
    reading = false;
    storageBlocked = false;
  }

  $effect(() => {
    const scope: ConversationScope = { workspaceId, subjectId, accountId };
    return untrack(() => {
      const context: NonNullable<typeof active> = { client: null, scope, controller: new AbortController(), authGeneration: browserSession.authenticationGeneration, authStamp: browserSession.authenticationStamp };
      active = context;
      conversation = null;
      messages = [];
      hasMore = false;
      reading = false;
      sending = false;
      draft = '';
      draftKey = null;

      draftStorageError = null;
      attempt = null;
      delivery = null;
      historyError = null;
      sendError = null;
      storageBlocked = false;
      inaccessible = false;
      try { attempt = readConversationAttempt(sessionStorage, scope); }
      catch { storageBlocked = true; sendError = 'Saved send input cannot be read. No new send will be started.'; }
      reading = true;
      connectionState = 'connecting';
      context.client = new ConversationMessagingClient(scope, context.controller.signal, {
        onPage: (page, restored) => {
          if (!current(context)) return;
          const nextDraftKey = conversationDraftKey(context.scope, page.conversation.id);
          if (nextDraftKey !== draftKey) {
            draftKey = nextDraftKey;

            try { draft = readConversationDraft(sessionStorage, nextDraftKey); }
            catch { draft = ''; draftStorageError = 'The saved draft could not be read. It has not been replaced.'; }
          }
          conversation = page.conversation;
          messages = restored;
          hasMore = page.has_more;
          reading = false;
          historyError = null;
          if (attempt && attempt.conversationId !== conversation.id) {
            storageBlocked = true;
            sendError = 'The saved send belongs to another conversation. It has not been replaced.';
          }
        },
        onStatus: status => {
          if (!current(context)) return;
          connectionState = status;
          if (status === 'closed') {
            reading = false;
            if (!conversation) historyError = 'Conversation connection is unavailable. Confirm your session and viewing permissions, then refresh.';
          }
        },
        onError: error => {
          if (!current(context)) return;
          historyError = error.message;
          reading = false;
          if ([401, 403, 404].includes(error.status ?? 0)) withdrawAccess(context);
        },
      }, listener => workspaceMultiplexer(scope.workspaceId).connectMessaging(listener));
      const unsubscribe = browserSession.subscribe(() => {
        // A withdrawn request keeps its actual failure; an aborted fetch is not an Account change.
        if (active !== context || context.controller.signal.aborted) return;
        if (!current(context)) {
          withdrawAccess(context);
          sendError = null;
          historyError = 'The conversation account changed or could not be confirmed. Reopen this page after confirming your session.';
        }
      });
      return () => {
        context.controller.abort();
        if (active === context) active = null;
        unsubscribe();
      };
    });
  });

  async function refresh(context = active): Promise<void> {
    if (!context || !current(context) || reading || inaccessible) return;
    reading = true;
    try {
      await context.client?.refresh();
      if (!current(context)) return;
      // onPage clears history errors only after a confirmed snapshot, including resubscription.
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
    if (!retry && (attempt || !chatHasText(draft) || draftBytes > CHAT_LIMITS.maxTextBytes)) return;
    const request = retry ? attempt : Object.freeze({ conversationId: conversation.id, operationId: crypto.randomUUID(), text: draft, replyTo: null, workRef: null });
    if (!request || request.conversationId !== conversation.id) return;
    // 同期保存が成功するまで送信しない。unknownでの編集や再訪でも同じ全入力を使う。
    try { saveConversationAttempt(sessionStorage, context.scope, request); }
    catch { storageBlocked = true; sendError = 'Send input could not be retained safely. Nothing new was dispatched.'; return; }
    attempt = request;
    if (!retry) {
      if (composer) composer.recordHistory(composer.snapshot());
      composer?.clear();
      editDraft('');
    }
    sending = true;
    delivery = null;
    sendError = null;
    try {
      const response = await context.client!.post(request);
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
      // The committed subscription page, not this ACK, advances the history cursor.
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
  <p class="muted" role="status">Conversation connection: {connectionState === 'open' ? 'connected' : connectionState === 'connecting' ? 'reconnecting' : 'disconnected — history is the last confirmed observation'}</p>
  {#if historyError}<p class="error" role="alert">{historyError}</p>{/if}
  {#if !conversation && !historyError && !inaccessible}<p role="status">Loading conversation…</p>{/if}
  {#if conversation}
    {#key draftKey}<ConversationHistory {messages} {accountId} {subjectName} />{/key}
  {/if}
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
  {#if !inaccessible && conversation}
    <form onsubmit={(event) => { event.preventDefault(); void send(); }}>
      <span id="conversation-input-label">Message to {subjectName}</span>
      <div class="conversation-editor">
      {#key draftKey}
        <ComposerInput bind:this={composer} initialText={draft} historyScope={draftKey ?? ''} historyStorage="session" workerFeatures={false}
          ariaLabel={`Message to ${subjectName}`} ariaDescribedBy="conversation-constraint" onchange={(snapshot) => editDraft(snapshot.content)}
          onunsupportedpaste={() => { sendError = 'Attachments are not supported in this conversation. Paste text instead.'; }}
          onsubmit={() => { void send(); }} />
      {/key}
      </div>
      <p id="conversation-constraint">Text only, up to {CHAT_LIMITS.maxTextBytes} UTF-8 bytes. Attachments are not supported. Ctrl/⌘+Enter to send.</p>
      {#if draftStorageError}<p class="error" role="alert">{draftStorageError}</p>{/if}
      {#if draftBytes > CHAT_LIMITS.maxTextBytes}<p class="error">The message exceeds the text limit.</p>{/if}
      {#if attempt}<p>Resolve the retained send before sending another message. Your next draft remains editable.</p>{/if}
      <button class="workspace-secondary-button" type="submit" disabled={sending || !!attempt || storageBlocked || !conversation || !chatHasText(draft) || draftBytes > CHAT_LIMITS.maxTextBytes}>Send message</button>
    </form>
  {/if}
</section>

<style>
  .subject-conversation { display: grid; gap: var(--space-4); font-size: var(--font-size-body); line-height: var(--line-height-body); min-width: 0; }
  header { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: var(--space-3); }
  h2, p { margin: 0; }
  h2 { font-size: var(--font-size-body); }
  .message-text { white-space: pre-wrap; overflow-wrap: anywhere; }
  form { display: grid; gap: var(--space-2); }
  .conversation-editor { min-width: 0; border: 1px solid var(--line-strong); padding: var(--space-2); }
  .conversation-editor:focus-within { outline: 2px solid var(--accent); outline-offset: 2px; }
  .conversation-editor :global(.cm-content) { min-height: 4rem; }
  .conversation-editor :global(.cm-scroller) { max-height: 12rem; overflow-y: auto; }
  button { justify-self: start; font-size: var(--font-size-body); line-height: var(--line-height-body); border-radius: var(--radius-soft); }
  button:hover:not(:disabled) { background: var(--interactive-hover); }
  button:active:not(:disabled) { background: var(--interactive-selected); }
  button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .error { color: var(--danger); }
</style>
