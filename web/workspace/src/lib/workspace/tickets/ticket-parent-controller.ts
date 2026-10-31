import type { TicketChildrenPage, TicketLinkSummary, TicketParentLink, TicketParentSetRequest } from "#lib/generated/ticket-api.ts";
import type { ObservedOperation } from "#lib/workspace/api/ticket-operations.ts";
import { parentOperation, type TicketParentClient } from "#lib/workspace/api/ticket-parent.ts";

export const MAX_VISIBLE_CHILDREN = 500;
type Attempt = { operation: ObservedOperation; key: string; rejected: boolean };
export type TicketParentState = {
  workspaceId: string;
  ticketId: string;
  canSetParent: boolean;
  parent: TicketParentLink | null;
  children: TicketChildrenPage | null;
  candidates: TicketLinkSummary[];
  candidatesTruncated: boolean;
  loadingParent: boolean;
  loadingChildren: boolean;
  searching: boolean;
  saving: boolean;
  parentError: string | null;
  childrenError: string | null;
  searchError: string | null;
  operationError: string | null;
  retentionError: string | null;
  pending: Attempt | null;
  reconciled: boolean;
  saved: boolean;
};
const empty = (): TicketParentState => ({
  workspaceId: "", ticketId: "", canSetParent: false, parent: null, children: null,
  candidates: [], candidatesTruncated: false, loadingParent: false, loadingChildren: false,
  searching: false, saving: false, parentError: null, childrenError: null,
  searchError: null, operationError: null, retentionError: null, pending: null, reconciled: false, saved: false,
});
function rejected(cause: unknown): boolean {
  return cause instanceof Error && /\((400|401|403|404|409|422)\)/.test(cause.message);
}
function failure(cause: unknown): string {
  const message = cause instanceof Error ? cause.message : "Request could not be completed.";
  if (/\(401\)|\(403\)/.test(message)) return "Permission denied. No parent change is confirmed.";
  if (/\(409\)/.test(message)) return "Parent update conflict. Reread the parent before choosing a new change.";
  if (rejected(cause)) return "Parent request rejected. Reread the parent before trying a new change.";
  return "Outcome unavailable or unknown. No success is inferred; retain the exact request for retry.";
}

export class TicketParentController {
  private value = empty();
  private listeners = new Set<(state: TicketParentState) => void>();
  private attempts = new Map<string, Attempt>();
  private epoch = 0;
  private searchEpoch = 0;
  private identity = "";
  constructor(private client: TicketParentClient, private storage?: Pick<Storage, "getItem" | "setItem" | "removeItem">) {}
  private storageKey(): string { return `yoi.ticket-parent-attempt:${this.identity}`; }
  private retain(attempt: Attempt | null): boolean {
    try {
      if (attempt) this.storage?.setItem(this.storageKey(), JSON.stringify(attempt));
      else this.storage?.removeItem(this.storageKey());
      if (attempt) this.attempts.set(this.identity, attempt);
      else this.attempts.delete(this.identity);
      return true;
    } catch {
      this.patch({ retentionError: "Parent request retention is unavailable. Keep this page open; new changes are blocked." });
      return false;
    }
  }
  private restore(workspaceId: string, ticketId: string): Attempt | null {
    const memory = this.attempts.get(this.identity);
    if (memory) return memory;
    const raw = this.storage?.getItem(this.storageKey());
    if (!raw) return null;
    if (raw.length > 16_384) throw new Error("Invalid retained request");
    const value = JSON.parse(raw);
    if (!value || typeof value !== "object" || typeof value.operation?.body !== "string" || typeof value.rejected !== "boolean") throw new Error("Invalid retained request");
    const body = JSON.parse(value.operation.body);
    const nullable = (item: unknown) => item === null || typeof item === "string";
    if (!body || typeof body !== "object" || typeof body.operation_key !== "string" || !body.operation_key || !nullable(body.parent) || !body.expected || !nullable(body.expected.parent_ticket_id) || !nullable(body.expected.last_operation_key)) throw new Error("Invalid retained request");
    const request: TicketParentSetRequest = {
      expected: { parent_ticket_id: body.expected.parent_ticket_id, last_operation_key: body.expected.last_operation_key },
      parent: body.parent, operation_key: body.operation_key,
    };
    const operation = parentOperation(workspaceId, { ticket_id: ticketId, observation: request.expected, parent: null }, request.parent, request.operation_key);
    if (operation.path !== value.operation.path || operation.method !== value.operation.method || operation.body !== value.operation.body || value.key !== request.operation_key) throw new Error("Invalid retained request");
    const attempt: Attempt = { operation, key: request.operation_key, rejected: value.rejected };
    this.attempts.set(this.identity, attempt);
    if (!attempt.rejected) this.unknownKeys.add(attempt.key);
    return attempt;
  }
  subscribe = (listener: (state: TicketParentState) => void): (() => void) => {
    this.listeners.add(listener); listener(this.value);
    return () => { this.listeners.delete(listener); };
  };
  private patch(update: Partial<TicketParentState>): void {
    this.value = { ...this.value, ...update };
    for (const listener of this.listeners) listener(this.value);
  }
  async select(workspaceId: string, ticketId: string, initial: TicketParentLink, canSetParent: boolean): Promise<void> {
    ++this.epoch; ++this.searchEpoch;
    this.identity = JSON.stringify([workspaceId, ticketId]);
    this.value = empty();
    let pending: Attempt | null = null;
    let retentionError: string | null = null;
    try { pending = this.restore(workspaceId, ticketId); }
    catch { retentionError = "Retained parent request cannot be read. New changes are blocked; do not discard an unknown operation."; }
    this.patch({ workspaceId, ticketId, parent: initial.ticket_id === ticketId ? initial : null,
      canSetParent, pending, retentionError });
    await Promise.all([this.readParent(), this.readChildren()]);
  }
  suspend(): void {
    ++this.epoch; ++this.searchEpoch;
    this.identity = "";
    this.value = empty();
    this.patch({});
  }
  setEligibility(canSetParent: boolean): void {
    if (this.value.canSetParent !== canSetParent) {
      ++this.searchEpoch;
      this.patch({ canSetParent, candidates: [], searching: false });
    }
  }
  async readParent(): Promise<void> {
    if (this.value.loadingParent || this.value.saving) return;
    const epoch = this.epoch;
    const { workspaceId, ticketId } = this.value;
    this.patch({ loadingParent: true, parentError: null, reconciled: false });
    try {
      const link = await this.client.parent(workspaceId, ticketId);
      if (epoch !== this.epoch) return;
      if (link.ticket_id !== ticketId) throw new Error("Wrong parent response identity");
      const pending = this.value.pending;
      const saved = pending !== null && link.observation.last_operation_key === pending.key;
      if (saved) this.retain(null);
      this.patch({ parent: link, reconciled: true,
        ...(saved ? { pending: null, saved: true, operationError: null } : {}) });
    } catch (cause) {
      if (epoch === this.epoch) this.patch({ parentError: failure(cause),
        ...(cause instanceof Error && /\(401\)|\(403\)/.test(cause.message) ? { canSetParent: false } : {}) });
    }
    finally { if (epoch === this.epoch) this.patch({ loadingParent: false }); }
  }
  async readChildren(more = false): Promise<void> {
    if (this.value.loadingChildren) return;
    const previous = this.value.children;
    if (more && (!previous?.has_more || !previous.next_cursor || previous.items.length >= MAX_VISIBLE_CHILDREN)) return;
    const epoch = this.epoch;
    const { workspaceId, ticketId } = this.value;
    const cursor = more ? previous!.next_cursor : null;
    this.patch({ loadingChildren: true, childrenError: null });
    try {
      const page = await this.client.children(workspaceId, ticketId, cursor);
      if (epoch !== this.epoch) return;
      if (page.parent_ticket_id !== ticketId || page.items.length > 50 || (page.has_more && page.next_cursor === cursor)) {
        throw new Error("Invalid children response identity or cursor");
      }
      const items = more && previous
        ? [...new Map([...previous.items, ...page.items].map((item) => [item.id, item])).values()]
        : page.items;
      this.patch({ children: { ...page, items } });
    } catch (cause) { if (epoch === this.epoch) this.patch({ childrenError: failure(cause) }); }
    finally { if (epoch === this.epoch) this.patch({ loadingChildren: false }); }
  }
  async search(input: string, reference = false): Promise<void> {
    if (!this.value.canSetParent || this.value.pending || !input.trim()) return;
    const epoch = this.epoch;
    const searchEpoch = ++this.searchEpoch;
    const { workspaceId, ticketId } = this.value;
    this.patch({ searching: true, candidates: [], candidatesTruncated: false, searchError: null });
    try {
      const result = reference
        ? { items: [await this.client.resolve(workspaceId, input.trim())], hasMore: false }
        : await this.client.search(workspaceId, input.trim());
      if (epoch !== this.epoch || searchEpoch !== this.searchEpoch) return;
      this.patch({ candidates: result.items.filter((item) => item.id !== ticketId), candidatesTruncated: result.hasMore });
    } catch (cause) {
      if (epoch === this.epoch && searchEpoch === this.searchEpoch) this.patch({ searchError: failure(cause) });
    } finally {
      if (epoch === this.epoch && searchEpoch === this.searchEpoch) this.patch({ searching: false });
    }
  }
  async change(parent: TicketLinkSummary | null, operationKey: string): Promise<void> {
    const state = this.value;
    if (!state.canSetParent || state.retentionError || !state.parent || state.parentError || state.loadingParent || state.saving || state.pending || parent?.id === state.ticketId) return;
    const desired = parent?.id ?? null;
    if (desired === state.parent.observation.parent_ticket_id) return;
    const attempt: Attempt = {
      operation: parentOperation(state.workspaceId, state.parent, desired, operationKey),
      key: operationKey, rejected: false,
    };
    await this.run(attempt);
  }
  async retry(): Promise<void> {
    if (this.value.pending && this.value.canSetParent) await this.run(this.value.pending);
  }
  private async run(attempt: Attempt): Promise<void> {
    if (this.value.saving || this.value.loadingParent || this.value.retentionError) return;
    if (!this.retain({ ...attempt, rejected: false })) return;
    const epoch = this.epoch;
    const previouslyUnknown = this.unknownKeys.has(attempt.key);
    this.unknownKeys.add(attempt.key);
    ++this.searchEpoch;
    this.patch({ saving: true, searching: false, pending: attempt, saved: false, operationError: null, reconciled: false });
    try {
      const result = await this.client.set(attempt.operation);
      if (epoch !== this.epoch) return;
      if (result.ticket_id !== this.value.ticketId || result.observation.last_operation_key !== attempt.key) throw new Error("Wrong mutation response identity");
      this.retain(null);
      this.unknownKeys.delete(attempt.key);
      this.patch({ pending: null, saved: true, candidates: [] });
    } catch (cause) {
      if (epoch === this.epoch) {
        // 後の拒否応答では、先行する応答消失時の保存有無を否定できない。
        const updated = { ...attempt, rejected: rejected(cause) && !previouslyUnknown };
        if (updated.rejected) this.unknownKeys.delete(attempt.key);
        this.retain(updated);
        this.patch({ pending: updated, operationError: failure(cause),
          ...(cause instanceof Error && /\(401\)|\(403\)/.test(cause.message) ? { canSetParent: false } : {}) });
      }
    } finally {
      if (epoch === this.epoch) this.patch({ saving: false });
    }
    // 完全一致再送の保存済み応答を現在の親の観測として使わず、別に再読する。
    if (epoch === this.epoch && !this.value.pending) await this.readParent();
  }
  private unknownKeys = new Set<string>();
  useRefreshedObservation(): void {
    if (this.value.saving || !this.value.reconciled || !this.value.pending?.rejected) return;
    if (!this.retain(null)) return;
    this.patch({ pending: null, operationError: null, saved: false });
  }
  dispose(): void { ++this.epoch; ++this.searchEpoch; this.listeners.clear(); }
}
