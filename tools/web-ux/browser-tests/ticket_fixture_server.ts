import { createHash } from "node:crypto";
// Synthetic API fixture serving the production static UI; no Worker/Git execution.
import { extname, join, normalize } from "jsr:@std/path@1.1.4";
import {
  fixtureDetail,
  fixturePage,
} from "../../../web/workspace/src/lib/workspace/home/dashboard.test-fixtures.ts";
import type { TicketDetail } from "../../../web/workspace/src/lib/generated/ticket-api.ts";
import { emptyLaunchOptions } from "../../../web/workspace/src/lib/workspace/sidebar/worker-launch.test-fixtures.ts";
const port = Number(Deno.args[0]);
const root = Deno.args[1];
const legacy = Deno.args.includes("--legacy");
const tickets = new Map<string, TicketDetail>();
const bodies: unknown[] = [];
function contentDigest(value: TicketDetail): string {
  return createHash("sha256").update("ticket.content.sha256.v1\0").update(JSON.stringify([
    value.id, value.title, value.body, value.targets.map(({ repository_key, ref_selector, access }) => ({ repository_key, ref_selector, access })),
  ])).digest("hex");
}
function ticket(key: string) {
  if (!tickets.has(key)) {
    const value: TicketDetail = fixtureDetail(key);
    value.state = "planning";
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
    value.action_eligibility.can_queue = true;
    value.action_eligibility.queue_tickets = [key];
    value.action_eligibility.can_start_manual_worker = true;
    if (key === "T-2") {
      value.targets = [{ repository_key: "main", ref_selector: "develop", access: "read_only" }];
    }
    if (key === "T-3") {
      value.merge_requests = [{
        merge_request_id: "mr-1",
        repository_key: "main",
        state: "open",
        selector_from: "work/fixture",
        selector_to: "develop",
        review_status: "none",
        source_ref_observation: { status: "unavailable", code: "provider_unavailable" },
        current_subject_ref: null,
        review_subject_ref: null,
        review_excerpt: null,
        integration_evidence_error: null,
        updated_at: "2026-01-02T00:00:00Z",
      }];
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
Deno.serve({ hostname: "127.0.0.1", port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  if (url.pathname === "/fixture-state") return Response.json(bodies);
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
  const match = url.pathname.match(/^\/api\/w\/([^/]+)(\/.*)$/);
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
          host_worker_bridge: { status: "ready", note: "fixture", diagnostics: [] },
          companion_console: { status: "ready", note: "fixture", diagnostics: [] },
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
        online: false,
        disposition: "offline",
        worker: null,
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
    if (path === "/protocol/ws" && request.headers.get("upgrade") === "websocket") {
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
    if (path === "/tickets" && request.method === "POST") {
      const body = await request.json();
      bodies.push({ path, body });
      ticket("T-4");
      const created = tickets.get("T-4")!;
      created.title = body.title;
      created.body = body.body;
      created.targets = body.targets ?? [];
      return Response.json({
        id: created.id,
        resource_key: "T-4",
        slug: "fixture",
        status: "Open",
      });
    }
    if (path === "/tickets") {
      const items = ["T-1", "T-2", "T-3"].map(ticket).filter((t) =>
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
      if (request.method !== "GET") {
        const body = await request.json();
        bodies.push({ path, body });
        if (url.searchParams.get("failure")) {
          return Response.json({ message: "Outcome unknown; refresh before retry" }, {
            status: 503,
          });
        }
        const stored = tickets.get(key) ?? (ticket(key), tickets.get(key)!);
        if (body.expected_content_digest && body.expected_content_digest !== stored.content_digest) {
          return Response.json({ message: "Ticket content conflict; refresh" }, { status: 409 });
        }
        if ((tm[2] === "/state" || tm[2] === "/close") && body.expected_state !== stored.state) {
          return Response.json({ message: "Ticket state conflict; refresh" }, { status: 409 });
        }
        if (tm[2] === "/state") {
          stored.state = body.state;
          stored.events.push({
            sequence: stored.events.length,
            kind: "state_changed",
            body: body.body,
            reason: body.reason,
            from: body.expected_state,
            to: body.state,
            references: [],
            attributes: {},
            event_ref: `event-${stored.events.length}`,
          });
        } else if (tm[2] === "/events") {
          stored.events.push({
            sequence: stored.events.length,
            kind: body.role,
            body: body.body,
            references: [],
            attributes: {},
            event_ref: `event-${stored.events.length}`,
          });
        } else if (tm[2] === "/close") {
          stored.state = "closed";
          stored.resolution = body.resolution;
        } else if (tm[2] === "/ready") stored.state = "ready";
        else if (tm[2] === "/queue") {
          stored.state = "queued";
          return Response.json({ requested_ticket: key, queued_tickets: [key] });
        } else if (request.method === "PATCH") {
          if (body.target) stored.targets = body.target.targets ?? [];
          if (body.title) stored.title = body.title;
          if (body.body) stored.body = body.body;
        }
        stored.content_digest = contentDigest(stored);
        stored.event_count = stored.events.length;
      }
      return Response.json(ticket(key));
    }
    return Response.json({ message: "Unavailable fixture resource" }, { status: 404 });
  }
  const relative = normalize(url.pathname.replace(/^\/+/, "") || "index.html");
  if (relative.startsWith("..")) return new Response("not found", { status: 404 });
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
      headers: { "content-type": mime[extname(file)] ?? "application/octet-stream" },
    });
  } catch {
    return new Response(await Deno.readFile(join(root, "index.html")), {
      headers: { "content-type": "text/html" },
    });
  }
});
