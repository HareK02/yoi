/// <reference lib="dom" />
import { loadWhoami } from "./api.ts";
import type { WhoamiResponse } from "./model.ts";

export type SessionSnapshot =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "ready"; value: WhoamiResponse };

// 世代は権限ではなく、認証操作より前に開始した応答を捨てるためだけに使う。
// Cookieの実状態は常にwhoamiが所有し、tab間通知や一般401からexpiredを推測しない。
const GENERATION_KEY = "yoi.browser-auth-generation";
export class SessionState {
  snapshot: SessionSnapshot = { kind: "loading" };
  private generation = 0;
  private pending: Promise<SessionSnapshot> | null = null;
  private listeners = new Set<(state: SessionSnapshot) => void>();
  private timer: ReturnType<typeof setTimeout> | undefined;

  get authenticationGeneration(): number {
    return this.generation;
  }

  constructor(private readonly read = loadWhoami) {}

  subscribe(listener: (state: SessionSnapshot) => void): () => void {
    this.listeners.add(listener);
    listener(this.snapshot);
    return () => this.listeners.delete(listener);
  }

  private publish(snapshot: SessionSnapshot): void {
    this.snapshot = snapshot;
    clearTimeout(this.timer);
    if (
      snapshot.kind === "ready" &&
      snapshot.value.session.kind === "authenticated"
    ) {
      const expiry = snapshot.value.session.expires_at;
      if (expiry !== null) {
        // Timerは再照会の契機。local clockだけで期限切れと断定しない。
        const remaining = Date.parse(expiry) - Date.now();
        this.timer = setTimeout(() => {
          void this.refresh();
        }, Math.min(2_147_483_647, Math.max(1_000, remaining)));
      }
    }
    for (const listener of this.listeners) listener(snapshot);
  }

  invalidate(): void {
    ++this.generation;
    this.pending = null;
    this.publish({ kind: "loading" });
  }

  async refresh(): Promise<SessionSnapshot> {
    if (this.pending) return this.pending;
    const generation = this.generation;
    const stamp = storageStamp();
    this.publish({ kind: "loading" });
    const pending = (async () => {
      let result: SessionSnapshot;
      try {
        result = { kind: "ready", value: await this.read() };
      } catch {
        result = { kind: "error" };
      }
      if (generation !== this.generation || stamp !== storageStamp()) {
        // 新世代が既に確定していれば再取得すら不要。古い完了で確定結果を巻き戻さない。
        if (generation !== this.generation) {
          if (this.pending) return await this.pending;
          if (this.snapshot.kind !== "loading") return this.snapshot;
        }
        // 別tabの操作がstorage eventより先に完了していても古い応答を採用しない。
        if (generation === this.generation) this.invalidate();
        return await this.refresh();
      }
      this.pending = null;
      this.publish(result);
      return result;
    })();
    this.pending = pending;
    return pending;
  }

  dispose(): void {
    clearTimeout(this.timer);
    this.listeners.clear();
    ++this.generation;
  }
}

function storageStamp(): string | null {
  try {
    return globalThis.localStorage?.getItem(GENERATION_KEY) ?? null;
  } catch {
    return null;
  }
}

export const browserSession = new SessionState();

export function sessionMutation(
  reason: "authentication" | "logout" = "authentication",
): void {
  browserSession.invalidate();
  const message = JSON.stringify({ id: crypto.randomUUID(), reason });
  try {
    localStorage.setItem(GENERATION_KEY, message);
  } catch { /* 保存拒否でも同tab fenceは維持。 */ }
  if (reason === "logout") clearReturnTo();
}

export const CLEAR_RETURN_EVENT = "yoi:clear-auth-return";
function clearReturnTo(): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(CLEAR_RETURN_EVENT));
  }
}

let installed = false;
export function recheckAfterUnauthorized(
  authenticationGeneration: number,
): void {
  if (
    installed &&
    authenticationGeneration === browserSession.authenticationGeneration
  ) void browserSession.refresh();
}
export function observeBrowserSession(): void {
  if (installed || typeof window === "undefined") return;
  installed = true;
  const refresh = () => {
    void browserSession.refresh();
  };
  window.addEventListener("focus", refresh);
  window.addEventListener("pageshow", refresh);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) refresh();
  });
  window.addEventListener("storage", (event) => {
    if (event.key !== GENERATION_KEY && event.key !== null) return;
    browserSession.invalidate();
    try {
      if (
        event.key === null ||
        JSON.parse(event.newValue ?? "null")?.reason === "logout"
      ) clearReturnTo();
    } catch { /* 不明な通知も権限ではなく再照会だけ。 */ }
    refresh();
  });
}
