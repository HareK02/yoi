/** Workspaceごとに共有するsubscription transport。
 * 受信frameをRust生成validatorと保持中selectorの両方で検査する。
 * 不正frameは共有socketを固定診断で閉じ、再接続時にroutingを消してsnapshotを取り直す。
 * 正常なsubscription_closedは対象subscriptionだけを終了または再購読する。
 * 送信Methodの実行時decodeと意味検査はRust Serverが所有する。
 */
import type { MessagingCommand, MessagingEvent, MessagingResponse, MessagingResult } from "#lib/generated/chat-api.ts";
import { browser } from "$app/env";
import type {
  EventSubscriptionSelector,
  Method,
  SubscriptionFrame,
  SubscriptionId,
} from "#lib/generated/protocol.ts";
import {
  decodeSubscriptionFrame,
  subscriptionFrameMatchesSelector,
} from "#lib/generated/protocol-validator.ts";
import { workspaceApiPath } from "#lib/workspace/api/http.ts";

export type WorkspaceMultiplexerFailure = {
  kind: "protocol" | "transport";
  message: string;
};

type Listener = {
  onFrame(frame: SubscriptionFrame): void;
  onStatus?(
    status: "connecting" | "open" | "closed",
    failure?: WorkspaceMultiplexerFailure,
  ): void;
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

import type { WorkspaceMessagingConnection } from "./messaging-transport.ts";
type MessagingListener = {
  onEvent(event: MessagingEvent): void;
  onStatus(status: "connecting" | "open" | "closed", failure?: WorkspaceMultiplexerFailure): void;
};
type PendingMessaging = {
  clientId: string;
  command: MessagingCommand;
  resolve(result: MessagingResult): void;
  reject(): void;
};

const multiplexers = new Map<string, WorkspaceMultiplexer>();
const INVALID_PROTOCOL_CLOSE_CODE = 4002;
let nextMultiplexerSequence = 0;

function nextMultiplexerId(kind: "client" | "request"): string {
  nextMultiplexerSequence += 1;
  return `${kind}-${nextMultiplexerSequence}`;
}

export function workspaceMultiplexer(
  workspaceId: string,
): WorkspaceMultiplexer {
  let multiplexer = multiplexers.get(workspaceId);
  if (!multiplexer) {
    multiplexer = new WorkspaceMultiplexer(workspaceId);
    multiplexers.set(workspaceId, multiplexer);
  }
  return multiplexer;
}

export function disposeWorkspaceMultiplexer(workspaceId: string): void {
  const multiplexer = multiplexers.get(workspaceId);
  if (!multiplexer) return;
  multiplexers.delete(workspaceId);
  multiplexer.dispose();
}

export class WorkspaceMultiplexer {
  readonly #workspaceId: string;
  readonly #subscriptions = new Map<string, ActiveSubscription>();
  readonly #requests = new Map<string, string>();
  readonly #runtimeSubscriptions = new Map<string, string>();
  readonly #messaging = new Map<string, MessagingListener>();
  readonly #messagingRequests = new Map<string, PendingMessaging>();
  readonly #messagingSubscriptions = new Map<string, string>();
  #socket: WebSocket | null = null;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #disconnectFailure: WorkspaceMultiplexerFailure | null = null;
  #closed = false;

  constructor(workspaceId: string) {
    this.#workspaceId = workspaceId;
  }

  subscribe(
    selector: EventSubscriptionSelector,
    listener: Listener,
  ): WorkspaceMultiplexerSubscription {
    const clientId = nextMultiplexerId("client");
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
      subscription.listener.onStatus?.("open");
      this.#sendSubscribe(subscription);
    } else {
      this.#ensureConnected();
    }
    return {
      close: () => this.#remove(clientId),
      sendWorkerMethod: (method) => this.#sendWorkerMethod(clientId, method),
    };
  }

  connectMessaging(listener: MessagingListener): WorkspaceMessagingConnection {
    const clientId = nextMultiplexerId("client");
    this.#messaging.set(clientId, listener);
    this.#closed = false;
    if (this.#socket?.readyState === WebSocket.OPEN) {
      queueMicrotask(() => { if (this.#messaging.has(clientId)) listener.onStatus("open"); });
    } else this.#ensureConnected();
    return {
      request: (command, signal) => this.#messagingRequest(clientId, command, signal),
      close: () => {
        for (const [id, owner] of this.#messagingSubscriptions) {
          if (owner !== clientId) continue;
          if (this.#socket?.readyState === WebSocket.OPEN) this.#send({ protocol_version: 2, frame: "messaging_request", message: { request_id: nextMultiplexerId("request"), command: { kind: "unsubscribe", subscription_id: id } } });
          this.#messagingSubscriptions.delete(id);
        }
        for (const pending of this.#messagingRequests.values()) if (pending.clientId === clientId) pending.reject();
        this.#messaging.delete(clientId);
        if (!this.#hasClients()) this.dispose();
      },
    };
  }

  #hasClients(): boolean { return this.#subscriptions.size > 0 || this.#messaging.size > 0; }

  #messagingRequest(clientId: string, command: MessagingCommand, signal: AbortSignal): Promise<MessagingResult> {
    if (!this.#messaging.has(clientId) || signal.aborted || this.#socket?.readyState !== WebSocket.OPEN || this.#messagingRequests.size >= 128) return Promise.reject(new Error("Messaging outcome unknown"));
    const requestId = nextMultiplexerId("request");
    return new Promise((resolve, reject) => {
      const cleanup = () => { clearTimeout(timer); signal.removeEventListener("abort", fail); this.#messagingRequests.delete(requestId); };
      const fail = () => { cleanup(); reject(new Error("Messaging outcome unknown")); };
      const timer = setTimeout(fail, 15000);
      signal.addEventListener("abort", fail, { once: true });
      this.#messagingRequests.set(requestId, { clientId, command, resolve: result => { cleanup(); resolve(result); }, reject: fail });
      this.#send({ protocol_version: 2, frame: "messaging_request", message: { request_id: requestId, command } });
    });
  }

  #receiveMessaging(response: MessagingResponse): void {
    const pending = this.#messagingRequests.get(response.request_id);
    if (!pending) {
      if (response.result.kind === "subscribed") this.#send({ protocol_version: 2, frame: "messaging_request", message: { request_id: nextMultiplexerId("request"), command: { kind: "unsubscribe", subscription_id: response.result.subscription_id } } });
      return;
    }
    const result = response.result;
    if (result.kind === "subscribed") {
      if (pending.command.kind !== "subscribe" || result.selector.subject_id !== pending.command.selector.subject_id || result.selector.conversation_id !== pending.command.selector.conversation_id || result.selector.after_sequence !== pending.command.selector.after_sequence) { this.#rejectInboundFrame(); return; }
      this.#messagingSubscriptions.set(result.subscription_id, pending.clientId);
    } else if (result.kind === "unsubscribed") this.#messagingSubscriptions.delete(result.subscription_id);
    pending.resolve(result);
  }

  #ensureConnected(): void {
    if (
      !browser || this.#socket || this.#closed || !this.#hasClients()
    ) return;
    for (const subscription of this.#subscriptions.values()) {
      subscription.listener.onStatus?.("connecting");
    }
    for (const listener of this.#messaging.values()) listener.onStatus("connecting");
    const url = new URL(
      workspaceApiPath(this.#workspaceId, "/protocol/ws"),
      window.location.origin,
    );
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(url);
    this.#socket = socket;
    socket.addEventListener("open", () => {
      if (this.#socket !== socket) return;
      for (const listener of this.#messaging.values()) listener.onStatus("open");
      for (const subscription of this.#subscriptions.values()) {
        subscription.listener.onStatus?.("open");
        this.#sendSubscribe(subscription);
      }
    });
    socket.addEventListener("message", (event) => {
      if (this.#socket !== socket) return;
      this.#receive(event.data);
    });
    socket.addEventListener("error", () => {
      if (this.#socket !== socket) return;
      socket.close();
    });
    socket.addEventListener("close", () => {
      if (this.#socket !== socket) return;
      this.#socket = null;
      const failure = this.#disconnectFailure ?? {
        kind: "transport" as const,
        message: "Workspace subscription disconnected",
      };
      this.#disconnectFailure = null;
      for (const pending of this.#messagingRequests.values()) pending.reject();
      this.#messagingSubscriptions.clear();
      for (const listener of this.#messaging.values()) listener.onStatus("closed", failure);
      this.#requests.clear();
      this.#runtimeSubscriptions.clear();
      for (const subscription of this.#subscriptions.values()) {
        subscription.requestId = null;
        subscription.subscriptionId = null;
        subscription.listener.onStatus?.("closed", failure);
      }
      if (!this.#closed && this.#hasClients()) {
        this.#reconnectTimer = setTimeout(() => this.#ensureConnected(), 500);
      }
    });
  }

  #sendSubscribe(subscription: ActiveSubscription): void {
    const requestId = nextMultiplexerId("request");
    subscription.requestId = requestId;
    this.#requests.set(requestId, subscription.clientId);
    this.#send({
      protocol_version: 2,
      frame: "request",
      message: {
        method: "subscribe_events",
        params: { request_id: requestId, selector: subscription.selector },
      },
    });
  }

  #receive(input: unknown): void {
    const decoded = decodeSubscriptionFrame(input);
    if (!decoded.ok) {
      this.#rejectInboundFrame();
      return;
    }
    const frame = decoded.value;
    if (frame.frame === "messaging_response") { this.#receiveMessaging(frame.message); return; }
    if (frame.frame === "messaging_event") {
      const clientId = this.#messagingSubscriptions.get(frame.message.subscription_id);
      if (clientId) this.#messaging.get(clientId)?.onEvent(frame.message);
      if (frame.message.kind === "closed") this.#messagingSubscriptions.delete(frame.message.subscription_id);
      return;
    }
    if (frame.frame === "response" && frame.message.result === "subscribed") {
      const clientId = this.#requests.get(frame.message.payload.request_id);
      const subscription = clientId
        ? this.#subscriptions.get(clientId)
        : undefined;
      if (!clientId || !subscription) return;
      if (!subscriptionFrameMatchesSelector(frame, subscription.selector)) {
        this.#rejectInboundFrame();
        return;
      }
      this.#requests.delete(frame.message.payload.request_id);
      const subscriptionId = frame.message.payload.subscription_id;
      subscription.subscriptionId = subscriptionId;
      this.#runtimeSubscriptions.set(subscriptionId, clientId);
      subscription.listener.onFrame(frame);
      return;
    }
    if (
      frame.frame === "response" &&
      frame.message.result === "subscription_rejected"
    ) {
      const clientId = this.#requests.get(frame.message.payload.request_id);
      const subscription = clientId
        ? this.#subscriptions.get(clientId)
        : undefined;
      subscription?.listener.onFrame(frame);
      if (clientId) {
        this.#requests.delete(frame.message.payload.request_id);
        this.#remove(clientId);
      }
      return;
    }
    if (frame.frame === "event") {
      const clientId = this.#runtimeSubscriptions.get(
        frame.message.data.subscription_id,
      );
      const subscription = clientId
        ? this.#subscriptions.get(clientId)
        : undefined;
      if (
        subscription &&
        !subscriptionFrameMatchesSelector(frame, subscription.selector)
      ) {
        this.#rejectInboundFrame();
        return;
      }
      subscription?.listener.onFrame(frame);
      if (
        frame.message.event === "subscription_closed" &&
        clientId &&
        subscription
      ) {
        this.#runtimeSubscriptions.delete(frame.message.data.subscription_id);
        subscription.subscriptionId = null;
        if (
          (frame.message.data.code === "lagged" ||
            frame.message.data.code === "server_shutdown") &&
          this.#socket?.readyState === WebSocket.OPEN
        ) {
          subscription.listener.onStatus?.("connecting");
          this.#sendSubscribe(subscription);
        } else {
          this.#remove(clientId);
        }
      }
    }
  }

  #rejectInboundFrame(): void {
    this.#disconnectFailure = {
      kind: "protocol",
      message: "Workspace protocol frame rejected",
    };
    this.#socket?.close(
      INVALID_PROTOCOL_CLOSE_CODE,
      "Invalid workspace protocol frame",
    );
  }

  #sendWorkerMethod(clientId: string, method: Method): void {
    const subscription = this.#subscriptions.get(clientId);
    if (!subscription?.subscriptionId) {
      throw new Error("Worker protocol subscription is not open");
    }
    this.#send({
      protocol_version: 2,
      frame: "worker_protocol",
      message: { subscription_id: subscription.subscriptionId, method },
    });
  }

  #remove(clientId: string): void {
    const subscription = this.#subscriptions.get(clientId);
    if (!subscription) return;
    this.#subscriptions.delete(clientId);
    if (
      subscription.subscriptionId && this.#socket?.readyState === WebSocket.OPEN
    ) {
      this.#send({
        protocol_version: 2,
        frame: "request",
        message: {
          method: "unsubscribe_events",
          params: {
            request_id: nextMultiplexerId("request"),
            subscription_id: subscription.subscriptionId,
          },
        },
      });
      this.#runtimeSubscriptions.delete(subscription.subscriptionId);
    }
    if (!this.#hasClients()) {
      this.#closed = true;
      if (this.#reconnectTimer) clearTimeout(this.#reconnectTimer);
      this.#socket?.close();
      this.#socket = null;
    }
  }

  dispose(): void {
    this.#closed = true;
    if (this.#reconnectTimer) {
      clearTimeout(this.#reconnectTimer);
      this.#reconnectTimer = null;
    }
    for (const pending of this.#messagingRequests.values()) pending.reject();
    for (const listener of this.#messaging.values()) listener.onStatus("closed");
    this.#messaging.clear();
    this.#messagingSubscriptions.clear();
    this.#subscriptions.clear();
    this.#requests.clear();
    this.#runtimeSubscriptions.clear();
    this.#disconnectFailure = null;
    this.#socket?.close();
    this.#socket = null;
  }

  #send(frame: SubscriptionFrame): void {
    if (this.#socket?.readyState !== WebSocket.OPEN) return;
    this.#socket.send(JSON.stringify(frame));
  }
}
