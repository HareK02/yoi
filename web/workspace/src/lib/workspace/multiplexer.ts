import { browser } from '$app/environment';
import type {
  EventSubscriptionSelector,
  Method,
  SubscriptionFrame,
  SubscriptionId,
} from '$lib/generated/protocol';
import { workspaceApiPath } from '$lib/workspace/api/http';

type Listener = {
  onFrame(frame: SubscriptionFrame): void;
  onStatus?(status: 'connecting' | 'open' | 'closed', message?: string): void;
};

type ActiveSubscription = {
  clientId: string;
  selector: EventSubscriptionSelector;
  listener: Listener;
  requestId: string | null;
  subscriptionId: SubscriptionId | null;
};

export type WorkspaceMultiplexerSubscription = {
  close(): void;
  sendWorkerMethod(method: Method): void;
};

const multiplexers = new Map<string, WorkspaceMultiplexer>();

export function workspaceMultiplexer(workspaceId: string): WorkspaceMultiplexer {
  let multiplexer = multiplexers.get(workspaceId);
  if (!multiplexer) {
    multiplexer = new WorkspaceMultiplexer(workspaceId);
    multiplexers.set(workspaceId, multiplexer);
  }
  return multiplexer;
}

export class WorkspaceMultiplexer {
  readonly #workspaceId: string;
  readonly #subscriptions = new Map<string, ActiveSubscription>();
  readonly #requests = new Map<string, string>();
  readonly #runtimeSubscriptions = new Map<string, string>();
  #socket: WebSocket | null = null;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #closed = false;

  constructor(workspaceId: string) {
    this.#workspaceId = workspaceId;
  }

  subscribe(
    selector: EventSubscriptionSelector,
    listener: Listener,
  ): WorkspaceMultiplexerSubscription {
    const clientId = crypto.randomUUID();
    const subscription: ActiveSubscription = {
      clientId,
      selector,
      listener,
      requestId: null,
      subscriptionId: null,
    };
    this.#subscriptions.set(clientId, subscription);
    this.#closed = false;
    if (this.#socket?.readyState === WebSocket.OPEN) {
      subscription.listener.onStatus?.('open');
      this.#sendSubscribe(subscription);
    } else {
      this.#ensureConnected();
    }
    return {
      close: () => this.#remove(clientId),
      sendWorkerMethod: (method) => this.#sendWorkerMethod(clientId, method),
    };
  }

  #ensureConnected(): void {
    if (!browser || this.#socket || this.#closed || this.#subscriptions.size === 0) return;
    for (const subscription of this.#subscriptions.values()) {
      subscription.listener.onStatus?.('connecting');
    }
    const url = new URL(
      workspaceApiPath(this.#workspaceId, '/protocol/ws'),
      window.location.origin,
    );
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(url);
    this.#socket = socket;
    socket.addEventListener('open', () => {
      for (const subscription of this.#subscriptions.values()) {
        subscription.listener.onStatus?.('open');
        this.#sendSubscribe(subscription);
      }
    });
    socket.addEventListener('message', (event) => this.#receive(String(event.data)));
    socket.addEventListener('error', () => socket.close());
    socket.addEventListener('close', () => {
      if (this.#socket !== socket) return;
      this.#socket = null;
      this.#requests.clear();
      this.#runtimeSubscriptions.clear();
      for (const subscription of this.#subscriptions.values()) {
        subscription.requestId = null;
        subscription.subscriptionId = null;
        subscription.listener.onStatus?.('closed', 'Workspace subscription disconnected');
      }
      if (!this.#closed && this.#subscriptions.size > 0) {
        this.#reconnectTimer = setTimeout(() => this.#ensureConnected(), 500);
      }
    });
  }

  #sendSubscribe(subscription: ActiveSubscription): void {
    const requestId = crypto.randomUUID();
    subscription.requestId = requestId;
    this.#requests.set(requestId, subscription.clientId);
    this.#send({
      protocol_version: 1,
      frame: 'request',
      message: {
        method: 'subscribe_events',
        params: { request_id: requestId, selector: subscription.selector },
      },
    });
  }

  #receive(text: string): void {
    let frame: SubscriptionFrame;
    try {
      frame = JSON.parse(text) as SubscriptionFrame;
    } catch {
      this.#socket?.close();
      return;
    }
    if (frame.protocol_version !== 1) {
      this.#socket?.close();
      return;
    }
    if (frame.frame === 'response' && frame.message.result === 'subscribed') {
      const clientId = this.#requests.get(frame.message.payload.request_id);
      const subscription = clientId ? this.#subscriptions.get(clientId) : undefined;
      if (!clientId || !subscription) return;
      this.#requests.delete(frame.message.payload.request_id);
      const subscriptionId = frame.message.payload.subscription_id;
      if (!subscriptionId) return;
      subscription.subscriptionId = subscriptionId;
      this.#runtimeSubscriptions.set(subscriptionId, clientId);
      subscription.listener.onFrame(frame);
      return;
    }
    if (frame.frame === 'response' && frame.message.result === 'subscription_rejected') {
      const clientId = this.#requests.get(frame.message.payload.request_id);
      const subscription = clientId ? this.#subscriptions.get(clientId) : undefined;
      subscription?.listener.onFrame(frame);
      if (clientId) {
        this.#requests.delete(frame.message.payload.request_id);
        this.#remove(clientId);
      }
      return;
    }
    if (frame.frame === 'event') {
      const clientId = this.#runtimeSubscriptions.get(frame.message.data.subscription_id);
      const subscription = clientId ? this.#subscriptions.get(clientId) : undefined;
      subscription?.listener.onFrame(frame);
      if (
        frame.message.event === 'subscription_closed' &&
        clientId &&
        subscription &&
        this.#socket?.readyState === WebSocket.OPEN
      ) {
        this.#runtimeSubscriptions.delete(frame.message.data.subscription_id);
        subscription.subscriptionId = null;
        subscription.listener.onStatus?.('connecting', frame.message.data.message);
        this.#sendSubscribe(subscription);
      }
    }
  }

  #sendWorkerMethod(clientId: string, method: Method): void {
    const subscription = this.#subscriptions.get(clientId);
    if (!subscription?.subscriptionId) throw new Error('Worker protocol subscription is not open');
    this.#send({
      protocol_version: 1,
      frame: 'worker_protocol',
      message: { subscription_id: subscription.subscriptionId, method },
    });
  }

  #remove(clientId: string): void {
    const subscription = this.#subscriptions.get(clientId);
    if (!subscription) return;
    this.#subscriptions.delete(clientId);
    if (subscription.subscriptionId && this.#socket?.readyState === WebSocket.OPEN) {
      this.#send({
        protocol_version: 1,
        frame: 'request',
        message: {
          method: 'unsubscribe_events',
          params: {
            request_id: crypto.randomUUID(),
            subscription_id: subscription.subscriptionId,
          },
        },
      });
      this.#runtimeSubscriptions.delete(subscription.subscriptionId);
    }
    if (this.#subscriptions.size === 0) {
      this.#closed = true;
      if (this.#reconnectTimer) clearTimeout(this.#reconnectTimer);
      this.#socket?.close();
      this.#socket = null;
    }
  }

  #send(frame: SubscriptionFrame): void {
    if (this.#socket?.readyState !== WebSocket.OPEN) return;
    this.#socket.send(JSON.stringify(frame));
  }
}
