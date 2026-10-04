import { extname, join, normalize } from "jsr:@std/path@1.1.4";

const port = Number(Deno.args[0]);
const buildRoot = Deno.args[1];
const sectionCount = Deno.args[2] === undefined ? 90 : Number(Deno.args[2]);
if (
  !Number.isInteger(port) || !buildRoot || !Number.isInteger(sectionCount) || sectionCount < 3 ||
  sectionCount > 100
) {
  throw new Error("usage: server.ts <port> <build-root> [section-count: 3..100]");
}

const workspaceId = "memory-review";
const representativeSubjectId =
  "release-coordination-subject-with-a-deliberately-long-stable-identity-for-responsive-review";
const representativeMemoryId =
  "memory-decision-with-a-deliberately-long-stable-identity-for-overflow-review-0000000001";
const derivedMemoryId = "memory-derived-source-0002";
const emptySubjectId = "empty-subject";
const staleSubjectId = "stale-subject";
const failedSubjectId = "failed-subject";
const ungeneratedSubjectId = "ungenerated-subject";
const errorSubjectId = "error-subject";
const pagedSubjectId = "paged-subject";
const subjectPageCursor = "fixture-subject-page-2";
const emptySubjectPageCursor = "fixture-empty-subject-page";
const errorSubjectPageCursor = "fixture-error-subject-page";
const memoryPageCursor = "fixture-memory-page-2";
const revisionPageCursor = "fixture-revision-page-2";
const createdSubjectIds = new Set<string>();
const subjectCreateRequests: unknown[] = [];
let createdSubjectCount = 0;

const paragraphs = Array.from(
  { length: sectionCount },
  (_, index) =>
    `## Section ${
      index + 1
    }\n\nThis is representative resident Memory context with **important context**, an identifier \`memory-item-${
      index + 1
    }\`, and a long URL https://example.com/${"long-segment-".repeat(12)}${
      index + 1
    }.\n\n> A durable note for section ${index + 1}.\n\n- first detail\n- second detail`,
).join("\n\n");
const surfaceBodyMd =
  `# Resident subject context\n\nThis generated surface is read-only.\n\n| Very wide heading | Another heading | Decision |\n| --- | --- | --- |\n| ${
    "wide-value-".repeat(16)
  } | stable | keep scrolling local |\n\n##### Deep surface heading\n\n###### Deepest surface heading\n\n\`\`\`rust\nfn example() { println!("${
    "wide-code-".repeat(18)
  }"); }\n\`\`\`\n\n[Safe link](https://example.com) [Unsafe link](javascript:alert(1))\n\n<img src=x onerror=alert(1)>\n\n${paragraphs}\n\n## Surface content complete`;

const detailBodyMd = `# Committed Memory detail

This immutable revision preserves its candidate evidence, source refs, and derivation refs.

| Local table overflow | Value |
| --- | --- |
| ${"detail-wide-value-".repeat(14)} | stable |

\`\`\`text
${"detail-wide-code-".repeat(18)}
\`\`\`

[Safe evidence link](https://example.com/evidence) [Unsafe evidence link](javascript:alert(2))

<script>alert("not rendered")</script>`;

const subjects = [
  subject(representativeSubjectId, "Release coordination", "active", 42),
  subject(emptySubjectId, "Empty ready subject", "active", 0),
  subject(staleSubjectId, "Stale surface subject", "active", 19),
  subject(failedSubjectId, "Failed surface subject", "active", 8),
  subject(ungeneratedSubjectId, "Ungenerated subject", "active", 0),
  subject(errorSubjectId, "Unavailable subject", "retired", 3),
];
const pagedSubject = subject(pagedSubjectId, "Subject on the next page", "active", 1);

function subject(id: string, role: string, state: "active" | "retired", storeRevision: number) {
  return {
    id,
    role,
    state,
    store_revision: storeRevision,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-02T03:04:05Z",
  };
}

function memoriesFor(subjectId: string) {
  if (createdSubjectIds.has(subjectId)) return [];
  if (subjectId === emptySubjectId || subjectId === ungeneratedSubjectId) return [];
  if (subjectId === staleSubjectId) {
    return [
      memorySummary("stale-memory-1", 2, "lesson", "resolved", "A resolved stale-surface lesson"),
    ];
  }
  if (subjectId === failedSubjectId) {
    return [
      memorySummary(
        "failed-memory-1",
        1,
        "constraint",
        "retracted",
        "A retracted failed-surface constraint",
      ),
    ];
  }
  return [
    memorySummary(
      representativeMemoryId,
      3,
      "decision",
      "active",
      "Keep subject Memory provenance typed and visible",
    ),
    memorySummary(
      "memory-resolved-0002",
      2,
      "lesson",
      "resolved",
      "The bounded reader must preserve continuations",
    ),
    memorySummary(
      "memory-retracted-0003",
      1,
      "working_assumption",
      "retracted",
      "T-672 UNIQUE END MARKER",
    ),
  ];
}

function memorySummary(
  id: string,
  revision: number,
  kind: string,
  state: string,
  claim: string,
) {
  return {
    id,
    revision,
    kind,
    state,
    claim,
    excerpt: `Read-only ${state} Memory with exact revision and provenance metadata.`,
    updated_at: "2026-01-02T03:04:05Z",
  };
}

function surfaceFor(subjectId: string) {
  if (subjectId === representativeSubjectId) {
    return {
      subject_id: subjectId,
      availability: "ready",
      snapshot: {
        snapshot_id: "surface-snapshot-0042",
        body_md: surfaceBodyMd,
        memory_refs: [
          { memory_id: representativeMemoryId, revision: 3 },
          { memory_id: derivedMemoryId, revision: 4 },
        ],
        built_from_store_revision: 42,
        created_at: "2026-01-02T03:04:05Z",
      },
    };
  }
  if (subjectId === emptySubjectId) {
    return {
      subject_id: subjectId,
      availability: "ready",
      snapshot: {
        snapshot_id: "surface-snapshot-empty",
        body_md: "",
        memory_refs: [],
        built_from_store_revision: 0,
        created_at: "2026-01-02T03:04:05Z",
      },
    };
  }
  if (subjectId === staleSubjectId) return { subject_id: subjectId, availability: "stale" };
  if (subjectId === failedSubjectId) return { subject_id: subjectId, availability: "failed" };
  return { subject_id: subjectId, availability: "ungenerated" };
}

function memoryDetail(memoryId: string, requestedRevision: number | null) {
  const revision = requestedRevision && requestedRevision <= 3 ? requestedRevision : 3;
  const states = { 1: "retracted", 2: "resolved", 3: "active" } as const;
  return {
    memory_id: memoryId,
    revision,
    current_revision: 3,
    kind: "decision",
    state: states[revision as keyof typeof states],
    claim: revision === 3
      ? "Keep subject Memory provenance typed and visible"
      : `Historical provenance decision, revision ${revision}`,
    body_md: detailBodyMd,
    why_useful:
      "Future readers can audit why a committed Memory exists without exposing unbounded session content.",
    staleness: null,
    change_reason: revision === 3
      ? "Clarified the durable read contract."
      : "Immutable historical revision.",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: `2026-01-0${revision}T03:04:05Z`,
    body_offset: 0,
    body_byte_offset: 0,
    body_truncated: false,
    source_candidate_ids: ["candidate-release-decision-0001"],
    source_candidates: [{
      candidate_id: "candidate-release-decision-0001",
      evidence: [{
        id: "evidence-message-0001",
        kind: "message",
        entry_range: [12, 14],
        origin: {
          kind: "flow_instruction",
          workspace_id: workspaceId,
          runtime_id: "runtime-fixture",
          worker_id: "worker-fixture",
          flow_selector: "builtin:coder-review",
          flow_definition_id: "flow-definition-fixture",
          flow_definition_revision: 7,
        },
        excerpt: "Use normal explicit subject routes and preserve provenance.",
        summary: "The product contract was explicitly requested.",
      }],
      evidence_total: 1,
      evidence_truncated: false,
      source_refs: [{
        session_id: "session-fixture-0001",
        segment_id: "segment-fixture-0001",
        entry_range: [12, 14],
        evidence_id: "evidence-message-0001",
        origin: { kind: "human_input", account_id: "account-fixture" },
        evidence_kind: "message",
        label: "T-672 product direction",
        summary: "Bounded source reference retained with the committed revision.",
      }],
      source_refs_total: 1,
      source_refs_truncated: false,
    }],
    derived_from: [{ memory_id: derivedMemoryId, revision: 4 }],
    evidence_has_more: false,
  };
}

function revisions(memoryId: string, cursor: string | null) {
  const all = [
    {
      revision: 3,
      kind: "decision",
      state: "active",
      claim: "Keep subject Memory provenance typed and visible",
      change_reason: "Clarified the durable read contract.",
      updated_at: "2026-01-03T03:04:05Z",
    },
    {
      revision: 2,
      kind: "decision",
      state: "resolved",
      claim: "Historical provenance decision, revision 2",
      change_reason: "Resolved after endpoint integration.",
      updated_at: "2026-01-02T03:04:05Z",
    },
    {
      revision: 1,
      kind: "decision",
      state: "retracted",
      claim: "Historical provenance decision, revision 1",
      change_reason: "Retracted the legacy shape.",
      updated_at: "2026-01-01T03:04:05Z",
    },
  ];
  const continued = cursor === revisionPageCursor;
  return {
    memory_id: memoryId,
    current_revision: 3,
    items: continued ? all.slice(2) : all.slice(0, 2),
    ...(continued ? {} : { next_cursor: revisionPageCursor }),
    has_more: !continued,
  };
}

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });

const mime: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
};

async function staticResponse(pathname: string): Promise<Response> {
  const relative = pathname === "/" ? "index.html" : pathname.replace(/^\/+/, "");
  const normalized = normalize(relative);
  if (normalized.startsWith("..")) return new Response("not found", { status: 404 });
  let filePath = join(buildRoot, normalized);
  try {
    const stat = await Deno.stat(filePath);
    if (stat.isDirectory) filePath = join(filePath, "index.html");
    return new Response(await Deno.readFile(filePath), {
      headers: { "content-type": mime[extname(filePath)] ?? "application/octet-stream" },
    });
  } catch {
    return new Response(await Deno.readFile(join(buildRoot, "index.html")), {
      headers: { "content-type": "text/html; charset=utf-8" },
    });
  }
}

Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  if (url.pathname === "/fixture/subject-create-requests") {
    return json({ count: subjectCreateRequests.length, requests: subjectCreateRequests });
  }
  if (url.pathname === "/api/workspaces") {
    return json([{
      workspace_id: workspaceId,
      owner_account_id: "owner",
      display_name: "Memory Review",
      state: "active",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-02T00:00:00Z",
    }]);
  }
  if (url.pathname === `/api/w/${workspaceId}/workspace`) {
    return json({
      workspace_id: workspaceId,
      display_name: "Memory Review",
      record_authority: "fixture",
      schema_version: 1,
      auth: {
        Passkey: {
          rp_id: "127.0.0.1",
          origin: `http://127.0.0.1:${port}`,
          public_base_url: `http://127.0.0.1:${port}`,
          cookie_name: "fixture",
        },
      },
      permissions: {
        manage_repositories: false,
        manage_secrets: false,
        manage_runtimes: false,
        delete_workspace: false,
      },
      extension_points: {
        store: "fixture",
        event_stream: { status: "ready", note: "fixture", diagnostics: [] },
        host_worker_bridge: { status: "ready", note: "fixture", diagnostics: [] },
        companion_console: { status: "ready", note: "fixture", diagnostics: [] },
      },
    });
  }
  if (url.pathname === `/api/w/${workspaceId}/repositories`) {
    return json({ workspace_id: workspaceId, items: [], source: "fixture", diagnostics: [] });
  }
  if (url.pathname === `/api/w/${workspaceId}/working-directories`) {
    return json({ workspace_id: workspaceId, items: [], diagnostics: [] });
  }
  if (url.pathname === `/api/w/${workspaceId}/workers/launch-options`) {
    return json({
      workspace_id: workspaceId,
      runtimes: [{
        runtime_id: "embedded-worker-runtime",
        display_name: "Embedded Runtime",
        built_in: true,
        worker_creation_available: true,
        working_directory_required: false,
        status: "active",
        diagnostics: [],
      }],
      default_profile: "builtin:companion",
      profiles: [
        {
          id: "builtin:companion",
          label: "Companion",
          description: "General Workspace assistance with explicit subject Memory support.",
          feature_connections: { subjektiv: true },
        },
        {
          id: "builtin:standalone",
          label: "Standalone",
          description: "Independent Worker without Workspace subject Memory support.",
          feature_connections: { subjektiv: false },
        },
      ],
      repositories: [],
      working_directories: [],
      diagnostics: [],
    });
  }

  const subjectsPath = `/api/w/${workspaceId}/subjektiv/subjects`;
  if (url.pathname === subjectsPath) {
    if (request.method === "POST") {
      const payload = await request.json().catch(() => null) as { role?: unknown } | null;
      subjectCreateRequests.push(payload);
      if (!payload || typeof payload.role !== "string" || payload.role.trim().length === 0) {
        return json({ error: "Bad Request", message: "subject role must not be empty" }, 400);
      }
      if (payload.role === "Permission denied") {
        return json({ error: "Forbidden", message: "workspace permission denied" }, 403);
      }
      createdSubjectCount += 1;
      const id = `created-subject-${String(createdSubjectCount).padStart(4, "0")}`;
      const created = subject(id, payload.role, "active", 0);
      createdSubjectIds.add(id);
      subjects.unshift(created);
      return json(created, 201);
    }
    if (request.method !== "GET") return new Response("method not allowed", { status: 405 });
    const cursor = url.searchParams.get("cursor");
    if (cursor === errorSubjectPageCursor) {
      return json({ error: "Service Unavailable", message: "subject list unavailable" }, 503);
    }
    if (cursor === emptySubjectPageCursor) {
      return json({ limit: 100, items: [], has_more: false });
    }
    const continued = cursor === subjectPageCursor;
    return json({
      limit: 100,
      items: continued ? [pagedSubject] : subjects,
      ...(continued ? {} : { next_cursor: subjectPageCursor }),
      has_more: !continued,
    });
  }
  if (url.pathname.startsWith(`${subjectsPath}/`)) {
    const suffix = url.pathname.slice(subjectsPath.length + 1);
    const parts = suffix.split("/").map(decodeURIComponent);
    const subjectId = parts[0];
    if (subjectId === errorSubjectId) return json({}, 503);
    const foundSubject = subjects.find((item) => item.id === subjectId);
    if (!foundSubject) return json({}, 404);

    if (parts.length === 1) return json(foundSubject);
    if (parts.length === 2 && parts[1] === "surface") return json(surfaceFor(subjectId));
    if (parts.length === 2 && parts[1] === "memories") {
      const paginated = subjectId === representativeSubjectId;
      const continued = paginated && url.searchParams.get("cursor") === memoryPageCursor;
      const items = continued
        ? [memorySummary("memory-page-two", 1, "lesson", "active", "Memory on the next page")]
        : memoriesFor(subjectId);
      return json({
        items,
        ...(paginated && !continued ? { next_cursor: memoryPageCursor } : {}),
        has_more: paginated && !continued,
      });
    }
    if (parts.length >= 3 && parts[1] === "memories") {
      const memoryId = parts[2];
      if (parts.length === 4 && parts[3] === "revisions") {
        return json(revisions(memoryId, url.searchParams.get("cursor")));
      }
      if (parts.length === 3) {
        const requested = url.searchParams.get("revision");
        return json(memoryDetail(memoryId, requested === null ? null : Number(requested)));
      }
    }
  }

  if (
    url.pathname === `/api/w/${workspaceId}/protocol/ws` &&
    request.headers.get("upgrade") === "websocket"
  ) {
    const { socket, response } = Deno.upgradeWebSocket(request);
    socket.onmessage = (event) => {
      const frame = JSON.parse(String(event.data));
      if (frame?.message?.method !== "subscribe_events") return;
      const requestId = frame.message.params.request_id;
      socket.send(JSON.stringify({
        protocol_version: 2,
        frame: "response",
        message: {
          result: "subscribed",
          payload: {
            request_id: requestId,
            subscription_id: "fixture-workers",
            selector: { topic: "workspace_workers" },
            snapshot: { topic: "workers", data: { workers: [] } },
          },
        },
      }));
    };
    return response;
  }
  return await staticResponse(url.pathname);
});
