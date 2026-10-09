import type { DriveMutationResponse } from "#lib/generated/drive-api.ts";
import {
  createDriveClient,
  DRIVE_RESPONSE_MAX_BYTES,
  DRIVE_TEXT_MAX_BYTES,
  type DriveClient,
  type DriveEntry,
  type DriveEntryRef,
  type DriveMutation,
  DriveRequestError,
  type DriveUploadTarget,
  parseDriveEntryRef,
} from "./api.ts";

export type DriveDraft = {
  text: string;
  baseText: string;
  expectedRevision: string;
  contentType: string;
  conflict: boolean;
};
export type DriveReceipt = {
  requestId: string;
  state: "pending" | "committed" | "not_committed" | "unknown";
  operation: DriveMutation["operation"] | "upload";
  error: string | null;
  response: DriveMutationResponse | null;
};
export type DriveControllerState = {
  workspaceId: string | null;
  entry: DriveEntry | null;
  entries: DriveEntry[];
  nextAfter: string | null;
  search: { query: string; includeText: boolean } | null;
  text: string | null;
  truncated: boolean;
  image: Blob | null;
  draft: DriveDraft | null;
  receipts: DriveReceipt[];
  loading: boolean;
  error: string | null;
};
type ReceiptRecord = {
  receipt: DriveReceipt;
  saved?: { text: string; revision: string };
};
const empty = (): DriveControllerState => ({
  workspaceId: null,
  entry: null,
  entries: [],
  nextAfter: null,
  search: null,
  text: null,
  truncated: false,
  image: null,
  draft: null,
  receipts: [],
  loading: false,
  error: null,
});
function key(ws: string, ref: DriveEntryRef): string {
  return JSON.stringify([ws, ref.node_id]);
}
function message(error: unknown): string {
  return error instanceof DriveRequestError
    ? error.message
    : "Drive request could not be completed";
}
/** A subscribable snapshot store; Svelte consumers may use `$controller` directly.
 * All asynchronous publication is fenced by selection generation. Aborting a
 * write does not establish failure: its receipt is retained for reconciliation.
 */
export class DriveController {
  private value = empty();
  private listeners = new Set<(state: DriveControllerState) => void>();
  private drafts = new Map<string, DriveDraft>();
  private receipts = new Map<string, Map<string, ReceiptRecord>>();
  private generation = 0;
  private pageGeneration = 0;
  private abort = new AbortController();
  private writeAbort = new AbortController();
  private writeGeneration = 0;
  private disposed = false;
  constructor(
    private readonly client: DriveClient = createDriveClient(),
    private readonly newRequestId: () => string = () => crypto.randomUUID(),
  ) {}
  get state(): DriveControllerState {
    return this.value;
  }
  subscribe(listener: (state: DriveControllerState) => void): () => void {
    this.listeners.add(listener);
    listener(this.value);
    return () => this.listeners.delete(listener);
  }
  private publish(patch: Partial<DriveControllerState>): void {
    this.value = { ...this.value, ...patch };
    for (const listener of this.listeners) listener(this.value);
  }
  private current(generation: number): boolean {
    return !this.disposed && generation === this.generation;
  }
  private selection(): {
    ws: string;
    entry: DriveEntry;
    key: string;
    generation: number;
    signal: AbortSignal;
  } {
    if (this.disposed || !this.value.workspaceId || !this.value.entry) {
      throw new Error("Select a Drive entry first");
    }
    return {
      ws: this.value.workspaceId,
      entry: this.value.entry,
      key: key(this.value.workspaceId, this.value.entry.entry),
      generation: this.generation,
      signal: this.abort.signal,
    };
  }
  private currentReceipts(k: string): DriveReceipt[] {
    return [...(this.receipts.get(k)?.values() ?? [])].map((record) =>
      record.receipt
    );
  }
  private cancelWrites(reason: string): void {
    ++this.writeGeneration;
    for (const records of this.receipts.values()) {
      for (const record of records.values()) {
        if (record.receipt.state === "pending") {
          record.receipt = {
            ...record.receipt,
            state: "unknown",
            error: reason,
          };
        }
      }
    }
    this.writeAbort.abort();
    this.writeAbort = new AbortController();
  }
  /** Cancel active writes only. Cancellation is not proof of noncommit, so the
   * receipt and draft are retained and the next action must be status lookup.
   * Read/preview/search signals and selection identity are left untouched.
   */
  cancelPending(): void {
    if (this.disposed) return;
    if (
      !this.currentReceiptsForSelection().some((r) => r.state === "pending")
    ) return;
    this.cancelWrites(
      "Transfer cancelled; outcome unknown; check request status",
    );
    const s = this.selection();
    this.publish({ receipts: this.currentReceipts(s.key) });
  }
  private currentReceiptsForSelection(): DriveReceipt[] {
    const { workspaceId, entry } = this.value;
    return workspaceId && entry
      ? this.currentReceipts(key(workspaceId, entry.entry))
      : [];
  }
  private fence(): void {
    // Late promises must never publish into a reselected identity, even A -> B -> A.
    ++this.generation;
    ++this.pageGeneration;
    this.cancelWrites("Selection changed; check request status");
    this.abort.abort();
    this.abort = new AbortController();
  }
  async select(workspaceId: string | null, ref?: DriveEntryRef): Promise<void> {
    if (this.disposed) return;
    if (ref && workspaceId) parseDriveEntryRef(ref, workspaceId);
    this.fence();
    const generation = this.generation;
    const pageGeneration = this.pageGeneration;
    const signal = this.abort.signal;
    this.publish({ ...empty(), workspaceId, loading: workspaceId !== null });
    if (!workspaceId) return;
    try {
      const entry = ref
        ? await this.client.metadata(workspaceId, ref, signal)
        : await this.client.root(workspaceId, signal);
      if (!this.current(generation)) return;
      const k = key(workspaceId, entry.entry);
      this.publish({
        entry,
        draft: isText(entry) ? this.drafts.get(k) ?? null : null,
        receipts: this.currentReceipts(k),
      });
      if (entry.kind === "folder") {
        const result = await this.client.list(
          workspaceId,
          entry.entry,
          {},
          signal,
        );
        if (
          !this.current(generation) || pageGeneration !== this.pageGeneration
        ) return;
        this.publish({ entries: result.entries, nextAfter: result.next_after });
      } else if (isText(entry)) {
        const result = await this.client.readText(
          workspaceId,
          entry.entry,
          signal,
        );
        if (!this.current(generation)) return;
        let draft = this.drafts.get(k) ?? null;
        // Clean acknowledged drafts may refresh, but dirty/conflicted drafts
        // and unresolved requests must keep their original CAS revision.
        const unresolved = this.currentReceipts(k).some((receipt) =>
          receipt.state === "pending" || receipt.state === "unknown"
        );
        const refreshable = !draft ||
          (draft.text === draft.baseText && !draft.conflict && !unresolved);
        if (refreshable && !unresolved) {
          draft = null;
          this.drafts.delete(k);
        }
        if (
          !draft && !unresolved && isText(result.entry) && !result.truncated &&
          (result.entry.size ?? Infinity) <= DRIVE_TEXT_MAX_BYTES
        ) {
          draft = {
            text: result.text,
            baseText: result.text,
            expectedRevision: result.entry.revision,
            contentType: result.entry.content_type ?? "text/plain",
            conflict: false,
          };
          this.drafts.set(k, draft);
        }
        this.publish({
          entry: result.entry,
          text: isText(result.entry) ? result.text : null,
          truncated: result.truncated,
          draft: isText(result.entry) ? draft : null,
        });
      } else if (
        isImage(entry) && (entry.size ?? Infinity) <= DRIVE_RESPONSE_MAX_BYTES
      ) {
        const image = await this.client.image(workspaceId, entry, signal);
        if (!this.current(generation)) return;
        this.publish({ image });
      }
      if (this.current(generation) && pageGeneration === this.pageGeneration) {
        this.publish({ loading: false });
      }
    } catch (error) {
      if (this.current(generation) && pageGeneration === this.pageGeneration) {
        this.publish({ loading: false, error: message(error) });
      }
    }
  }
  async refresh(): Promise<void> {
    const s = this.selection();
    await this.select(s.ws, s.entry.entry);
  }
  async search(query: string, includeText = false): Promise<void> {
    const s = this.selection();
    const pageGeneration = ++this.pageGeneration;
    this.publish({
      search: { query, includeText },
      entries: [],
      nextAfter: null,
      loading: true,
      error: null,
    });
    try {
      const result = await this.client.search(
        s.ws,
        query,
        includeText,
        {},
        s.signal,
      );
      if (
        !this.current(s.generation) || pageGeneration !== this.pageGeneration
      ) return;
      this.publish({
        entries: result.entries,
        nextAfter: result.next_after,
        loading: false,
      });
    } catch (error) {
      if (
        this.current(s.generation) && pageGeneration === this.pageGeneration
      ) this.publish({ loading: false, error: message(error) });
    }
  }
  async loadMore(): Promise<void> {
    const s = this.selection();
    const after = this.value.nextAfter;
    if (!after || this.value.loading) return;
    const search = this.value.search;
    const pageGeneration = ++this.pageGeneration;
    this.publish({ loading: true, error: null });
    try {
      const result = search
        ? await this.client.search(s.ws, search.query, search.includeText, {
          after,
        }, s.signal)
        : await this.client.list(s.ws, s.entry.entry, { after }, s.signal);
      if (
        !this.current(s.generation) || pageGeneration !== this.pageGeneration
      ) return;
      const entries = new Map(
        this.value.entries.map((entry) => [entry.entry.node_id, entry]),
      );
      for (const entry of result.entries) {
        entries.set(entry.entry.node_id, entry);
      }
      this.publish({
        entries: [...entries.values()],
        nextAfter: result.next_after,
        loading: false,
      });
    } catch (error) {
      if (
        this.current(s.generation) && pageGeneration === this.pageGeneration
      ) this.publish({ loading: false, error: message(error) });
    }
  }
  edit(text: string): void {
    const s = this.selection();
    const existing = this.drafts.get(s.key);
    if (!existing || !isText(s.entry)) {
      throw new Error("This file has no editable bounded text draft");
    }
    const draft = { ...existing, text };
    this.drafts.set(s.key, draft);
    this.publish({ draft });
  }
  /** Explicit user discard/reload, never performed as conflict recovery. */
  async discardDraft(): Promise<void> {
    const s = this.selection();
    if (
      this.currentReceipts(s.key).some((r) =>
        r.state === "unknown" || r.state === "pending"
      )
    ) throw new Error("Reconcile the pending request before discarding");
    this.drafts.delete(s.key);
    await this.refresh();
  }
  async save(): Promise<string> {
    const s = this.selection();
    const draft = this.drafts.get(s.key);
    if (!draft || !isText(s.entry)) throw new Error("No editable draft");
    const signal = this.writeAbort.signal;
    return await this.write(
      "update_text",
      (id) =>
        this.client.mutate(s.ws, id, {
          operation: "update_text",
          id: s.entry.entry,
          expected_revision: draft.expectedRevision,
          text: draft.text,
          content_type: draft.contentType,
        }, signal),
      { text: draft.text, revision: draft.expectedRevision },
    );
  }
  async mutate(mutation: DriveMutation): Promise<string> {
    const s = this.selection();
    const signal = this.writeAbort.signal;
    return await this.write(
      mutation.operation,
      (id) => this.client.mutate(s.ws, id, mutation, signal),
    );
  }
  async upload(blob: Blob, target: DriveUploadTarget): Promise<string> {
    const s = this.selection();
    const signal = this.writeAbort.signal;
    return await this.write(
      "upload",
      (id) => this.client.upload(s.ws, id, blob, target, signal),
    );
  }
  private async write(
    operation: DriveReceipt["operation"],
    send: (id: string) => Promise<DriveMutationResponse>,
    saved?: ReceiptRecord["saved"],
  ): Promise<string> {
    const s = this.selection();
    const writeGeneration = this.writeGeneration;
    const active = () =>
      this.current(s.generation) && writeGeneration === this.writeGeneration;
    if (
      this.currentReceipts(s.key).some((r) =>
        r.state === "pending" || r.state === "unknown"
      )
    ) throw new Error("Reconcile the existing request; do not submit again");
    const id = this.newRequestId();
    const records = this.receipts.get(s.key) ??
      new Map<string, ReceiptRecord>();
    if (records.has(id)) throw new Error("Request ID must be unique");
    const record: ReceiptRecord = {
      receipt: {
        requestId: id,
        operation,
        state: "pending",
        error: null,
        response: null,
      },
      saved,
    };
    records.set(id, record);
    this.receipts.set(s.key, records);
    this.publish({ receipts: this.currentReceipts(s.key), error: null });
    try {
      const response = await send(id);
      if (!active()) return id;
      this.committed(s.key, record, response);
    } catch (error) {
      if (!active()) return id;
      // Non-typed errors include preflight validation (no transmission). The real
      // client wraps every failure after fetch as typed unknown.
      const unknown = error instanceof DriveRequestError &&
        error.classification === "unknown";
      record.receipt = {
        ...record.receipt,
        state: unknown ? "unknown" : "not_committed",
        error: message(error),
      };
      if (error instanceof DriveRequestError && error.code === "conflict") {
        const draft = this.drafts.get(s.key);
        if (draft) {
          const conflict = { ...draft, conflict: true };
          this.drafts.set(s.key, conflict);
          this.publish({ draft: conflict });
        }
      }
      this.publish({
        receipts: this.currentReceipts(s.key),
        error: message(error),
      });
    }
    return id;
  }
  private committed(
    k: string,
    record: ReceiptRecord,
    response: DriveMutationResponse,
  ): void {
    record.receipt = {
      ...record.receipt,
      state: "committed",
      response,
      error: null,
    };
    const entry = response.entry;
    const draft = this.drafts.get(k);
    if (
      record.saved && draft && entry &&
      draft.expectedRevision === record.saved.revision &&
      entry.entry.node_id === this.value.entry?.entry.node_id
    ) {
      const updated = {
        ...draft,
        baseText: record.saved.text,
        expectedRevision: entry.revision,
        conflict: false,
      };
      this.drafts.set(k, updated);
      this.publish({ draft: updated, entry, text: record.saved.text });
    } else if (
      entry && entry.entry.node_id === this.value.entry?.entry.node_id
    ) this.publish({ entry });
    this.publish({ receipts: this.currentReceipts(k), error: null });
  }
  async reconcile(requestId: string): Promise<void> {
    const s = this.selection();
    const record = this.receipts.get(s.key)?.get(requestId);
    if (!record || record.receipt.state !== "unknown") {
      throw new Error("No unknown request to reconcile");
    }
    try {
      const status = await this.client.status(
        s.ws,
        requestId,
        this.writeAbort.signal,
      );
      if (!this.current(s.generation)) return;
      if (status.state === "committed" && status.response) {
        this.committed(s.key, record, status.response);
      } else {
        record.receipt = {
          ...record.receipt,
          error: "No commit observed yet; outcome remains unknown",
        };
        this.publish({ receipts: this.currentReceipts(s.key) });
      }
    } catch (error) {
      if (!this.current(s.generation)) return;
      record.receipt = { ...record.receipt, error: message(error) };
      this.publish({ receipts: this.currentReceipts(s.key) });
    }
  }
  dispose(): void {
    this.fence();
    this.disposed = true;
    this.listeners.clear();
  }
}
export function isText(entry: DriveEntry): boolean {
  return entry.kind === "file" &&
    ["text/plain", "text/markdown"].includes(entry.content_type ?? "");
}
export function isImage(entry: DriveEntry): boolean {
  return entry.kind === "file" &&
    /^image\/(png|jpeg|gif|webp)$/.test(entry.content_type ?? "");
}
