import { loadJson, workspaceApiPath } from "#lib/workspace/api/http.ts";

export type JournalMetadata = {
  id: string;
  subject_id: string;
  source_session_id: string;
  title: string;
  current_position_bytes: number;
  process_order: number;
  process_byte: number;
  state_token: string;
  created_at: string;
  updated_at: string;
};
export type JournalEntry = {
  id: string;
  journal_id: string;
  position: number;
  state: "open" | "fixed";
  title: string;
  correction_refs: string[];
  content_token: string;
  created_at: string;
  updated_at: string;
  fixed_at: string | null;
};
export type SourceStatus = {
  state: "missing" | "unavailable";
  history_href: string | null;
};
export type JournalPage<T> = { items: T[]; next: string | null };
export type TextPage = {
  text: string;
  next_byte: number | null;
  content_token: string;
};
export type MemorySelector = {
  memory_id: string;
  change_id: string;
  candidate_id: string;
};
export type RelatedCursor = MemorySelector & { source_index: number };
export type SourceLink = {
  source_index: number;
  original_source: "unknown" | "missing" | "available";
  connection: { state: "unconnected"; reason: string } | {
    state: "linked";
    journal_id: string;
    entry_id: string;
    target: "available" | "unavailable" | "erased";
    reconsideration: boolean;
  };
};
export type MemorySources = {
  selector: MemorySelector;
  sources: {
    link: SourceLink;
    entry: JournalEntry | null;
    original_source: SourceStatus;
  }[];
  total_sources: number;
  next_source_index: number | null;
};
export type JournalResponses = {
  sessions: JournalPage<
    {
      journal: JournalMetadata;
      unreflected_sources: boolean;
      original_source: SourceStatus;
    }
  >;
  entries: JournalPage<JournalEntry>;
  entry: {
    journal: JournalMetadata;
    entry: JournalEntry;
    correction_targets: JournalEntry[];
    original_source: SourceStatus;
  };
  text: TextPage;
  corrections: JournalPage<JournalEntry>;
  memory_sources: MemorySources;
  related_memory: {
    items: {
      related: {
        selector: MemorySelector;
        source_index: number;
        reconsideration: boolean;
      };
      claim: string;
    }[];
    next: RelatedCursor | null;
  };
};
export type JournalRequest =
  | { action: "sessions"; after: string | null; limit: number }
  | {
    action: "entries";
    journal_id: string | null;
    query: string | null;
    after: string | null;
    limit: number;
  }
  | { action: "entry"; journal_id: string; entry_id: string }
  | {
    action: "text";
    journal_id: string;
    entry_id: string | null;
    field: "body" | "ongoing" | "current_position";
    expected_token: string | null;
    byte_offset: number;
    max_bytes: number;
  }
  | {
    action: "corrections";
    journal_id: string;
    entry_id: string;
    after: string | null;
    limit: number;
  }
  | {
    action: "memory_sources";
    selector: MemorySelector;
    source_offset: number;
    limit: number;
  }
  | {
    action: "related_memory";
    journal_id: string;
    entry_id: string;
    after: RelatedCursor | null;
    limit: number;
  };

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Invalid Journal object");
  }
  return value as Record<string, unknown>;
}
function text(value: unknown, max = 16384): string {
  if (
    typeof value !== "string" || new TextEncoder().encode(value).length > max
  ) throw new Error("Invalid Journal text");
  return value;
}
function number(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Error("Invalid Journal number");
  }
  return value;
}
function boolean(value: unknown): boolean {
  if (typeof value !== "boolean") throw new Error("Invalid Journal boolean");
  return value;
}
function nullable<T>(value: unknown, parse: (value: unknown) => T): T | null {
  return value == null ? null : parse(value);
}
function choice<T extends string>(value: unknown, values: readonly T[]): T {
  if (typeof value !== "string" || !values.includes(value as T)) {
    throw new Error("Invalid Journal state");
  }
  return value as T;
}
function array<T>(value: unknown, parse: (value: unknown) => T, max = 32): T[] {
  if (!Array.isArray(value) || value.length > max) {
    throw new Error("Invalid Journal list");
  }
  return value.map(parse);
}
const id = (value: unknown) => text(value, 512);
function selector(value: unknown): MemorySelector {
  const v = object(value);
  return {
    memory_id: id(v.memory_id),
    change_id: id(v.change_id),
    candidate_id: id(v.candidate_id),
  };
}
export function parseJournalEntry(value: unknown): JournalEntry {
  const v = object(value);
  return {
    id: id(v.id),
    journal_id: id(v.journal_id),
    title: text(v.title),
    position: number(v.position),
    state: choice(v.state, ["open", "fixed"]),
    correction_refs: array(v.correction_refs, id, 128),
    content_token: id(v.content_token),
    created_at: text(v.created_at),
    updated_at: text(v.updated_at),
    fixed_at: nullable(v.fixed_at, text),
  };
}
function metadata(value: unknown): JournalMetadata {
  const v = object(value);
  return {
    id: id(v.id),
    subject_id: id(v.subject_id),
    source_session_id: id(v.source_session_id),
    title: text(v.title),
    current_position_bytes: number(v.current_position_bytes),
    process_order: number(v.process_order),
    process_byte: number(v.process_byte),
    state_token: id(v.state_token),
    created_at: text(v.created_at),
    updated_at: text(v.updated_at),
  };
}
function sourceStatus(value: unknown): SourceStatus {
  const v = object(value);
  const href = nullable(v.history_href, text);
  if (
    href !== null &&
    (!href.startsWith("/w/") || href.includes("\\") ||
      /[\u0000-\u001f]/.test(href))
  ) throw new Error("Invalid history link");
  return {
    state: choice(v.state, ["missing", "unavailable"]),
    history_href: href,
  };
}
function page<T>(value: unknown, parse: (v: unknown) => T): JournalPage<T> {
  const v = object(value);
  return { items: array(v.items, parse), next: nullable(v.next, text) };
}
function sourceLink(value: unknown): SourceLink {
  const v = object(value), c = object(v.connection);
  const common = {
    source_index: number(v.source_index),
    original_source: choice(
      v.original_source,
      ["unknown", "missing", "available"] as const,
    ),
  };
  if (c.state === "unconnected") {
    return {
      ...common,
      connection: {
        state: "unconnected",
        reason: choice(c.reason, [
          "not_reconciled",
          "insufficient_source",
          "source_not_admitted",
          "ambiguous_source",
          "journal_not_fixed",
        ]),
      },
    };
  }
  if (c.state !== "linked") throw new Error("Invalid Journal connection");
  return {
    ...common,
    connection: {
      state: "linked",
      journal_id: id(c.journal_id),
      entry_id: id(c.entry_id),
      target: choice(c.target, ["available", "unavailable", "erased"]),
      reconsideration: boolean(c.reconsideration),
    },
  };
}
export function parseJournalResponse<K extends keyof JournalResponses>(
  value: unknown,
  kind: K,
  subjectId?: string,
): JournalResponses[K] {
  const envelope = object(value);
  if (envelope.kind !== kind) throw new Error("Unexpected Journal response");
  const v = object(envelope.value);
  let result: JournalResponses[keyof JournalResponses];
  switch (kind) {
    case "sessions":
      result = page(v, (item) => {
        const i = object(item);
        const journal = metadata(i.journal);
        if (subjectId && journal.subject_id !== subjectId) {
          throw new Error("Journal Subject mismatch");
        }
        return {
          journal,
          unreflected_sources: boolean(i.unreflected_sources),
          original_source: sourceStatus(i.original_source),
        };
      });
      break;
    case "entries":
    case "corrections":
      result = page(v, parseJournalEntry);
      break;
    case "entry": {
      const journal = metadata(v.journal);
      const entry = parseJournalEntry(v.entry);
      if (
        (subjectId && journal.subject_id !== subjectId) ||
        entry.journal_id !== journal.id
      ) throw new Error("Journal identity mismatch");
      result = {
        journal,
        entry,
        correction_targets: array(v.correction_targets, parseJournalEntry, 16),
        original_source: sourceStatus(v.original_source),
      };
      break;
    }
    case "text":
      result = {
        text: text(v.text),
        next_byte: nullable(v.next_byte, number),
        content_token: id(v.content_token),
      };
      break;
    case "memory_sources":
      result = {
        selector: selector(v.selector),
        total_sources: number(v.total_sources),
        next_source_index: nullable(v.next_source_index, number),
        sources: array(v.sources, (source) => {
          const s = object(source);
          const link = sourceLink(s.link);
          const entry = nullable(s.entry, parseJournalEntry);
          if (
            entry &&
            (link.connection.state !== "linked" ||
              entry.id !== link.connection.entry_id ||
              entry.journal_id !== link.connection.journal_id)
          ) throw new Error("Journal link mismatch");
          return {
            link,
            entry,
            original_source: sourceStatus(s.original_source),
          };
        }),
      };
      break;
    case "related_memory":
      result = {
        items: array(v.items, (item) => {
          const i = object(item), r = object(i.related);
          return {
            related: {
              selector: selector(r.selector),
              source_index: number(r.source_index),
              reconsideration: boolean(r.reconsideration),
            },
            claim: text(i.claim),
          };
        }),
        next: nullable(v.next, (cursor) => {
          const c = object(cursor);
          return { ...selector(c), source_index: number(c.source_index) };
        }),
      };
      break;
    default:
      throw new Error("Unknown Journal response");
  }
  return result as JournalResponses[K];
}
export function loadJournal<K extends JournalRequest["action"]>(
  fetchFn: typeof fetch,
  workspaceId: string,
  subjectId: string,
  request: Extract<JournalRequest, { action: K }>,
) {
  return loadJson(
    fetchFn,
    workspaceApiPath(
      workspaceId,
      `/subjektiv/subjects/${encodeURIComponent(subjectId)}/journal-browser`,
    ),
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
    },
    (value) => {
      const parsed = parseJournalResponse(value, request.action, subjectId);
      const identity: JournalRequest = request;
      if (identity.action === "entry") {
        const detail = parsed as JournalResponses["entry"];
        if (
          detail.entry.id !== identity.entry_id ||
          detail.journal.id !== identity.journal_id
        ) throw new Error("Journal request identity mismatch");
      }
      if (identity.action === "memory_sources") {
        const links = parsed as MemorySources;
        for (
          const key of ["memory_id", "change_id", "candidate_id"] as const
        ) {
          if (links.selector[key] !== identity.selector[key]) {
            throw new Error(
              "Memory selector mismatch",
            );
          }
        }
      }
      return parsed;
    },
    { diagnosticLabel: "Journal API", maxResponseBytes: 65536 },
  );
}
export function journalHref(
  workspaceId: string,
  subjectId: string,
  values: Record<string, string | null | undefined> = {},
): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value != null && value !== "") params.set(key, value);
  }
  const suffix = params.toString();
  return `/w/${encodeURIComponent(workspaceId)}/memory/${
    encodeURIComponent(subjectId)
  }/journal${suffix ? `?${suffix}` : ""}`;
}
export function boundedParam(
  params: URLSearchParams,
  key: string,
  max = 16384,
): string | null {
  const value = params.get(key);
  return value && new TextEncoder().encode(value).length <= max ? value : null;
}
export function offsetParam(params: URLSearchParams, key: string): number {
  const v = params.get(key);
  return v && /^(0|[1-9][0-9]{0,14})$/.test(v) &&
      Number.isSafeInteger(Number(v))
    ? Number(v)
    : 0;
}
