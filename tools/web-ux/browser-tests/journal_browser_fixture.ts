/** UI向けのHTTP代替データ。Journalの正本・認可・永続化の実装ではない。 */
export const journalFixture = {
  workspaceId: "memory-review",
  subjectId:
    "release-coordination-subject-with-a-deliberately-long-stable-identity-for-responsive-review",
  memoryId:
    "memory-decision-with-a-deliberately-long-stable-identity-for-overflow-review-0000000001",
  candidateId: "candidate-release-decision-0001",
  literalQuery: "release [a+b] %_",
};

function journal(id: string, subjectId: string) {
  return {
    id,
    subject_id: subjectId,
    source_session_id: `session-${id}`,
    title: id === "journal-release"
      ? "Release coordination Session"
      : id === "journal-review"
      ? "Independent review Session"
      : "Archived planning Session",
    current_position_bytes: 120,
    process_order: 24,
    process_byte: 120,
    state_token: `state-${id}`,
    created_at: "2026-01-02T03:04:05Z",
    updated_at: "2026-01-04T03:04:05Z",
  };
}
function source(journalId: string) {
  return journalId === "journal-release"
    ? { state: "missing", history_href: null }
    : journalId === "journal-review"
    ? { state: "unavailable", history_href: null }
    : {
      state: "unavailable",
      history_href: "/w/memory-review/workers/W-fixture/console",
    };
}
function entry(index: number) {
  const journalId = index === 4
    ? "journal-planning"
    : index % 2
    ? "journal-review"
    : "journal-release";
  return {
    id: `entry-${String(index).padStart(2, "0")}`,
    journal_id: journalId,
    position: index + 1,
    state: index === 0 ? "open" : "fixed",
    title: index === 2
      ? "Corrected release evidence"
      : `Release milestone ${
        String(index).padStart(2, "0")
      } · ${journalFixture.literalQuery}`,
    correction_refs: index === 2 ? ["entry-00"] : [],
    content_token: `content-entry-${index}`,
    created_at: "2026-01-02T03:04:05Z",
    updated_at: "2026-01-03T03:04:05Z",
    fixed_at: index === 0 ? null : "2026-01-03T03:04:05Z",
  };
}
const entries = Array.from({ length: 28 }, (_, index) => entry(index));
const related = Array.from({ length: 8 }, (_, index) => ({
  related: {
    selector: {
      memory_id: index === 0
        ? journalFixture.memoryId
        : `memory-related-${index}`,
      change_id: "change-current",
      candidate_id: journalFixture.candidateId,
    },
    source_index: index,
    reconsideration: index === 0,
  },
  claim: index === 0
    ? "Keep subject Memory provenance typed and visible"
    : `Related release lesson ${index}`,
}));
const body =
  `# Preserved Journal narrative\n\nThis saved Journal remains readable without the original Worker or Session. It is not the original transcript or proof of each Memory claim.\n\n${
    Array.from({ length: 64 }, (_, i) =>
      `## Release event ${
        i + 1
      }\n\nThe team compared the available evidence and recorded the release decision. Literal search token: ${journalFixture.literalQuery}.`)
      .join("\n\n")
  }\n\n## Journal narrative complete`;

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });
const failure = (status: number, message: string) =>
  json(
    { error: status === 403 ? "Forbidden" : "Unavailable", message },
    status,
  );

export async function journalBrowserFixture(
  request: Request,
  subjectId: string,
): Promise<Response> {
  if (request.method !== "POST") {
    return failure(405, "read-only Journal queries require POST");
  }
  const r = await request.json().catch(() => null) as
    | Record<string, any>
    | null;
  if (!r || typeof r.action !== "string") {
    return failure(400, "missing query action");
  }
  const reply = (value: unknown) => json({ kind: r.action, value });
  if (subjectId === "failed-subject") {
    return failure(503, "Journal read temporarily unavailable");
  }
  if (r.journal_id === "journal-denied") {
    return failure(403, "Journal read permission denied");
  }
  const empty = subjectId === "empty-subject" ||
    subjectId === "ungenerated-subject";
  switch (r.action) {
    case "sessions": {
      if (empty) return reply({ items: [], next: null });
      const journals = ["journal-release", "journal-review", "journal-archive"];
      const offset = r.after === "fixture-sessions-2" ? 2 : 0;
      return reply({
        items: journals.slice(offset, offset + 2).map((id) => ({
          journal: journal(id, subjectId),
          original_source: source(id),
          unreflected_sources: id === "journal-review",
        })),
        next: offset === 0 ? "fixture-sessions-2" : null,
      });
    }
    case "entries": {
      if (r.query === "fixture-denied") {
        return failure(403, "Journal read permission denied");
      }
      if (r.query === "fixture-failure") {
        return failure(503, "Journal list unavailable");
      }
      const filtered = empty
        ? []
        : entries.filter((e) =>
          (!r.journal_id || r.journal_id === e.journal_id) &&
          (!r.query ||
            `${e.title} ${body}`.toLowerCase().includes(
              String(r.query).toLowerCase(),
            ))
        );
      const offset = r.after === "fixture-entries-2" ? 16 : 0;
      const limit = Math.min(Number(r.limit) || 20, 16);
      return reply({
        items: filtered.slice(offset, offset + limit),
        next: offset + limit < filtered.length ? "fixture-entries-2" : null,
      });
    }
    case "entry": {
      const e = entries.find((e) =>
        e.id === r.entry_id && e.journal_id === r.journal_id
      );
      return e
        ? reply({
          journal: journal(e.journal_id, subjectId),
          entry: e,
          correction_targets: entries.filter((target) =>
            e.correction_refs.includes(target.id)
          ),
          original_source: source(e.journal_id),
        })
        : failure(404, "Journal entry not found");
    }
    case "text": {
      const e = r.entry_id
        ? entries.find((e) =>
          e.id === r.entry_id && e.journal_id === r.journal_id
        )
        : null;
      const token = r.field === "current_position"
        ? `state-${r.journal_id}`
        : e?.content_token;
      if (!token) return failure(404, "Journal text not found");
      if (r.expected_token && r.expected_token !== token) {
        return failure(409, "Journal text changed; reread from the beginning");
      }
      const text = r.field === "body"
        ? body
        : r.field === "ongoing"
        ? "## Ongoing release context\n\nThe next review is still open."
        : "## Current Session position\n\nRelease review is awaiting the next evidence check. This current position does not replace the event history.";
      const bytes = new TextEncoder().encode(text);
      const start = Number(r.byte_offset) || 0;
      let end = Math.min(
        bytes.length,
        start + Math.min(Number(r.max_bytes) || 16384, 16384),
      );
      while (end < bytes.length && (bytes[end] & 0xc0) === 0x80) end -= 1;
      return reply({
        text: new TextDecoder().decode(bytes.slice(start, end)),
        next_byte: end < bytes.length ? end : null,
        content_token: token,
      });
    }
    case "corrections":
      return reply({
        items: r.entry_id === "entry-00" ? [entry(2)] : [],
        next: null,
      });
    case "related_memory": {
      if (r.entry_id === "entry-03") return reply({ items: [], next: null });
      const offset = r.after ? Number(r.after.source_index) + 1 : 0;
      const limit = Math.min(Number(r.limit) || 3, 20);
      const items = related.slice(offset, offset + limit);
      const last = items.at(-1)?.related;
      return reply({
        items,
        next: offset + limit < related.length && last
          ? { ...last.selector, source_index: last.source_index }
          : null,
      });
    }
    case "memory_sources": {
      const s = r.selector;
      // 不変selectorは本番Memory loaderから受け取り、routeだけから推測しない。
      if (
        !s || s.candidate_id !== journalFixture.candidateId ||
        ![
          "change-current",
          "change-earlier",
          "change-original",
          "change-parent",
        ].includes(
          s.change_id,
        )
      ) return failure(404, "Immutable candidate not found");
      if (s.memory_id === "memory-sources-failure") {
        return failure(503, "Related Journal unavailable");
      }
      if (s.memory_id === "memory-resolved-0002") {
        return reply({
          selector: s,
          sources: [{
            link: {
              source_index: 0,
              original_source: "unknown",
              connection: {
                state: "unconnected",
                reason: "insufficient_source",
              },
            },
            entry: null,
            original_source: { state: "unavailable", history_href: null },
          }],
          total_sources: 1,
          next_source_index: null,
        });
      }
      const offset = Number(r.source_offset) || 0;
      const limit = Math.min(Number(r.limit) || 3, 20);
      const sources = entries.slice(offset, Math.min(8, offset + limit)).map((
        e,
        index,
      ) => ({
        link: {
          source_index: offset + index,
          original_source: e.journal_id === "journal-release"
            ? "missing"
            : "unknown",
          connection: {
            state: "linked",
            journal_id: e.journal_id,
            entry_id: e.id,
            target: "available",
            reconsideration: e.id === "entry-00",
          },
        },
        entry: e,
        original_source: source(e.journal_id),
      }));
      return reply({
        selector: s,
        sources,
        total_sources: 8,
        next_source_index: offset + limit < 8 ? offset + limit : null,
      });
    }
    default:
      return failure(400, "unsupported read-only Journal query");
  }
}
