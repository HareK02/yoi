import { createHash } from "node:crypto";
// Synthetic API fixture serving the production static UI; no Worker/Git execution.
import { extname, join, normalize } from "jsr:@std/path@1.1.4";
import {
  fixtureDetail,
  fixturePage,
} from "../../../web/workspace/src/lib/workspace/home/dashboard.test-fixtures.ts";
import type {
  TicketAssignmentTracking,
  TicketAssignmentTrackingHistoryEntry,
  TicketAssignmentTrackingProjection,
  TicketDetail,
  TicketQueryItem,
  TicketQueryRequest,
  WorkspaceTag,
} from "../../../web/workspace/src/lib/generated/ticket-api.ts";
import { emptyLaunchOptions } from "../../../web/workspace/src/lib/workspace/sidebar/worker-launch.test-fixtures.ts";
const port = Number(Deno.args[0]);
const root = Deno.args[1];
const legacy = Deno.args.includes("--legacy");
const tickets = new Map<string, TicketDetail>();
const bodies: unknown[] = [];
const retained = new Map<string, TicketAssignmentTrackingProjection>();
const handlingHistory = new Map<
  string,
  TicketAssignmentTrackingHistoryEntry[]
>();
const receipts = new Map<string, { fingerprint: string; result: unknown }>();
const tags: WorkspaceTag[] = [
  {
    id: "tag-investigation",
    name: "Investigation",
    workspace_id: "tickets",
    deleted: false,
    last_operation_key: "create-investigation",
  },
  {
    id: "tag-ui",
    name: "UI",
    workspace_id: "tickets",
    deleted: false,
    last_operation_key: "create-ui",
  },
];
let assignmentNumber = 10;
let failure: "none" | "unknown_saved" | "unknown_unsaved" | "stale" | "denied" =
  "none";
function remember(row: TicketAssignmentTrackingProjection) {
  retained.set(row.assignment_id, row);
  if (!handlingHistory.has(row.assignment_id)) {
    handlingHistory.set(
      row.assignment_id,
      row.tracking
        ? [{
          at: row.updated_at ?? row.assigned_at,
          event_id: row.last_event_id!,
          operation_key: `initial-${row.assignment_id}`,
          source_context: { actor: "fixture-orchestrator" },
          tracking: structuredClone(row.tracking),
        }]
        : [],
    );
  }
}
function queryItem(t: TicketDetail): TicketQueryItem {
  return {
    id: t.id,
    resource_key: t.resource_key,
    title: t.title,
    state: t.state,
    priority: t.priority,
    tags: t.tags,
    assignment_tracking: t.assignment_tracking,
    closure_operation_key: t.closure_operation_key,
    current_closure: t.current_closure,
    current_worker: t.current_worker,
    content_digest: t.content_digest,
    created_at: t.created_at,
    updated_at: t.updated_at,
    legacy_handoff: t.legacy_handoff,
    readiness: t.readiness,
    workspace_action_priority: "background",
    evidence: t.evidence,
    merge_request: t.merge_request,
    merge_requests: t.merge_requests,
    linked_objective_ids: [],
    linked_objective_keys: [],
    matched_fields: [],
    snippet: t.body,
    matching_event: null,
    relation_count: t.relations.outgoing.length,
    blocker_count: t.relations.blockers.length,
    unresolved_blocker_count: t.relations.blockers.length,
    unresolved_review_count: 0,
  };
}
function pageSlice<T>(items: T[], cursor: string | null, limit: number) {
  const offset = Number(cursor ?? 0);
  const slice = items.slice(offset, offset + limit);
  const hasMore = offset + slice.length < items.length;
  return {
    items: slice,
    page: {
      ...fixturePage(slice.length),
      limit,
      sort: "updated_desc",
      has_more: hasMore,
      next_cursor: hasMore ? String(offset + slice.length) : null,
    },
  };
}
function contentDigest(value: TicketDetail): string {
  return createHash("sha256").update("ticket.content.sha256.v1\0").update(
    JSON.stringify([
      value.id,
      value.title,
      value.body,
      value.targets.map(({ repository_key, ref_selector, access }) => ({
        repository_key,
        ref_selector,
        access,
      })),
    ]),
  ).digest("hex");
}
function ticket(key: string) {
  if (!tickets.has(key)) {
    const value: TicketDetail = {
      ...fixtureDetail(key),
      assignment_tracking: [],
    };
    value.state = "open";
    value.created_at = "2025-12-31T00:00:00Z";
    value.updated_at = "2026-01-02T00:00:00Z";
    value.title = key === "T-2"
      ? "Read-only investigation"
      : key === "T-3"
      ? "Independent review before merge"
      : "Compare options without deciding";
    value.body = key === "T-2"
      ? "Investigate using read-only sources; review is not required."
      : key === "T-3"
      ? "Implement the change, then independent review before merge."
      : "Return options to the user. Do not draw a conclusion.";
    value.action_eligibility.can_assign_orchestrator = true;
    value.action_eligibility.can_unassign_orchestrator = true;
    value.action_eligibility.can_queue = false;
    value.action_eligibility.queue_tickets = [];
    const tracking: TicketDetail["assignment_tracking"][number] = {
      workspace_id: "tickets",
      ticket_id: value.id,
      assignment_id: `assignment-${key}`,
      role: "orchestrator",
      principal: {
        kind: "workspace_agent",
        agent_key: "workspace-orchestrator",
      },
      assigned_at: "2026-01-01T00:00:00Z",
      assigned_by: "fixture-owner",
      current: true,
      last_event_id: `handling-${key}`,
      updated_at: "2026-01-02T00:00:00Z",
      migration_diagnostic: null,
      release: null,
      tracking: key === "T-2"
        ? {
          status: "waiting",
          waiting_reason: "Awaiting the user's policy choice",
          next_action: "Continue after the answer",
          progress: null,
          result: null,
        }
        : key === "T-3"
        ? {
          status: "inprogress",
          progress: "Preparing independent review",
          next_action: "Request review of the exact source",
          waiting_reason: null,
          result: null,
        }
        : {
          status: "done",
          result: "Compared options; returned them without deciding",
          waiting_reason: null,
          progress: null,
          next_action: null,
        },
    };
    value.assignment_tracking = [tracking];
    value.assignments = [{
      assignment_id: tracking.assignment_id,
      role: tracking.role,
      principal: tracking.principal,
      assigned_at: tracking.assigned_at,
      assigned_by: tracking.assigned_by,
    }];
    value.tags = [structuredClone(tags[0])];
    if (key === "T-3" || key === "T-35") {
      value.tags.push(structuredClone(tags[1]));
    }
    if (key === "T-3") {
      value.current_worker = {
        assignment_id: "worker-assignment-3",
        runtime_id: "embedded",
        worker_id: "running-worker",
        worker_resource_key: "W-3",
      };
    }
    if (key === "T-2") {
      value.current_worker = {
        assignment_id: "worker-assignment-2",
        runtime_id: "embedded",
        worker_id: "stopped-worker",
        worker_resource_key: "W-2",
      };
    }
    if (key === "T-4") {
      value.title = "Completed investigation with retained history";
      value.body =
        "Investigate and deliver the complete findings, then judge case closure separately.";
      tracking.tracking = {
        status: "done",
        result: "Delivered the full requested investigation",
        waiting_reason: null,
        progress: null,
        next_action: null,
      };
      value.state = "closed";
      value.closure_operation_key = "close-T-4";
      value.current_closure = {
        at: "2026-01-02T00:00:00Z",
        author: "fixture-owner",
        reason: "completed",
        explanation: "The requested investigation was delivered",
        operation_key: "close-T-4",
        references: [],
      };
      value.closure_history = [{
        at: value.current_closure.at,
        author: value.current_closure.author,
        explanation: value.current_closure.explanation,
        operation_key: "close-T-4",
        references: [],
        action: { action: "close", reason: "completed" },
        previous_closure_operation_key: null,
      }];
      value.action_eligibility.can_start_manual_worker = false;
    }
    value.action_eligibility.can_start_manual_worker = value.state === "open";
    if (key === "T-2") {
      value.targets = [{
        repository_key: "main",
        ref_selector: "develop",
        access: "read_only",
      }];
    }
    if (key === "T-3") {
      value.merge_requests = [{
        merge_request_id: "mr-1",
        repository_key: "main",
        state: "open",
        selector_from: "work/fixture",
        selector_to: "develop",
        review_status: "none",
        source_ref_observation: {
          status: "unavailable",
          code: "provider_unavailable",
        },
        current_subject_ref: null,
        review_subject_ref: null,
        review_excerpt: null,
        integration_evidence_error: null,
        updated_at: "2026-01-02T00:00:00Z",
      }];
    }
    remember(tracking);
    if (["T-1", "T-4"].includes(key)) {
      const previous: TicketAssignmentTrackingProjection = {
        ...structuredClone(tracking),
        assignment_id: `previous-${key}`,
        current: false,
        last_event_id: `previous-event-${key}`,
        release: {
          action: "replaced",
          actor: "fixture-owner",
          at: "2026-01-01T00:00:00Z",
          event_id: `replace-${key}`,
          reason: "New request",
          replacement_assignment_id: tracking.assignment_id,
          last_tracking: structuredClone(tracking.tracking),
          operation_key: `replace-op-${key}`,
        },
      };
      remember(previous);
      if (key === "T-4") {
        remember({
          ...structuredClone(previous),
          assignment_id: "released-T-4",
          release: {
            ...previous.release!,
            action: "unassigned",
            replacement_assignment_id: null,
            reason: "Responsibility released without stopping a Worker",
          },
        });
      }
    }
    tickets.set(key, value);
  }
  const stored = tickets.get(key)!;
  stored.content_digest = contentDigest(stored);
  const value = structuredClone(stored);
  if (legacy) {
    const wire = value as unknown as Record<string, unknown>;
    wire.current_coder = wire.current_worker;
    delete wire.current_worker;
    const actions = wire.action_eligibility as Record<string, unknown>;
    actions.can_start_manual_coder = actions.can_start_manual_worker;
    delete actions.can_start_manual_worker;
  }
  return value;
}
function projectedAssignment(
  row: TicketAssignmentTrackingProjection,
  workspaceId: string,
) {
  const value = { ...structuredClone(row), workspace_id: workspaceId };
  if (workspaceId === "tickets-unknown") {
    value.tracking = null;
    value.updated_at = null;
    value.last_event_id = `legacy-assign-${value.assignment_id}`;
    value.migration_diagnostic =
      "Historical delegation has unknown handling; no status inferred";
    if (value.release) value.release.last_tracking = null;
  }
  return value;
}
function projectedTicket(key: string, workspaceId: string) {
  const value = ticket(key);
  value.tags = value.tags.map((tag) =>
    tags.find((canonical) => canonical.id === tag.id) ?? tag
  )
    .filter((tag) => !tag.deleted).map((tag) => ({
      ...tag,
      workspace_id: workspaceId,
    }));
  if (workspaceId === "tickets-readonly") {
    value.action_eligibility = {
      blockers: ["Read-only fixture"],
      can_assign_orchestrator: false,
      can_unassign_orchestrator: false,
      can_start_manual_worker: false,
      can_queue: false,
      queue_tickets: [],
    };
  }
  value.assignment_tracking = value.assignment_tracking.map((row) =>
    projectedAssignment(row, workspaceId)
  );
  if (workspaceId === "tickets-long") {
    value.title +=
      " — Long investigation title with a full comparison and explicit user confirmation "
        .repeat(4);
    for (const row of value.assignment_tracking) {
      if (row.tracking) {
        row.tracking.waiting_reason =
          "Waiting for a deliberate user choice, not a Worker state change. "
            .repeat(8);
        row.tracking.result =
          "Returned comparison: no Ticket closure, approval or Worker stop inferred.\n"
            .repeat(8);
      }
    }
  }
  return value;
}
function workerSummary(workerId: string, state: string) {
  return {
    worker_id: workerId,
    runtime_id: "embedded",
    host_id: "fixture-host",
    display_name: workerId,
    label: workerId,
    profile: "builtin:orchestrator",
    singleton_key: workerId === "orchestrator-worker"
      ? "workspace-orchestrator"
      : null,
    tags: [],
    workspace: {
      visibility: "shared",
      identity: "tickets",
      workspace_id: "tickets",
    },
    state,
    last_seen_at: "2026-01-02T00:00:00Z",
    pinned: false,
    retention_state: "retained",
    implementation: { kind: "yoi", display_hint: "Fixture Worker" },
    workdir_attachments: [],
    diagnostics: [],
  };
}
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  if (url.pathname === "/fixture-state") return Response.json(bodies);
  if (url.pathname === "/fixture-snapshot") {
    return Response.json({
      tickets: [...tickets.values()],
      assignments: [...retained.values()],
      receipts: [...receipts.keys()],
    });
  }
  if (url.pathname === "/fixture-control" && request.method === "POST") {
    const control = await request.json();
    if (control.reset) {
      tickets.clear();
      retained.clear();
      handlingHistory.clear();
      receipts.clear();
      bodies.length = 0;
      failure = "none";
    }
    if (control.failure) failure = control.failure;
    return Response.json({ configured: true });
  }
  if (url.pathname === "/api/workspaces") {
    return Response.json([{
      workspace_id: "tickets",
      display_name: "Shared investigation",
      owner_account_id: "owner",
      state: "active",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-02T00:00:00Z",
    }]);
  }
  const match = url.pathname.match(
    /^(?:\/api\/w|\/workspaces)\/([^/]+)(\/.*)$/,
  );
  if (match) {
    const [, id, path] = match;
    if (path === "/workspace") {
      return Response.json({
        workspace_id: id,
        display_name: "Shared investigation",
        record_authority: "fixture",
        schema_version: 1,
        auth: {
          Passkey: {
            rp_id: "127.0.0.1",
            origin: url.origin,
            public_base_url: url.origin,
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
          host_worker_bridge: {
            status: "ready",
            note: "fixture",
            diagnostics: [],
          },
          companion_console: {
            status: "ready",
            note: "fixture",
            diagnostics: [],
          },
        },
      });
    }
    if (path === "/repositories") {
      return Response.json({
        workspace_id: id,
        items: [{
          repository_key: "main",
          default_selector: "develop",
          kind: "git",
          provider: "git",
          source: { kind: "local_path", uri: "/fixture/main" },
          source_fingerprint: "fixture",
          observed_status: "ready",
          record_authority: "fixture",
        }],
        source: "fixture",
        diagnostics: [],
      });
    }
    if (path === "/orchestrator") {
      return Response.json({
        workspace_id: id,
        online: id === "tickets-running",
        disposition: id === "tickets-running" ? "online" : "offline",
        worker: workerSummary(
          "orchestrator-worker",
          id === "tickets-running" ? "Running" : "Stopped",
        ),
        diagnostics: [],
      });
    }
    if (path === "/workers/launch-options") {
      const options = emptyLaunchOptions(id);
      options.default_profile = "builtin:ticket-worker";
      options.profiles = [{
        id: "builtin:ticket-worker",
        label: "Ticket Worker",
        description: "Intent-led work",
        feature_connections: { subjektiv: false },
      }, {
        id: "builtin:coder",
        label: "Coder",
        description: "Code recipe",
        feature_connections: { subjektiv: false },
      }];
      options.working_directories = [{
        working_directory_id: "read-only-sources",
        source: {
          kind: "external_grant",
          grant_id: "fixture-grant",
          grant_permissions: { read: true, write: false, command: false },
        },
        materializer_kind: "client_hosted_external",
        status: "active",
        cleanliness: "unknown",
      }];
      if (legacy) {
        for (const runtime of options.runtimes) {
          const wire = runtime as unknown as Record<string, unknown>;
          wire.working_directory_required = wire.supports_workdir_attachments;
          delete wire.supports_workdir_attachments;
        }
      }
      return Response.json(options);
    }
    if (path === "/workers/W-2" || path === "/workers/W-3") {
      const running = path === "/workers/W-3";
      return Response.json({
        ...workerSummary(
          running ? "running-worker" : "stopped-worker",
          running ? "Running" : "Stopped",
        ),
        resource_key: running ? "W-3" : "W-2",
        availability: "observed",
      });
    }
    if (path === "/workers") {
      return Response.json({
        workspace_id: id,
        items: [],
        diagnostics: [],
        limit: 100,
        source: "fixture",
      });
    }
    if (path === "/working-directories") {
      return Response.json({ workspace_id: id, items: [], diagnostics: [] });
    }
    if (
      path === "/protocol/ws" && request.headers.get("upgrade") === "websocket"
    ) {
      const { socket, response } = Deno.upgradeWebSocket(request);
      socket.onmessage = (event) => {
        const frame = JSON.parse(String(event.data));
        if (frame?.message?.method === "subscribe_events") {
          socket.send(JSON.stringify({
            protocol_version: 2,
            frame: "response",
            message: {
              result: "subscribed",
              payload: {
                request_id: frame.message.params.request_id,
                subscription_id: "fixture-workers",
                selector: { topic: "workspace_workers" },
                snapshot: { topic: "workers", data: { workers: [] } },
              },
            },
          }));
        }
      };
      return response;
    }
    if (path === "/tickets/query" && request.method !== "POST") {
      return Response.json(
        { message: "Use POST with TicketQueryRequest JSON" },
        { status: 405 },
      );
    }
    if (path === "/tickets/query" && request.method === "POST") {
      const body: TicketQueryRequest = await request.json();
      bodies.push({ path, method: request.method, body });
      if (id === "tickets-error") {
        return Response.json({
          message:
            "Ticket query unavailable — recover by retrying the same read",
        }, { status: 503 });
      }
      const keys = id === "tickets-empty"
        ? []
        : id === "tickets-pages"
        ? Array.from({ length: 40 }, (_, index) => `T-${index + 1}`)
        : ["T-1", "T-2", "T-3", "T-4"];
      if (id !== "tickets-empty" && tickets.has("T-41")) keys.push("T-41");
      const items = keys.map((key) => projectedTicket(key, id)).filter((t) => {
        if (
          body.states?.length &&
          !body.states.includes(t.state as "open" | "closed")
        ) return false;
        if (
          body.closure_reason &&
          (t.state !== "closed" ||
            JSON.stringify(t.current_closure?.reason) !==
              JSON.stringify(body.closure_reason))
        ) return false;
        const reasons = t.closure_history.map((entry) =>
          entry.action.action === "reopen"
            ? ""
            : typeof entry.action.reason === "string"
            ? entry.action.reason
            : entry.action.reason.other
        ).join(" ");
        const query = body.query?.toLocaleLowerCase();
        if (
          query &&
          !`${t.resource_key} ${t.title} ${t.body} ${
            t.current_closure?.explanation ?? ""
          } ${reasons}`
            .toLocaleLowerCase().includes(query)
        ) return false;
        if (
          body.tag_ids?.some((tagId) => !t.tags.some((tag) => tag.id === tagId))
        ) return false;
        const rows = t.assignment_tracking.filter((row) =>
          row.current &&
          (!body.assigned_principal ||
            JSON.stringify(row.principal) ===
              JSON.stringify(body.assigned_principal))
        );
        if (body.assigned_principal && !rows.length) return false;
        if (
          body.handling_status &&
          !rows.some((row) => row.tracking?.status === body.handling_status)
        ) return false;
        return true;
      }).map(queryItem);
      return Response.json({
        ...pageSlice(items, body.cursor ?? null, body.limit ?? 30),
        record_authority: "fixture",
      });
    }
    if (path === "/ticket-tags") {
      if (request.method === "GET") {
        return Response.json(tags.map((tag) => ({ ...tag, workspace_id: id })));
      }
      if (id === "tickets-readonly") {
        return Response.json({ message: "Permission denied" }, { status: 403 });
      }
      const body = await request.json();
      bodies.push({ path, method: request.method, body });
      const existing = tags.find((tag) => tag.id === body.action.tag_id);
      if (body.action.kind === "create") {
        const tag = {
          id: `tag-${tags.length + 1}`,
          name: body.action.name,
          workspace_id: id,
          last_operation_key: body.operation_key,
          deleted: false,
        };
        tags.push(tag);
        return Response.json(tag);
      }
      if (!existing) {
        return Response.json({ message: "Tag unavailable" }, { status: 404 });
      }
      if (
        body.action.expected_last_operation_key !== existing.last_operation_key
      ) {
        return Response.json({ message: "Tag changed; refresh" }, {
          status: 409,
        });
      }
      if (body.action.kind === "rename") existing.name = body.action.name;
      if (body.action.kind === "delete") {
        existing.deleted = true;
        for (const value of tickets.values()) {
          value.tags = value.tags.filter((tag) => tag.id !== existing.id);
        }
      }
      existing.last_operation_key = body.operation_key;
      return Response.json(existing);
    }
    if (path === "/tickets" && request.method === "POST") {
      if (id === "tickets-readonly") {
        return Response.json({
          message: "Permission denied; read-only fixture",
        }, { status: 403 });
      }
      const body = await request.json();
      bodies.push({ path, body });
      ticket("T-41");
      const created = tickets.get("T-41")!;
      created.assignment_tracking = [];
      created.assignments = [];
      created.tags = [];
      created.action_eligibility.can_unassign_orchestrator = false;
      created.title = body.title;
      created.body = body.body;
      created.targets = body.targets ?? [];
      return Response.json({
        id: created.id,
        resource_key: "T-41",
        slug: "fixture",
        status: "Open",
      });
    }
    if (path === "/tickets") {
      const items = ["T-1", "T-2", "T-3", "T-4"].map(ticket).filter((t) =>
        !url.searchParams.has("states") ||
        url.searchParams.get("states")!.split(",").includes(t.state)
      );
      return Response.json({
        workspace_id: id,
        items: items.map((t) => ({
          id: t.id,
          resource_key: t.resource_key,
          title: t.title,
          state: t.state,
          tags: t.tags,
          closure_operation_key: t.closure_operation_key,
          current_closure: t.current_closure,
          priority: t.priority,
          updated_at: t.updated_at,
          queued_at: t.queued_at,
          queued_by: t.queued_by,
          record_source: "fixture",
          workspace_action_priority: "background",
        })),
        invalid_records: [],
        record_authority: "fixture",
        limit: 30,
        page: fixturePage(items.length),
      });
    }
    const tm = path.match(/^\/tickets\/([^/]+)(.*)$/);
    if (tm) {
      const key = decodeURIComponent(tm[1]).match(/T-\d+/)?.[0] ?? "T-1";
      const stored = tickets.get(key) ?? (ticket(key), tickets.get(key)!);
      const suffix = tm[2];
      if (
        request.method === "GET" &&
        suffix === "/assignment-tracking/assignments"
      ) {
        const rows = [...retained.values()].filter((row) =>
          row.ticket_id === stored.id
        );
        const after = url.searchParams.get("after_assignment_id");
        const offset = after
          ? rows.findIndex((row) => row.assignment_id === after) + 1
          : 0;
        const items = rows.slice(
          offset,
          offset + Number(url.searchParams.get("limit") ?? 20),
        );
        return Response.json({
          items: items.map((row) => projectedAssignment(row, id)),
          next_assignment_id: offset + items.length < rows.length
            ? items.at(-1)?.assignment_id
            : null,
        });
      }
      if (request.method === "GET" && suffix === "/assignment-diagnostics") {
        const assignmentId = url.searchParams.get("assignment_id");
        const row = retained.get(assignmentId ?? "");
        if (!row || row.ticket_id !== stored.id) {
          return Response.json({ message: "Assignment unavailable" }, {
            status: 404,
          });
        }
        return Response.json({
          assignment_id: row.assignment_id,
          current: row.current,
          input_origin_known: id !== "tickets-unknown",
          actionable: id !== "tickets-unknown" && row.current &&
            stored.state === "open" &&
            row.tracking?.status !== "done",
          inputs: id === "tickets-unknown" ? [] : [
            {
              input_event_id: `assign-input-${row.assignment_id}`,
              delivery_id: key === "T-2"
                ? `delivery-input-${row.assignment_id}`
                : null,
              notification: key === "T-2"
                ? "request_persisted"
                : "not_reserved",
              acceptance_outcome_unknown: key === "T-2",
            },
            ...stored.events.filter((entry) =>
              entry.kind === "assignment_answer" &&
              entry.attributes.orchestrator_assignment_id === row.assignment_id
            ).map((entry) => ({
              input_event_id: entry.event_ref,
              delivery_id: null,
              notification: "not_reserved",
              acceptance_outcome_unknown: false,
            })),
          ],
          next_input_event_id: null,
          starts: key === "T-3" && id !== "tickets-unknown"
            ? [{
              operation_id: "start-T-3",
              claim_state: "committed",
              assignment_id: "worker-assignment-3",
              runtime_id: "embedded",
              worker_id: "running-worker",
              actual_processing_unknown: true,
            }]
            : [],
          next_start_operation_id: null,
        });
      }
      if (
        request.method === "GET" && suffix.startsWith("/assignment-tracking")
      ) {
        const assignmentId = url.searchParams.get("assignment_id");
        if (suffix === "/assignment-tracking/history") {
          const rows = id === "tickets-unknown"
            ? []
            : handlingHistory.get(assignmentId ?? "") ?? [];
          const after = url.searchParams.get("after_event_id");
          const offset = after
            ? rows.findIndex((entry) => entry.event_id === after) + 1
            : 0;
          const items = rows.slice(
            offset,
            offset + Number(url.searchParams.get("limit") ?? 20),
          );
          return Response.json({
            items,
            next_event_id: offset + items.length < rows.length
              ? items.at(-1)?.event_id
              : null,
          });
        }
        return Response.json(
          assignmentId
            ? [...retained.values()].filter((row) =>
              row.ticket_id === stored.id && row.assignment_id === assignmentId
            ).map((row) => projectedAssignment(row, id))
            : projectedTicket(key, id).assignment_tracking,
        );
      }
      if (
        request.method !== "GET" &&
        ["/state", "/close", "/ready", "/queue"].includes(suffix)
      ) {
        return Response.json({
          message:
            "Legacy phase actions are unavailable; use a reasoned case judgment or Assign",
        }, { status: 404 });
      }
      if (request.method !== "GET" && id === "tickets-readonly") {
        return Response.json({
          message: "Permission denied; read-only fixture",
        }, { status: 403 });
      }
      if (
        request.method !== "GET" &&
        [
          "/assignments/orchestrator",
          "/assignment-answer",
          "/assignment-tracking",
          "/case",
          "/tags",
        ].includes(suffix)
      ) {
        if (id === "tickets-readonly") {
          return Response.json({
            message: "Permission denied; read-only fixture",
          }, { status: 403 });
        }
        const body = request.method === "DELETE"
          ? Object.fromEntries(url.searchParams)
          : await request.json();
        bodies.push({ path, method: request.method, body });
        const operationKey = body.operation_id ?? body.operation_key;
        if (!operationKey) {
          return Response.json({ message: "Operation identity required" }, {
            status: 400,
          });
        }
        const fingerprint = JSON.stringify({
          path,
          method: request.method,
          body,
        });
        const receipt = receipts.get(operationKey);
        if (receipt) {
          return receipt.fingerprint === fingerprint
            ? Response.json(receipt.result)
            : Response.json({ message: "Operation identity conflict" }, {
              status: 409,
            });
        }
        const injected = failure;
        failure = "none";
        if (injected === "stale") {
          return Response.json({
            message: "Assignment changed; refresh before a new operation",
          }, { status: 409 });
        }
        if (injected === "denied") {
          return Response.json({ message: "Permission denied" }, {
            status: 403,
          });
        }
        if (injected === "unknown_unsaved") {
          return Response.json({ message: "Outcome unknown" }, { status: 503 });
        }
        const current = stored.assignment_tracking.find((row) =>
          row.current && row.role === "orchestrator"
        );
        let result: unknown;
        if (suffix === "/assignments/orchestrator") {
          const expected = request.method === "DELETE"
            ? body.assignment_id
            : body.expected_assignment_id;
          if (expected !== (current?.assignment_id ?? null)) {
            return Response.json({ message: "Stale assignment identity" }, {
              status: 409,
            });
          }
          const newId = request.method === "DELETE"
            ? null
            : `assignment-new-${++assignmentNumber}`;
          if (current) {
            current.current = false;
            current.release = {
              action: newId ? "replaced" : "unassigned",
              actor: "fixture-owner",
              at: "2026-01-03T00:00:00Z",
              event_id: `release-${assignmentNumber}`,
              operation_key: operationKey,
              reason: "Explicit fixture operation",
              replacement_assignment_id: newId,
              last_tracking: structuredClone(current.tracking),
            };
            retained.set(current.assignment_id, structuredClone(current));
          }
          stored.action_eligibility.can_unassign_orchestrator = newId !== null;
          stored.assignment_tracking = [];
          stored.assignments = stored.assignments.filter((row) =>
            row.role !== "orchestrator"
          );
          if (newId) {
            const row: TicketAssignmentTrackingProjection = {
              workspace_id: id,
              ticket_id: stored.id,
              assignment_id: newId,
              assigned_at: "2026-01-03T00:00:00Z",
              assigned_by: "fixture-owner",
              current: true,
              role: "orchestrator",
              principal: body.principal,
              updated_at: "2026-01-03T00:00:00Z",
              last_event_id: `initial-${newId}`,
              release: null,
              migration_diagnostic: null,
              tracking: {
                status: "waiting",
                waiting_reason: "Initial judgment pending",
                next_action: null,
                progress: null,
                result: null,
              },
            };
            stored.assignment_tracking = [row];
            remember(row);
            stored.assignments.push({
              assignment_id: newId,
              role: row.role,
              assigned_at: row.assigned_at,
              assigned_by: row.assigned_by,
              principal: row.principal,
            });
            result = {
              workspace_id: id,
              ticket_id: stored.id,
              assignment: {
                workspace_id: id,
                ticket_id: stored.id,
                ...stored.assignments.at(-1),
              },
            };
          } else {result = {
              workspace_id: id,
              ticket_id: stored.id,
              assignment: null,
            };}
        } else if (
          suffix === "/assignment-answer" || suffix === "/assignment-tracking"
        ) {
          if (
            !current || current.assignment_id !== body.assignment_id ||
            current.last_event_id !== body.expected_last_event_id
          ) {
            return Response.json({
              message: "Stale assignment or observed handling event",
            }, { status: 409 });
          }
          if (suffix === "/assignment-answer") {
            if (
              current.tracking?.status === "done" || stored.state === "closed"
            ) {
              return Response.json({
                message: "Request a new assignment instead",
              }, { status: 409 });
            }
            stored.events.push({
              event_ref: `ticket-event:${stored.events.length}`,
              kind: "assignment_answer",
              body: body.body,
              sequence: stored.events.length,
              references: [],
              attributes: { orchestrator_assignment_id: current.assignment_id },
            });
            stored.event_count = stored.events.length;
            result = {
              assignment_id: current.assignment_id,
              input_event_id: `ticket-event:${stored.events.length - 1}`,
              event_index: stored.events.length - 1,
            };
          } else {
            current.tracking = body.tracking as TicketAssignmentTracking;
            current.last_event_id = `handling-update-${
              stored.events.length +
              (handlingHistory.get(current.assignment_id)?.length ?? 0)
            }`;
            current.updated_at = "2026-01-03T00:00:00Z";
            handlingHistory.get(current.assignment_id)!.push({
              at: current.updated_at,
              event_id: current.last_event_id,
              operation_key: operationKey,
              source_context: { actor: "fixture-orchestrator" },
              tracking: structuredClone(current.tracking),
            });
            result = current;
          }
        } else if (suffix === "/tags") {
          if (
            JSON.stringify(body.expected_tag_ids.toSorted()) !==
              JSON.stringify(stored.tags.map((tag) => tag.id).toSorted())
          ) {
            return Response.json({ message: "Tag set conflict" }, {
              status: 409,
            });
          }
          stored.tags = tags.filter((tag) =>
            !tag.deleted && body.tag_ids.includes(tag.id)
          );
          result = stored.tags;
        } else {
          if (
            body.expected_content_digest !== stored.content_digest ||
            body.expected_state !== stored.state ||
            body.expected_closure_operation_key !== stored.closure_operation_key
          ) {
            return Response.json({ message: "Case judgment conflict" }, {
              status: 409,
            });
          }
          const { action } = body;
          stored.closure_history.push({
            action,
            at: "2026-01-03T00:00:00Z",
            author: "fixture-owner",
            explanation: body.explanation,
            operation_key: operationKey,
            previous_closure_operation_key: stored.closure_operation_key,
            references: body.references ?? [],
          });
          if (action.action === "reopen") {
            stored.state = "open";
            stored.current_closure = null;
          } else {
            stored.state = "closed";
            stored.closure_operation_key = operationKey;
            stored.current_closure = {
              at: "2026-01-03T00:00:00Z",
              author: "fixture-owner",
              explanation: body.explanation,
              operation_key: operationKey,
              reason: action.reason,
              references: body.references ?? [],
            };
          }
          stored.action_eligibility.can_start_manual_worker =
            stored.state === "open";
          result = {
            id: stored.id,
            resource_key: key,
            slug: "fixture",
            kind: "task",
            labels: [],
            tags: stored.tags,
            title: stored.title,
            priority: stored.priority,
            status: stored.state === "open" ? "Open" : "Closed",
            workflow_state: stored.state,
            workflow_state_explicit: true,
          };
        }
        receipts.set(operationKey, {
          fingerprint,
          result: structuredClone(result),
        });
        if (injected === "unknown_saved") {
          return Response.json({
            message: "Outcome unknown; operation may already be saved",
          }, { status: 503 });
        }
        return Response.json(result);
      }
      if (request.method !== "GET") {
        const body = await request.json();
        bodies.push({ path, body });
        if (url.searchParams.get("failure")) {
          return Response.json({
            message: "Outcome unknown; refresh before retry",
          }, {
            status: 503,
          });
        }
        const stored = tickets.get(key) ?? (ticket(key), tickets.get(key)!);
        if (
          body.expected_content_digest &&
          body.expected_content_digest !== stored.content_digest
        ) {
          return Response.json(
            { message: "Ticket content conflict; refresh" },
            { status: 409 },
          );
        }
        if (tm[2] === "/events") {
          stored.events.push({
            sequence: stored.events.length,
            kind: body.role,
            body: body.body,
            references: [],
            attributes: {},
            event_ref: `event-${stored.events.length}`,
          });
        } else if (request.method === "PATCH") {
          if (body.target) stored.targets = body.target.targets ?? [];
          if (body.title) stored.title = body.title;
          if (body.body) stored.body = body.body;
        }
        stored.content_digest = contentDigest(stored);
        stored.event_count = stored.events.length;
      }
      return Response.json(projectedTicket(key, id));
    }
    return Response.json({ message: "Unavailable fixture resource" }, {
      status: 404,
    });
  }
  const relative = normalize(url.pathname.replace(/^\/+/, "") || "index.html");
  if (relative.startsWith("..")) {
    return new Response("not found", { status: 404 });
  }
  const mime: Record<string, string> = {
    ".html": "text/html",
    ".js": "text/javascript",
    ".css": "text/css",
    ".woff2": "font/woff2",
    ".svg": "image/svg+xml",
  };
  let file = join(root, relative);
  try {
    if ((await Deno.stat(file)).isDirectory) file = join(file, "index.html");
    return new Response(await Deno.readFile(file), {
      headers: {
        "content-type": mime[extname(file)] ?? "application/octet-stream",
      },
    });
  } catch {
    return new Response(await Deno.readFile(join(root, "index.html")), {
      headers: { "content-type": "text/html" },
    });
  }
});
