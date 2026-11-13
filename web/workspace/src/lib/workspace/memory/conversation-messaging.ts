import type { ChatConversation, ChatMessage, ChatMessagePage, ChatReceiveResponse, MessagingCommand, MessagingEvent, MessagingResult, MessagingSelector } from '#lib/generated/chat-api.ts';
import type { WorkspaceMessagingConnection } from '../messaging-transport.ts';
import { ConversationApiError, parseConversation, parseConversationPage, parseConversationReceive } from './conversation-api.ts';
import { conversationOperationId, type ConversationAttempt, type ConversationScope } from './conversation-attempt.ts';

export type ConversationMessagingObserver = {
  onPage(page: ChatMessagePage, messages: ChatMessage[]): void;
  onStatus(status: 'connecting' | 'open' | 'closed'): void;
  onError(error: ConversationApiError): void;
};
type Connect = (listener: {
  onEvent(event: MessagingEvent): void;
  onStatus(status: 'connecting' | 'open' | 'closed'): void;
}) => WorkspaceMessagingConnection;

/** 保存pageだけが履歴cursorを進める。POST ACKやsequenceの隣接性には依存しない。 */
export class ConversationMessagingClient {
  #connection: WorkspaceMessagingConnection;
  #conversation: ChatConversation | null = null;
  #messages: ChatMessage[] = [];
  #subscriptionId: string | null = null;
  #epoch = 0;
  #closed = false;
  constructor(readonly scope: ConversationScope, readonly signal: AbortSignal, readonly observer: ConversationMessagingObserver, connect: Connect) {
    this.#connection = connect({
      onStatus: status => {
        if (this.#closed || signal.aborted) return;
        this.#epoch++;
        this.#subscriptionId = null;
        observer.onStatus(status);
        if (status === 'open') void this.#subscribe(this.#epoch);
      },
      onEvent: event => {
        if (this.#closed || signal.aborted || event.subscription_id !== this.#subscriptionId) return;
        if (event.kind === 'messages') {
          try { this.#accept(event.page); }
          catch { this.#fail(new ConversationApiError(null)); }
        } else {
          this.#subscriptionId = null;
          if (event.reason === 'access_revoked') this.#fail(new ConversationApiError(403));
          else { observer.onStatus('connecting'); void this.#subscribe(++this.#epoch); }
        }
      },
    });
    signal.addEventListener('abort', () => this.close(), { once: true });
  }
  #current(epoch: number): boolean { return !this.#closed && !this.signal.aborted && epoch === this.#epoch; }
  #selector(): MessagingSelector {
    if (!this.#conversation) throw new ConversationApiError(null);
    return { subject_id: this.scope.subjectId, conversation_id: this.#conversation.id, after_sequence: this.#messages.at(-1)?.sequence ?? 0 };
  }
  async #request(command: MessagingCommand): Promise<MessagingResult> {
    try {
      const result = await this.#connection.request(command, this.signal);
      if (result.kind === 'rejected') throw new ConversationApiError(result.status);
      return result;
    } catch (error) { throw error instanceof ConversationApiError ? error : new ConversationApiError(null); }
  }
  async #subscribe(epoch: number): Promise<void> {
    try {
      if (!this.#conversation) {
        const result = await this.#request({ kind: 'open', request: { subject_id: this.scope.subjectId, operation_id: conversationOperationId(this.scope.subjectId) } });
        if (!this.#current(epoch)) return;
        if (result.kind !== 'opened') throw new ConversationApiError(null);
        this.#conversation = parseConversation(result.conversation, this.scope);
      }
      const result = await this.#request({ kind: 'subscribe', selector: this.#selector() });
      if (!this.#current(epoch)) return;
      if (result.kind !== 'subscribed') throw new ConversationApiError(null);
      this.#subscriptionId = result.subscription_id;
      this.#accept(result.page);
      this.observer.onStatus('open');
    } catch (error) { if (this.#current(epoch)) this.#fail(error instanceof ConversationApiError ? error : new ConversationApiError(null)); }
  }
  #accept(value: unknown): void {
    if (!this.#conversation || this.signal.aborted || this.#closed) return;
    const page = parseConversationPage(value, this.scope, this.#conversation.id, 0);
    const cursor = this.#messages.at(-1)?.sequence ?? 0;
    const saved = new Map(this.#messages.map(m => [m.id, m]));
    const incoming: ChatMessage[] = [];
    for (const m of page.messages) {
      if (m.sequence <= cursor) {
        const previous = saved.get(m.id);
        if (!previous || JSON.stringify(previous) !== JSON.stringify(m)) throw new ConversationApiError(null);
      } else incoming.push(m);
    }
    this.#messages = [...this.#messages, ...incoming];
    this.#conversation = page.conversation;
    this.observer.onPage(page, [...this.#messages]);
  }
  #fail(error: ConversationApiError): void {
    this.observer.onError(error);
    // A malformed conversation result cannot terminate unrelated Worker subscriptions.
    if ([401, 403, 404].includes(error.status ?? 0)) this.close();
  }
  async refresh(): Promise<void> {
    if (this.#closed || this.signal.aborted || !this.#conversation) return;
    const epoch = this.#epoch;
    const selector = this.#selector();
    const result = await this.#request({ kind: 'history', selector, query: { after_sequence: selector.after_sequence, limit: 100 } });
    if (!this.#current(epoch)) return;
    if (result.kind !== 'history') throw new ConversationApiError(null);
    this.#accept(result.page);
  }
  async post(attempt: ConversationAttempt): Promise<ChatReceiveResponse> {
    if (this.#closed || this.signal.aborted || !this.#conversation || attempt.conversationId !== this.#conversation.id) throw new ConversationApiError(null);
    const result = await this.#request({ kind: 'post', selector: this.#selector(), request: {
      operation_id: attempt.operationId, text: attempt.text, reply_to: attempt.replyTo, work_ref: attempt.workRef,
    } });
    if (result.kind !== 'saved') throw new ConversationApiError(null);
    return parseConversationReceive(result.response, this.scope, attempt);
  }
  close(): void {
    if (this.#closed) return;
    this.#closed = true; this.#epoch++; this.#subscriptionId = null;
    this.#connection.close();
  }
}
