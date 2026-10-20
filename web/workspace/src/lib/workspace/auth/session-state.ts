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
  private stale = true;

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
    for (const listener of this.listeners) listener(snapshot);
  }

  invalidate(preserveSnapshot = false): void {
    ++this.generation;
    this.pending = null;
    this.stale = true;
    if (!preserveSnapshot) this.publish({ kind: "loading" });
  }

  // Page load owns freshness. Components mounted after it reuse that result.
  async ensure(): Promise<SessionSnapshot> {
    if (this.pending) return this.pending;
    if (!this.stale && this.snapshot.kind === "ready") return this.snapshot;
    return this.refresh();
  }

  async refresh(): Promise<SessionSnapshot> {
    if (this.pending) return this.pending;
    const generation = this.generation;
    const stamp = storageStamp();
    // Keep the admitted page visible while a new page access is checked.
    if (this.snapshot.kind !== "ready") this.publish({ kind: "loading" });
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
          if (!this.stale && this.snapshot.kind !== "loading") {
            return this.snapshot;
          }
        }
        // 別tabの操作がstorage eventより先に完了していても古い応答を採用しない。
        if (generation === this.generation) this.invalidate();
        return await this.refresh();
      }
      this.pending = null;
      this.stale = false;
      this.publish(result);
      return result;
    })();
    this.pending = pending;
    return pending;
  }

  dispose(): void {
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
export function observeBrowserSession(): void {
  if (installed || typeof window === "undefined") return;
  installed = true;
  window.addEventListener("storage", (event) => {
    if (event.key !== GENERATION_KEY && event.key !== null) return;
    // Fence old responses, but leave this page alone until the next page access.
    browserSession.invalidate(true);
    try {
      if (
        event.key === null ||
        JSON.parse(event.newValue ?? "null")?.reason === "logout"
      ) clearReturnTo();
    } catch { /* 不明な通知でも次のページアクセス時に確認する。 */ }
  });
}
