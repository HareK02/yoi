import {
  boundedParam,
  loadJournal,
  offsetParam,
  type RelatedCursor,
} from "#lib/workspace/memory/journal-api.ts";
import { deferPage } from "#lib/workspace/navigation/deferred.ts";
import type { PageLoad } from "./$types";

export const load = deferPage(
  async ({ fetch, params, url }: Parameters<PageLoad>[0]) => {
    const workspaceId = params.workspaceId,
      subjectId = params.subjectId,
      p = url.searchParams;
    const journalId = boundedParam(p, "journal", 512),
      entryId = boundedParam(p, "entry", 512);
    const view = p.get("view") === "sessions" ? "sessions" : "entries";
    const query = boundedParam(p, "q", 1024), after = boundedParam(p, "after");
    const call = <K extends Parameters<typeof loadJournal>[3]["action"]>(
      request: Extract<Parameters<typeof loadJournal>[3], { action: K }>,
    ) => loadJournal(fetch, workspaceId, subjectId, request);
    const base = {
      workspaceId,
      subjectId,
      journalId,
      entryId,
      query,
      view,
      after,
      urlQuery: p.toString(),
    };
    if (!journalId || !entryId) {
      const sessions = view === "sessions"
        ? await call({ action: "sessions", after, limit: 20 })
        : null;
      const entries = view === "entries"
        ? await call({
          action: "entries",
          journal_id: journalId,
          query,
          after,
          limit: 20,
        })
        : null;
      const position = journalId
        ? await call({
          action: "text",
          journal_id: journalId,
          entry_id: null,
          field: "current_position",
          expected_token: boundedParam(p, "position_token"),
          byte_offset: offsetParam(p, "position_byte"),
          max_bytes: 8192,
        })
        : null;
      return {
        ...base,
        mode: "list" as const,
        sessions,
        entries,
        position,
        detail: null,
      };
    }
    const entry = await call({
      action: "entry",
      journal_id: journalId,
      entry_id: entryId,
    });
    if (!entry.data) {
      return {
        ...base,
        mode: "detail" as const,
        sessions: null,
        entries: null,
        position: null,
        detail: {
          entry,
          body: null,
          ongoing: null,
          position: null,
          corrections: null,
          targets: [],
          targetOffset: 0,
          targetNext: null,
          related: null,
        },
      };
    }
    const observed = entry.data;
    const read = (
      field: "body" | "ongoing" | "current_position",
      key: string,
    ) =>
      call({
        action: "text",
        journal_id: journalId,
        entry_id: field === "current_position" ? null : entryId,
        field,
        expected_token: boundedParam(p, `${key}_token`) ??
          (field === "current_position"
            ? observed.journal.state_token
            : observed.entry.content_token),
        byte_offset: offsetParam(p, `${key}_byte`),
        max_bytes: 8192,
      });
    let relatedAfter: RelatedCursor | null = null;
    const cursor = boundedParam(p, "related_after");
    if (cursor) {
      try {
        const v = JSON.parse(cursor);
        if (
          typeof v.memory_id === "string" && typeof v.change_id === "string" &&
          typeof v.candidate_id === "string" &&
          Number.isSafeInteger(v.source_index) && v.source_index >= 0
        ) relatedAfter = v;
      } catch { /* 不正cursorは先頭へ戻し、参照を推測しない。 */ }
    }
    const targetOffset = offsetParam(p, "target_offset");
    const targets = entry.data.correction_targets.slice(
      targetOffset,
      targetOffset + 3,
    );
    const [body, ongoing, position, corrections, related] = await Promise.all([
      read("body", "body"),
      read("ongoing", "ongoing"),
      read("current_position", "position"),
      call({
        action: "corrections",
        journal_id: journalId,
        entry_id: entryId,
        after: boundedParam(p, "corrections_after"),
        limit: 10,
      }),
      call({
        action: "related_memory",
        journal_id: journalId,
        entry_id: entryId,
        after: relatedAfter,
        limit: p.get("related") === "all" ? 20 : 3,
      }),
    ]);
    return {
      ...base,
      mode: "detail" as const,
      sessions: null,
      entries: null,
      position: null,
      detail: {
        entry,
        body,
        ongoing,
        position,
        corrections,
        related,
        targets,
        targetOffset,
        targetNext: targetOffset + targets.length <
            entry.data.correction_targets.length
          ? targetOffset + targets.length
          : null,
      },
    };
  },
);
