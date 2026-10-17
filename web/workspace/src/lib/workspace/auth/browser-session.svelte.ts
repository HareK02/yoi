import { AuthRequestError, loadWhoami } from "./api.ts";
import { notifySessionChanged } from "./hosting.ts";
import type { RequestActor } from "./model.ts";

export type BrowserSession =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "ready"; actor: RequestActor | null };

export function authErrorMessage(error: unknown): string {
  if (error instanceof AuthRequestError) {
    if (error.status === 401) {
      return "Your session or authentication challenge expired. Log in again or restart the Passkey operation.";
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
  private request = 0;
  private disposed = false;
  private controller: AbortController | null = null;

  get actor(): RequestActor | null {
    return this.session.kind === "ready" ? this.session.actor : null;
  }

  async refresh(): Promise<void> {
    if (this.busy || this.disposed) return;
    await this.readSession();
  }

  private async readSession(): Promise<void> {
    const current = ++this.request;
    this.session = { kind: "loading" };
    try {
      const result = await loadWhoami();
      if (this.disposed || current !== this.request) return;
      this.session = { kind: "ready", actor: result.actor };
    } catch {
      if (this.disposed || current !== this.request) return;
      this.session = { kind: "error" };
    }
  }

  async run(
    pending: string,
    success: string,
    operation: (signal: AbortSignal) => Promise<unknown>,
  ): Promise<boolean> {
    if (this.busy || this.disposed || this.session.kind !== "ready") {
      return false;
    }
    this.busy = true;
    ++this.request;
    const controller = new AbortController();
    this.controller = controller;
    this.error = "";
    this.status = pending;
    try {
      await operation(controller.signal);
      if (this.disposed) return false;
      notifySessionChanged();
      await this.readSession();
      if (this.disposed) return false;
      const confirmed = this.session.kind === "ready";
      this.status = confirmed ? success : "";
      return confirmed;
    } catch (error) {
      if (this.disposed) return false;
      this.error = authErrorMessage(error);
      this.status = "";
      // 失効/応答不明でも既存actorを操作権限として保持せず、正本を読み直す。
      await this.readSession();
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
    ++this.request;
    this.controller?.abort();
  }
}
