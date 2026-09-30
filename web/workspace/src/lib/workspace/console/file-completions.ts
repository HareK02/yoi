import type { ComposerCompletionEntry } from "./composer-completion.ts";

type Request = {
  prefix: string;
  signal: AbortSignal;
  resolve: (entries: ComposerCompletionEntry[]) => void;
  reject: (error: Error) => void;
  abort: () => void;
};

/** The wire reply has no request id/prefix. Never overlap requests on a lane,
 * including cancelled requests: their reply must be consumed before the next.
 * A timeout poisons the lane until reconnect, rather than misrouting a late reply.
 */
export class FileCompletions {
  private active: Request | null = null;
  private queued: Request | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private failure: Error | null = null;

  constructor(
    private send: (prefix: string) => void,
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
    return new Promise((resolve, reject) => {
      const request: Request = {
        prefix,
        signal,
        resolve,
        reject,
        abort: () => {
          resolve([]);
          if (this.queued === request) this.queued = null;
        },
      };
      signal.addEventListener("abort", request.abort, { once: true });
      if (this.queued) this.finish(this.queued, []);
      this.queued = request;
      this.pump();
    });
  }

  receive(entries: ComposerCompletionEntry[]): void {
    if (!this.active) return;
    clearTimeout(this.timer);
    const request = this.active;
    this.active = null;
    this.finish(request, entries);
    this.pump();
  }

  close(error = new Error("Worker completion connection closed.")): void {
    this.failure = error;
    clearTimeout(this.timer);
    for (const request of [this.active, this.queued]) {
      if (!request) continue;
      request.signal.removeEventListener("abort", request.abort);
      request.reject(error);
    }
    this.active = this.queued = null;
  }

  reset(): void {
    this.close();
    this.failure = null;
  }

  private finish(request: Request, entries: ComposerCompletionEntry[]): void {
    request.signal.removeEventListener("abort", request.abort);
    request.resolve(request.signal.aborted ? [] : entries);
  }

  private pump(): void {
    if (this.active || !this.queued || this.failure) return;
    const request = this.queued;
    this.queued = null;
    if (request.signal.aborted) {
      this.finish(request, []);
      return;
    }
    this.active = request;
    this.timer = setTimeout(
      () =>
        this.close(
          new Error("Worker completion request timed out; reconnect to retry."),
        ),
      this.timeoutMs,
    );
    try {
      this.send(request.prefix);
    } catch (error) {
      this.close(error instanceof Error ? error : new Error(String(error)));
    }
  }
}
