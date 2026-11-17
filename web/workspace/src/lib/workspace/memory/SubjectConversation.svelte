<script lang="ts">
  import { untrack } from 'svelte';
  import '#lib/workspace/composer/composer-shell.css';
  import ComposerInput from '#lib/workspace/console/ComposerInput.svelte';
  import ConversationHistory from './ConversationHistory.svelte';
  import type { ConversationOutgoing } from './conversation-view.ts';
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
  let outgoing = $state<ConversationOutgoing[]>([]);
  function updateOutgoing(request: ConversationAttempt, state: ConversationOutgoing['state']): void {
    const previous = outgoing.findIndex(item => item.request.operationId === request.operationId);
    if (previous < 0) outgoing = [...outgoing, { request, state }];
    else outgoing = outgoing.map((item, index) => index === previous ? { request, state } : item);
  }
  function retryMessage(operationId: string): void {
    if (attempt?.operationId === operationId) void send(true);
  }
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
    outgoing = [];
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
      outgoing = [];
      delivery = null;
      historyError = null;
      sendError = null;
      storageBlocked = false;
      inaccessible = false;
      try {
        attempt = readConversationAttempt(sessionStorage, scope);
        if (attempt) updateOutgoing(attempt, 'retry');
      }
      catch { storageBlocked = true; sendError = 'Saved message could not be read. Sending is paused.'; }
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
            sendError = 'Saved message belongs to another conversation. Sending is paused.';
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
          historyError = [401, 403, 404].includes(error.status ?? 0) ? error.message : 'Conversation history could not be loaded.';
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
      historyError = error instanceof ConversationApiError && [401, 403, 404].includes(error.status ?? 0) ? error.message : 'Conversation history could not be loaded.';
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
    catch { storageBlocked = true; sendError = 'Message could not be saved in this tab. Nothing was sent.'; return; }
    attempt = request;
    updateOutgoing(request, 'sending');
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
      updateOutgoing(request, response.delivery.state === 'accepted' ? 'delivered' : 'retry');
      if (response.delivery.state === 'accepted') {
        try {
          clearConversationAttempt(sessionStorage, context.scope, request);
          attempt = null;
        } catch {
          storageBlocked = true;
          sendError = 'Local storage could not be updated. Sending is paused.';
        }
      }
      // The committed subscription page, not this ACK, advances the history cursor.
    } catch (error) {
      if (!current(context)) return;
      updateOutgoing(request, 'retry');
      sendError = 'Delivery could not be confirmed.';
      if (error instanceof ConversationApiError && [401, 403, 404].includes(error.status ?? 0)) {
        sendError = error.message;
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
      if (attempt) updateOutgoing(attempt, 'retry');
      storageBlocked = false;
      sendError = null;
    } catch { sendError = 'Local storage is still unavailable. Sending is paused.'; }
  }
</script>

<section class="subject-conversation" aria-label="Conversation with {subjectName}">
  <header>
    <p class="muted" role="status">Conversation connection: {connectionState === 'open' ? 'connected' : connectionState === 'connecting' ? 'reconnecting' : 'disconnected — history is the last confirmed observation'}</p>
    {#if !inaccessible}<button class="workspace-secondary-button" type="button" onclick={() => refresh()} disabled={reading}>{reading ? 'Loading history…' : 'Refresh history'}</button>{/if}
  </header>
  {#if historyError}<p class="error" role="alert">{historyError}</p>{/if}
  {#if !conversation && !historyError && !inaccessible}<p role="status">Loading conversation…</p>{/if}
  {#if conversation}
    {#key draftKey}
      <ConversationHistory {messages} {accountId} {subjectName}
        outgoing={outgoing.filter(item => item.request.conversationId === conversation?.id)}
        onretry={retryMessage} retryDisabled={sending || storageBlocked || inaccessible} />
    {/key}
  {/if}
  {#if hasMore && !inaccessible}<button class="workspace-secondary-button" type="button" onclick={() => refresh()} disabled={reading}>Load newer messages</button>{/if}

  {#if sendError}<p class="error" role="alert">{sendError}</p>{/if}
  {#if storageBlocked}<button class="workspace-secondary-button" type="button" onclick={recheckStorage} disabled={sending}>Recheck local storage</button>{/if}
  {#if !inaccessible && conversation}
    <form onsubmit={(event) => { event.preventDefault(); void send(); }}>
      <div class="composer-input-shell" role="group" aria-label={`Message to ${subjectName}`}>
        {#key draftKey}
          <ComposerInput bind:this={composer} initialText={draft} historyScope={draftKey ?? ''} historyStorage="session" workerFeatures={false}
            ariaLabel={`Message to ${subjectName}`} ariaDescribedBy="conversation-constraint" onchange={(snapshot) => editDraft(snapshot.content)}
            onunsupportedpaste={() => { sendError = 'Attachments are not supported in this conversation. Paste text instead.'; }}
            onsubmit={() => { void send(); }} />
        {/key}
        <div class="composer-input-footer">
          <div class="composer-footer-slot"><span class="muted">Text only · Ctrl/⌘+Enter</span></div>
          <button class="composer-send-button" type="submit" aria-label="Send message" title="Send message"
            disabled={sending || !!attempt || storageBlocked || !conversation || !chatHasText(draft) || draftBytes > CHAT_LIMITS.maxTextBytes}>
            <svg class="composer-send-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M8 6L12 2L16 6" /><path d="M12 2V22" /></svg>
          </button>
        </div>
      </div>
      <p class="visually-hidden" id="conversation-constraint">Text only, up to {CHAT_LIMITS.maxTextBytes} UTF-8 bytes. Attachments are not supported. Ctrl/⌘+Enter to send.</p>
      {#if draftStorageError}<p class="error" role="alert">{draftStorageError}</p>{/if}
      {#if draftBytes > CHAT_LIMITS.maxTextBytes}<p class="error" role="alert">The message exceeds the text limit ({CHAT_LIMITS.maxTextBytes} UTF-8 bytes).</p>{/if}
    </form>
  {/if}
</section>

<style>
  .subject-conversation { display: flex; flex-direction: column; flex: 1 1 0; gap: var(--space-2); font-size: var(--font-size-body); line-height: var(--line-height-body); min-width: 0; min-height: 0; }
  header { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); flex: 0 0 auto; }
  p { margin: 0; }
  .muted { color: var(--text-muted); font-size: var(--font-size-compact); line-height: var(--line-height-compact); }
  form { display: grid; gap: var(--space-2); flex: 0 0 auto; }
  .visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  .workspace-secondary-button { align-self: start; flex-shrink: 0; font-size: var(--font-size-compact); line-height: var(--line-height-compact); border-radius: var(--radius-soft); }
  .workspace-secondary-button:hover:not(:disabled) { background: var(--interactive-hover); }
  .workspace-secondary-button:active:not(:disabled) { background: var(--interactive-selected); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .error { color: var(--danger); }
</style>
