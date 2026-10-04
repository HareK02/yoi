export type ConsoleAttachmentFetchErrorKind =
  | "missing"
  | "expired"
  | "unauthorized"
  | "network"
  | "failed";

export class ConsoleAttachmentFetchError extends Error {
  constructor(
    public readonly kind: ConsoleAttachmentFetchErrorKind,
    message: string,
  ) {
    super(message);
    this.name = "ConsoleAttachmentFetchError";
  }
}

const successful = new Map<string, Blob>();
const inFlight = new Map<string, Promise<Blob>>();

export function loadConsoleAttachment(
  url: string,
  fetcher: typeof fetch = fetch,
): Promise<Blob> {
  const cached = successful.get(url);
  if (cached) return Promise.resolve(cached);
  const pending = inFlight.get(url);
  if (pending) return pending;

  const request = (async () => {
    let response: Response;
    try {
      response = await fetcher(url, { credentials: "same-origin" });
    } catch (error) {
      throw new ConsoleAttachmentFetchError(
        "network",
        error instanceof Error ? error.message : "Network request failed",
      );
    }
    if (!response.ok) {
      const kind: ConsoleAttachmentFetchErrorKind = response.status === 404
        ? "missing"
        : response.status === 410
        ? "expired"
        : response.status === 401 || response.status === 403
        ? "unauthorized"
        : "failed";
      throw new ConsoleAttachmentFetchError(kind, `HTTP ${response.status}`);
    }
    const blob = await response.blob();
    successful.set(url, blob);
    return blob;
  })().finally(() => inFlight.delete(url));
  inFlight.set(url, request);
  return request;
}

export function clearConsoleAttachmentCache(): void {
  successful.clear();
  inFlight.clear();
}
