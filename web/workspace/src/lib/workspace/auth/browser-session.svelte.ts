import { AuthRequestError } from "./api.ts";
import {
  browserSession,
  observeBrowserSession,
  sessionMutation,
} from "./session-state.ts";
import { notifySessionChanged } from "./hosting.ts";
import type { RequestActor, WhoamiResponse } from "./model.ts";

export type BrowserSession =
  | { kind: "loading" }
  | { kind: "error" }
  | ({ kind: "ready" } & WhoamiResponse);

export function authErrorMessage(error: unknown): string {
  if (error instanceof AuthRequestError) {
    if (error.status === 401) {
      return "Authentication request was rejected. Check your session or restart the Passkey operation.";
    }
    if (error.status === 400 || error.status === 410) {
      return "The request or authentication challenge is invalid or expired. Restart the operation.";
    }
    return error.message;
  }
  if (error instanceof DOMException && error.name === "NotSupportedError") {
    return "This browser or key cannot complete this passkey operation. Use a supported device or key with saved passkeys and PIN or biometric verification. Completion is not confirmed.";
  }
  if (error instanceof DOMException && error.name === "NotAllowedError") {
    return "Passkey operation was cancelled or timed out, or no matching key was found. Completion is not confirmed. Use a supported device and try again.";
  }
  if (error instanceof DOMException && error.name === "AbortError") {
    return "Operation stopped. Completion is not confirmed; check your session before retrying.";
  }
  // Browser/transport例外の本文を表示せず、credentialや外部応答を診断へ複写しない。
  return "Authentication could not be confirmed. Check your session before retrying the operation.";
}

export class BrowserAuth {
  session = $state<BrowserSession>({ kind: "loading" });
  busy = $state(false);
  error = $state("");
  status = $state("");
  private unsubscribe: () => void;
  private disposed = false;
  private controller: AbortController | null = null;

  constructor() {
    observeBrowserSession();
    this.unsubscribe = browserSession.subscribe((snapshot) => {
      // 操作pendingはsession確認中と別。formを消してfocus/入力を失わず、busyで再操作を止める。
      if (this.busy && snapshot.kind === "loading") return;
      if (!this.disposed) {
        this.session = snapshot.kind === "ready"
          ? { kind: "ready", ...snapshot.value }
          : snapshot;
      }
    });
  }

  get actor(): RequestActor | null {
    return this.session.kind === "ready" ? this.session.actor : null;
  }

  async refresh(): Promise<void> {
    if (this.busy || this.disposed) return;
    await browserSession.refresh();
  }

  async run(
    pending: string,
    success: string,
    operation: (signal: AbortSignal) => Promise<unknown>,
    intent: "authentication" | "logout" = "authentication",
  ): Promise<boolean> {
    if (
      this.busy || this.disposed ||
      (intent !== "logout" && this.session.kind !== "ready")
    ) {
      return false;
    }
    this.busy = true;
    sessionMutation(intent);
    const authenticationGeneration = browserSession.authenticationGeneration;
    const controller = new AbortController();
    this.controller = controller;
    this.error = "";
    this.status = pending;
    try {
      await operation(controller.signal);
      const superseded =
        authenticationGeneration !== browserSession.authenticationGeneration;
      sessionMutation(intent);
      const confirmationGeneration = browserSession.authenticationGeneration;
      await browserSession.refresh();
      if (
        this.disposed || superseded ||
        confirmationGeneration !== browserSession.authenticationGeneration
      ) {
        if (!this.disposed) this.status = "";
        return false;
      }
      notifySessionChanged();
      const confirmed = this.session.kind === "ready";
      this.status = confirmed ? success : "";
      return confirmed;
    } catch (error) {
      if (!this.disposed) {
        this.error = authErrorMessage(error);
        this.status = "";
      }
      // 失効/応答不明でも既存actorを操作権限として保持せず、正本を読み直す。
      sessionMutation(intent);
      await browserSession.refresh();
      return false;
    } finally {
      if (!this.disposed) {
        this.busy = false;
        this.controller = null;
      }
    }
  }

  dispose(): void {
    this.disposed = true;
    this.unsubscribe();
    this.controller?.abort();
  }
}
