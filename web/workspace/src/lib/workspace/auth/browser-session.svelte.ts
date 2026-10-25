import type { PasskeyAdditionResultRequest } from "#lib/generated/auth-api.ts";
import { AuthRequestError, loadWhoami, readAdditionResult } from "./api.ts";
import {
  browserSession,
  observeBrowserSession,
  sessionMutation,
} from "./session-state.ts";
import { notifySessionChanged } from "./hosting.ts";
import type { AuthUser, RequestActor, WhoamiResponse } from "./model.ts";
import {
  AuthOperationError,
  type AuthStage,
  observeAuthOperation,
  operationErrorMessage,
} from "./operation.ts";

export type BrowserSession =
  | { kind: "loading" }
  | { kind: "error" }
  | ({ kind: "ready" } & WhoamiResponse);

export function authErrorMessage(error: unknown): string {
  if (error instanceof AuthOperationError) return operationErrorMessage(error);
  if (error instanceof AuthRequestError) {
    return `Authentication request was rejected (${error.status}). Check your session or restart the operation.`;
  }
  // 例外本文/causeや外部応答は表示にも診断にも複写しない。
  return "Authentication could not be confirmed. Check your session and the operation result before retrying.";
}
function sameUser(a: AuthUser | null, b: AuthUser | null): boolean {
  return a === null
    ? b === null
    : b !== null && a.user_id === b.user_id && a.account_id === b.account_id;
}
function sameSession(a: WhoamiResponse, b: WhoamiResponse): boolean {
  // anonymous/expiredはいずれも認証主体もbindingもない。別tabのlogoutでexpiredの
  // 表示だけが古くなっても、新たな明示loginを拒否しない。認証済みへの変化は拒否する。
  if (
    !a.actor && !b.actor && a.session.kind !== "authenticated" &&
    b.session.kind !== "authenticated"
  ) return true;
  return sameUser(a.actor, b.actor) && a.session.kind === b.session.kind &&
    (a.session.kind !== "authenticated" ||
      a.authentication_reference === b.authentication_reference &&
        b.session.kind === "authenticated" &&
        a.session.expires_at === b.session.expires_at &&
        a.actor?.auth_method === b.actor?.auth_method);
}
function returnedUser(value: unknown): AuthUser | null {
  if (typeof value !== "object" || value === null) return null;
  const user = (value as { user?: Partial<AuthUser> }).user;
  return user && typeof user.user_id === "string" &&
      typeof user.account_id === "string" && typeof user.handle === "string" &&
      typeof user.display_name === "string"
    ? user as AuthUser
    : null;
}
export class BrowserAuth {
  session = $state<BrowserSession>({ kind: "loading" });
  busy = $state(false);
  error = $state("");
  status = $state("");
  additionAttempt = $state<PasskeyAdditionResultRequest | null>(null);
  additionSaved = $state(false);
  private unsubscribe: () => void;
  private disposed = false;
  private controller: AbortController | null = null;
  private operationGeneration: number | null = null;

  constructor() {
    observeBrowserSession();
    this.unsubscribe = browserSession.subscribe((snapshot) => {
      if (
        this.operationGeneration !== null &&
        this.operationGeneration !== browserSession.authenticationGeneration
      ) this.controller?.abort();
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
  async ensure(): Promise<void> {
    if (!this.busy && !this.disposed) await browserSession.ensure();
  }
  async refresh(): Promise<void> {
    if (!this.busy && !this.disposed) await browserSession.refresh();
  }
  cancel(): void {
    this.controller?.abort();
  }
  async run(
    pending: string,
    success: string,
    operation: (signal: AbortSignal) => Promise<unknown>,
    intent: "authentication" | "logout" | "device-approval" = "authentication",
  ): Promise<boolean> {
    if (
      this.busy || this.disposed ||
      (intent !== "logout" && this.session.kind !== "ready")
    ) return false;
    const mutationReason = intent === "logout" ? "logout" : "authentication";
    const expected = this.session.kind === "ready" ? this.session : null;
    this.busy = true;
    sessionMutation(mutationReason, true);
    const generation = browserSession.authenticationGeneration;
    const stamp = browserSession.authenticationStamp;
    let acceptedGeneration = generation;
    let acceptedStamp = stamp;
    let submittedAddition = false;
    const controller = new AbortController();
    this.controller = controller;
    this.operationGeneration = generation;
    this.error = "";
    this.status = pending;
    const verifySession = async () => {
      if (stamp !== browserSession.authenticationStamp) controller.abort();
      controller.signal.throwIfAborted();
      let current: WhoamiResponse;
      try {
        current = await loadWhoami(controller.signal);
      } catch {
        throw new AuthOperationError(
          "session-check",
          "session-changed",
          "not-sent",
        );
      }
      if (stamp !== browserSession.authenticationStamp) controller.abort();
      controller.signal.throwIfAborted();
      if (!expected || !sameSession(expected, current)) {
        throw new AuthOperationError(
          "session-check",
          "session-changed",
          "not-sent",
        );
      }
    };
    observeAuthOperation(controller.signal, {
      verifySession,
      expected,
      additionAttempt: (attempt) => {
        submittedAddition = true;
        this.additionAttempt = attempt;
        this.additionSaved = false;
      },
      phase: (stage: AuthStage) => {
        if (!this.disposed) {
          this.status = stage === "authenticator"
            ? pending
            : stage === "complete"
            ? "Confirming the operation with the server…"
            : stage === "before-complete"
            ? "Checking the account before completion…"
            : stage === "options"
            ? "Checking the account and requesting passkey options…"
            : "Preparing the authenticator response…";
        }
      },
    });
    try {
      if (intent === "device-approval") await verifySession();
      const result = await operation(controller.signal);
      const user = returnedUser(result);
      const issuedReference = typeof result === "object" && result !== null
        ? (result as { authentication_reference?: unknown })
          .authentication_reference
        : null;
      controller.signal.throwIfAborted();
      if (
        this.disposed ||
        generation !== browserSession.authenticationGeneration ||
        stamp !== browserSession.authenticationStamp
      ) return false;
      // 自身の再確認によるinvalidateと、他の操作が行ったinvalidateを混同しない。
      this.operationGeneration = null;
      sessionMutation(mutationReason, true);
      const confirmationGeneration = browserSession.authenticationGeneration;
      acceptedGeneration = confirmationGeneration;
      const confirmationStamp = browserSession.authenticationStamp;
      acceptedStamp = confirmationStamp;
      this.status = "Confirming the expected browser session…";
      await browserSession.refresh();
      if (
        this.disposed || controller.signal.aborted ||
        confirmationGeneration !== browserSession.authenticationGeneration ||
        confirmationStamp !== browserSession.authenticationStamp
      ) {
        if (!this.disposed) this.status = "";
        return false;
      }
      const snapshot = browserSession.snapshot;
      const confirmed = snapshot.kind === "ready" &&
        (intent === "logout"
          ? snapshot.value.actor === null &&
            snapshot.value.session.kind !== "authenticated"
          : intent === "device-approval"
          ? snapshot.value.session.kind === "authenticated" && user !== null &&
            sameUser(user, expected?.actor ?? null) &&
            sameUser(user, snapshot.value.actor) &&
            snapshot.value.authentication_reference ===
              expected?.authentication_reference &&
            typeof result === "object" && result !== null &&
            (result as { status?: unknown }).status === "approved"
          : snapshot.value.session.kind === "authenticated" &&
            snapshot.value.actor?.auth_method === "browser_session" &&
            user !== null && typeof issuedReference === "string" &&
            issuedReference === snapshot.value.authentication_reference &&
            sameUser(user, snapshot.value.actor) &&
            (!expected?.actor || sameUser(expected.actor, user)));
      if (!confirmed) {
        throw new AuthOperationError(
          "confirmation",
          "session-changed",
          "saved-session-unconfirmed",
        );
      }
      notifySessionChanged();
      if (submittedAddition) this.additionSaved = true;
      this.status = success;
      return true;
    } catch (error) {
      if (!this.disposed) {
        this.error = authErrorMessage(error);
        this.status = "";
      }
      // 古い処理から新しいログアウト/主体確認をinvalidateしない。
      if (
        !this.disposed &&
        acceptedGeneration === browserSession.authenticationGeneration &&
        acceptedStamp === browserSession.authenticationStamp
      ) {
        this.operationGeneration = null;
        sessionMutation(mutationReason, true);
        await browserSession.refresh();
      }
      return false;
    } finally {
      this.operationGeneration = null;
      this.controller = null;
      if (!this.disposed) this.busy = false;
    }
  }
  async checkAddition(): Promise<void> {
    const attempt = this.additionAttempt;
    if (!attempt || this.busy || this.disposed) return;
    this.busy = true;
    this.error = "";
    this.status = "Checking the saved addition result…";
    // 保存の履歴は失敗に戻さないが、過去の照合で再追加を許可しない。
    this.additionSaved = false;
    const generation = browserSession.authenticationGeneration;
    const stamp = browserSession.authenticationStamp;
    const controller = new AbortController();
    this.controller = controller;
    this.operationGeneration = generation;
    const assertCurrent = () => {
      controller.signal.throwIfAborted();
      if (
        this.disposed ||
        generation !== browserSession.authenticationGeneration ||
        stamp !== browserSession.authenticationStamp
      ) {
        throw new AuthOperationError(
          "confirmation",
          "session-changed",
          "unknown",
        );
      }
    };
    try {
      const before = await loadWhoami(controller.signal);
      assertCurrent();
      if (
        !before.actor || before.actor.user_id !== attempt.expected_user_id ||
        before.actor.account_id !== attempt.expected_account_id
      ) {
        throw new AuthOperationError(
          "confirmation",
          "session-changed",
          "unknown",
        );
      }
      const result = await readAdditionResult(attempt, controller.signal);
      assertCurrent();
      const after = await loadWhoami(controller.signal);
      assertCurrent();
      if (!sameSession(before, after)) {
        throw new AuthOperationError(
          "confirmation",
          "session-changed",
          "unknown",
        );
      }
      if (result.status === "saved" && sameUser(result.user, before.actor)) {
        this.additionSaved = true;
        this.status =
          "This addition saved a key to your account. Your current session was checked; the session issued by completion is not confirmed. Test ID-free login with that key before retiring older-key access.";
      } else {this.status =
          "The addition result is still unknown. Completion may still be in flight. Do not add another key yet; check again or use the registered-key list and ID-free login to verify access.";}
    } catch {
      if (!this.disposed) {
        this.error =
          "The addition result could not be read with the current account/session. This does not prove failure. Return to the intended account and check its key list or ID-free login; do not recreate the original session or add blindly.";
        this.status = "";
      }
    } finally {
      this.operationGeneration = null;
      this.controller = null;
      if (!this.disposed) this.busy = false;
    }
  }
  dispose(): void {
    this.disposed = true;
    this.unsubscribe();
    this.controller?.abort();
  }
}
