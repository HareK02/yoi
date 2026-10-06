import type { Event, Method, Segment } from "#lib/generated/protocol.ts";
import type { ComposerDraftSnapshot } from "./composer-draft.ts";

export type ComposerAdmissionMethod = Extract<
  Method,
  { method: "submit" | "notify" }
>;
export type ComposerAdmission = {
  method: ComposerAdmissionMethod;
  snapshot: ComposerDraftSnapshot;
  delivery: ComposerDelivery;
  status: "waiting" | "unknown" | "rejected";
};

function admissionId(method: ComposerAdmissionMethod): string {
  return method.method === "submit"
    ? method.params.submission_request_id
    : method.params.notification_request_id;
}

/** Transport send is not admission. Keep exact typed input and idempotency IDs
 * until an authoritative acknowledgement; uncertain sends retain upload leases.
 */
export class ComposerAdmissions {
  private records = new Map<string, ComposerAdmission>();

  get(target: string): ComposerAdmission | null {
    return this.records.get(target) ?? null;
  }

  begin(
    target: string,
    method: ComposerAdmissionMethod,
    snapshot: ComposerDraftSnapshot,
    delivery: ComposerDelivery,
  ): ComposerAdmission {
    const prior = this.get(target);
    if (prior && prior.status !== "rejected") {
      throw new Error(
        "Resolve the in-flight admission before editing or sending a new draft.",
      );
    }
    const unchanged = prior && prior.method.method === method.method &&
      JSON.stringify(prior.snapshot.segments) ===
        JSON.stringify(snapshot.segments) &&
      (prior.snapshot.textPastes.length > 0) ===
        (snapshot.textPastes.length > 0);
    const record: ComposerAdmission = unchanged
      ? { ...prior, status: "waiting" }
      : {
        // Route-cached drafts may contain Svelte proxies. Snapshot the actual
        // JSON wire values rather than trying to structured-clone those proxies.
        method: JSON.parse(JSON.stringify(method)) as ComposerAdmissionMethod,
        snapshot: JSON.parse(JSON.stringify(snapshot)) as ComposerDraftSnapshot,
        delivery,
        status: "waiting",
      };
    this.records.set(target, record);
    return record;
  }

  retry(target: string): ComposerAdmission | null {
    const prior = this.get(target);
    if (!prior || prior.status === "rejected") return null;
    const record: ComposerAdmission = { ...prior, status: "waiting" };
    this.records.set(target, record);
    return record;
  }

  disconnected(target: string): void {
    const prior = this.get(target);
    if (prior && prior.status !== "rejected") {
      this.records.set(target, { ...prior, status: "unknown" });
    }
  }

  protects(segment: Segment): boolean {
    if (segment.kind !== "uploaded_file") return false;
    return [...this.records.values()].some((record) =>
      record.status !== "rejected" &&
      record.snapshot.segments.some((candidate) =>
        candidate.kind === "uploaded_file" &&
        candidate.file.artifact_id === segment.file.artifact_id
      )
    );
  }

  acknowledge(
    target: string,
    event: Event,
  ): { record: ComposerAdmission; accepted: boolean; message?: string } | null {
    const record = this.get(target);
    if (!record) return null;
    let id: string;
    let accepted: boolean;
    let message: string | undefined;
    if (
      (event.event === "submission_accepted" ||
        event.event === "submission_rejected") &&
      record.method.method === "submit"
    ) {
      id = event.data.submission_request_id;
      accepted = event.event === "submission_accepted";
      if (event.event === "submission_rejected") message = event.data.message;
    } else if (
      (event.event === "notification_accepted" ||
        event.event === "notification_rejected") &&
      record.method.method === "notify"
    ) {
      id = event.data.notification_request_id;
      accepted = event.event === "notification_accepted";
      if (event.event === "notification_rejected") message = event.data.message;
    } else return null;
    if (id !== admissionId(record.method)) return null;
    if (accepted) this.records.delete(target);
    else this.records.set(target, { ...record, status: "rejected" });
    return { record, accepted, message };
  }
}

export type ComposerDelivery = "submit" | "queue" | "notify";

export type ComposerDeliveryState = {
  delivery: ComposerDelivery;
  workerState: string;
  protocolOpen: boolean;
  sending: boolean;
  hasText: boolean;
  hasAttachments: boolean;
};

/**
 * Resolve whether the current Composer draft can use one delivery action.
 * Immediate Submit is idle-only; Queue submits to the busy Worker's FIFO.
 * Notify is running-only and remains distinct from next-turn input.
 */
export function canDeliverComposerDraft(state: ComposerDeliveryState): boolean {
  if (!state.protocolOpen || state.sending) return false;

  const hasInput = state.hasText || state.hasAttachments;
  switch (state.delivery) {
    case "submit":
      return state.workerState === "idle" && hasInput;
    case "queue":
      return (state.workerState === "running" ||
        state.workerState === "paused") &&
        hasInput;
    case "notify":
      return state.workerState === "running" && state.hasText &&
        !state.hasAttachments;
  }
}

export function sendComposerDelivery<T>(
  state: ComposerDeliveryState,
  method: T,
  send: (method: T) => void,
): boolean {
  if (!canDeliverComposerDraft(state)) return false;
  send(method);
  return true;
}
