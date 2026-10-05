import type { ComposerCompletionEntry } from "./composer-completion.ts";

type Request = {
  id: string;
  prefix: string;
  signal: AbortSignal;
  resolve: (entries: ComposerCompletionEntry[]) => void;
  reject: (error: Error) => void;
  abort: () => void;
};

/** A lane is scoped to a transport target and completion kind/context. Each
 * request also has a unique wire nonce: cancelled requests never need draining,
 * and legacy replies or another client's same-prefix broadcasts cannot match.
 * The editor's abort signal fences the draft, cursor and completion generation.
 */
export class FileCompletions {
  private active: Request | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private failure: Error | null = null;

  constructor(
    private send: (prefix: string, requestId: string) => void,
    private timeoutMs = 30_000,
  ) {}

  get pending(): boolean {
    return this.active !== null;
  }

  request(
    prefix: string,
    signal: AbortSignal,
  ): Promise<ComposerCompletionEntry[]> {
    if (signal.aborted) return Promise.resolve([]);
    if (this.failure) return Promise.reject(this.failure);
    this.finish([]);
    return new Promise((resolve, reject) => {
      const request: Request = {
        id: crypto.randomUUID(),
        prefix,
        signal,
        resolve,
        reject,
        abort: () => {
          if (this.active === request) this.finish([]);
        },
      };
      this.active = request;
      signal.addEventListener("abort", request.abort, { once: true });
      this.timer = setTimeout(() => {
        if (this.active === request) {
          this.reject(new Error("Worker completion request timed out; retry."));
        }
      }, this.timeoutMs);
      try {
        this.send(prefix, request.id);
      } catch (error) {
        this.reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  receive(
    entries: ComposerCompletionEntry[],
    prefix: string,
    requestId?: string | null,
  ): void {
    if (
      !this.active || this.active.id !== requestId ||
      this.active.prefix !== prefix
    ) return;
    this.finish(entries);
  }

  close(error = new Error("Worker completion connection closed.")): void {
    this.failure = error;
    this.reject(error);
  }

  reset(): void {
    this.close();
    this.failure = null;
  }

  private take(): Request | null {
    clearTimeout(this.timer);
    const request = this.active;
    this.active = null;
    request?.signal.removeEventListener("abort", request.abort);
    return request;
  }

  private finish(entries: ComposerCompletionEntry[]): void {
    const request = this.take();
    request?.resolve(request.signal.aborted ? [] : entries);
  }

  private reject(error: Error): void {
    this.take()?.reject(error);
  }
}
