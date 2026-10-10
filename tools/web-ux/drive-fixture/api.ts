// Browser-only, in-memory API fixture. This is NOT Backend authorization/storage evidence.
import type {
  DriveEntry,
  DriveEntryRef,
  DriveGrantResponse,
  DriveMutation,
  DriveMutationResponse,
} from "../../../web/workspace/src/lib/generated/drive-api.ts";
import { fixturePng } from "./raster.ts";

type Node = { entry: DriveEntry; bytes: Uint8Array };
type Workspace = {
  nodes: Map<string, Node>;
  receipts: Map<string, DriveMutationResponse>;
  grants: DriveGrantResponse[];
  next: bigint;
};
export type FixtureLog = {
  workspace: string;
  path: string;
  method: string;
  requestId: string | null;
  nodeId: string | null;
  expectedMutationId: string | null;
};
const encoder = new TextEncoder();
const rootId = "9007199254740993";
export const fixtureRootId = rootId;
const instant = "2026-01-02T00:00:00Z";
const errors = {
  denied: 403,
  not_found: 404,
  conflict: 409,
  invalid: 400,
  limit: 413,
  storage_unavailable: 503,
  outcome_unknown: 503,
} as const;
function failure(code: keyof typeof errors): Response {
  return Response.json({
    code,
    classification: code === "outcome_unknown" ? "unknown" : "not_committed",
    message: `Drive fixture ${code}`,
  }, { status: errors[code] });
}
export function createDriveFixture() {
  const workspaces = new Map<string, Workspace>();
  const log: FixtureLog[] = [];
  const faults = new Map<string, string>();
  function ws(id: string): Workspace {
    let state = workspaces.get(id);
    if (state) return state;
    state = { nodes: new Map(), receipts: new Map(), grants: [], next: 100n };
    workspaces.set(id, state);
    const add = (
      nodeId: string,
      name: string,
      kind: "folder" | "file",
      parent: string | null,
      contentType: string | null,
      bytes = new Uint8Array(),
    ) => {
      const ref = (n: string): DriveEntryRef => ({ workspace_id: id, node_id: n });
      state!.nodes.set(nodeId, {
        bytes,
        entry: {
          entry: ref(nodeId),
          parent: parent === null ? null : ref(parent),
          name,
          kind,
          last_mutation_id: "1",
          size: kind === "file" ? bytes.length : null,
          content_type: contentType,
          updated_by: "fixture-worker",
          updated_at: instant,
          latest_url: `/api/w/${encodeURIComponent(id)}/drive/${
            kind === "file" ? "download" : "metadata"
          }?entry_workspace_id=${encodeURIComponent(id)}&id=${nodeId}`,
        },
      });
    };
    add(rootId, "Drive", "folder", null, null);
    if (id.includes("empty")) return state;
    add("2", "資料", "folder", rootId, null);
    add(
      "3",
      "README.md",
      "file",
      rootId,
      "text/markdown",
      encoder.encode(
        `# Shared notes\n\nWorker-created latest-only document.\n\n[Local folder](/w/${
          encodeURIComponent(id)
        }/drive/2)\n\n[Unsafe](javascript:alert(1))\n\n<img src=x onerror=alert(1)>\n\n![External](https://example.invalid/credential-trap.png)\n\n![Image](/w/${
          encodeURIComponent(id)
        }/drive/5)\n`,
      ),
    );
    add("4", "notes.txt", "file", "2", "text/plain", encoder.encode("Second document — 日本語"));
    add("5", "landscape.png", "file", rootId, "image/png", fixturePng);
    add("6", "empty.txt", "file", rootId, "text/plain");
    add("7", "broken.png", "file", rootId, "image/png", encoder.encode("not a valid PNG"));
    add(
      "8",
      "unsafe.svg",
      "file",
      rootId,
      "image/svg+xml",
      encoder.encode('<svg onload="alert(1)"></svg>'),
    );
    add(
      "9",
      "large.txt",
      "file",
      rootId,
      "text/plain",
      encoder.encode("bounded preview\n".repeat(5000)),
    );
    add(
      "10",
      "長い名前 — 国際化された資料と分散開発の成果物 — ".repeat(2) + ".md",
      "file",
      rootId,
      "text/markdown",
      encoder.encode("# Long name"),
    );
    add("11", "escaped-64k.txt", "file", rootId, "text/plain", new Uint8Array(65536));
    if (id.includes("paged")) {
      for (let n = 20; n < 260; n++) {
        add(
          String(n),
          `Document ${n}.txt`,
          "file",
          rootId,
          "text/plain",
          encoder.encode(String(n)),
        );
      }
    }
    state.next = id.includes("paged") ? 300n : 100n;
    return state;
  }
  function mutate(id: string, requestId: string, mutation: DriveMutation): DriveEntry | null {
    const state = ws(id);
    const target = "id" in mutation ? state.nodes.get(mutation.id.node_id) : null;
    if ("id" in mutation && (mutation.id.workspace_id !== id || !target)) throw "not_found";
    if (
      target && "expected_mutation_id" in mutation &&
      target.entry.last_mutation_id !== mutation.expected_mutation_id
    ) throw "conflict";
    if ("parent" in mutation) {
      const parent = state.nodes.get(mutation.parent.node_id);
      if (mutation.parent.workspace_id !== id || parent?.entry.kind !== "folder") throw "not_found";
    }
    if ("name" in mutation) {
      if (
        !mutation.name || mutation.name.includes("/") || encoder.encode(mutation.name).length > 255
      ) throw "invalid";
      if (
        [...state.nodes.values()].some((n) =>
          n !== target && n.entry.parent?.node_id === mutation.parent.node_id &&
          n.entry.name === mutation.name
        )
      ) throw "conflict";
    }
    if (mutation.operation === "delete") {
      if (
        target!.entry.parent === null ||
        [...state.nodes.values()].some((n) => n.entry.parent?.node_id === mutation.id.node_id)
      ) throw "conflict";
      state.nodes.delete(mutation.id.node_id);
      return null;
    }
    if (mutation.operation === "relocate") {
      target!.entry.parent = mutation.parent;
      target!.entry.name = mutation.name;
      target!.entry.last_mutation_id = requestId;
      return target!.entry;
    }
    if (mutation.operation === "update_text") {
      target!.bytes = encoder.encode(mutation.text);
      target!.entry.size = target!.bytes.length;
      target!.entry.content_type = mutation.content_type;
      target!.entry.last_mutation_id = requestId;
      return target!.entry;
    }
    const nodeId = String(state.next++);
    const bytes = mutation.operation === "create_text"
      ? encoder.encode(mutation.text)
      : new Uint8Array();
    const entry: DriveEntry = {
      entry: { workspace_id: id, node_id: nodeId },
      parent: mutation.parent,
      name: mutation.name,
      kind: mutation.operation === "create_folder" ? "folder" : "file",
      last_mutation_id: requestId,
      size: mutation.operation === "create_folder" ? null : bytes.length,
      content_type: mutation.operation === "create_text" ? mutation.content_type : null,
      updated_by: "fixture-user",
      updated_at: instant,
      latest_url: `/api/w/${encodeURIComponent(id)}/drive/${
        mutation.operation === "create_folder" ? "metadata" : "download"
      }?entry_workspace_id=${encodeURIComponent(id)}&id=${nodeId}`,
    };
    state.nodes.set(nodeId, { entry, bytes });
    return entry;
  }
  async function handler(request: Request): Promise<Response | null> {
    const url = new URL(request.url);
    if (url.pathname === "/__fixture/log") return Response.json(log);
    if (url.pathname === "/__fixture/reset" && request.method === "POST") {
      workspaces.clear();
      faults.clear();
      log.length = 0;
      return Response.json({ ok: true });
    }
    if (url.pathname === "/__fixture/fault" && request.method === "POST") {
      const body = await request.json();
      faults.set(body.workspace, body.mode);
      return Response.json({ ok: true });
    }
    const match = url.pathname.match(/^\/api\/w\/([^/]+)\/drive(\/.*)$/);
    if (!match) return null;
    const id = decodeURIComponent(match[1]), path = match[2];
    const state = ws(id);
    const mutationRoute = path === "/mutate" || path === "/upload";
    const event: FixtureLog = {
      workspace: id,
      path,
      method: request.method,
      requestId: url.searchParams.get("request_id"),
      nodeId: url.searchParams.get("id") ?? url.searchParams.get("parent_id"),
      expectedMutationId: url.searchParams.get("expected_mutation_id"),
    };
    log.push(event);
    if (id.includes("denied")) return failure("denied");
    if (id.includes("offline") || id === "home-error") return failure("storage_unavailable");
    if (mutationRoute && (id.includes("readonly") || id === "home-member")) {
      return failure("denied");
    }
    if (path.startsWith("/requests/")) {
      const requestId = decodeURIComponent(path.slice(10));
      const response = state.receipts.get(requestId) ?? null;
      return Response.json({
        request_id: requestId,
        state: response ? "committed" : "uncommitted",
        response,
      });
    }
    if (path === "/grants") {
      if (id === "home-member") return failure("denied");
      if (request.method === "POST") {
        const input = await request.json();
        const grant: DriveGrantResponse = {
          ...input,
          grant_id: String(state.grants.length + 1),
          workspace_id: id,
          revoked: false,
          created_by: "fixture-owner",
          created_at: instant,
          revoked_by: null,
          revoked_at: null,
        };
        state.grants.push(grant);
        return Response.json(grant);
      }
      return Response.json({ grants: state.grants, next_after: null });
    }
    if (/^\/grants\/\d+$/.test(path) && request.method === "DELETE") {
      if (id === "home-member") return failure("denied");
      const grant = state.grants.find((g) => g.grant_id === path.split("/")[2]);
      if (!grant) return failure("not_found");
      grant.revoked = true;
      grant.revoked_at = instant;
      grant.revoked_by = "fixture-owner";
      return Response.json(grant);
    }
    if (mutationRoute) {
      try {
        const payload = path === "/mutate" ? await request.json() : null;
        const requestId = path === "/upload"
          ? url.searchParams.get("request_id")!
          : payload.request_id;
        event.requestId = requestId;
        if (payload) {
          event.nodeId = payload.mutation.id?.node_id ?? payload.mutation.parent?.node_id ?? null;
          event.expectedMutationId = payload.mutation.expected_mutation_id ?? null;
        }
        if (state.receipts.has(requestId)) return Response.json(state.receipts.get(requestId));
        const fault = faults.get(id);
        if (fault === "conflict") {
          faults.delete(id);
          return failure("conflict");
        }
        let entry: DriveEntry | null;
        if (path === "/mutate") entry = mutate(id, requestId, payload.mutation);
        else {
          const bytes = new Uint8Array(await request.arrayBuffer());
          if (bytes.length > 16 * 1024 * 1024) return failure("limit");
          if (bytes.length !== Number(url.searchParams.get("size"))) return failure("invalid");
          const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
          if (
            [...digest].map((n) => n.toString(16).padStart(2, "0")).join("") !==
              url.searchParams.get("sha256")
          ) return failure("invalid");
          const content_type = url.searchParams.get("content_type")!;
          if (url.searchParams.get("operation") === "create") {
            entry = mutate(id, requestId, {
              operation: "create_text",
              parent: { workspace_id: id, node_id: url.searchParams.get("parent_id")! },
              name: url.searchParams.get("name")!,
              text: "",
              content_type,
            });
          } else {entry = mutate(id, requestId, {
              operation: "update_text",
              id: { workspace_id: id, node_id: url.searchParams.get("id")! },
              expected_mutation_id: url.searchParams.get("expected_mutation_id")!,
              text: "",
              content_type,
            });}
          const node = state.nodes.get(entry!.entry.node_id)!;
          node.bytes = bytes;
          node.entry.size = bytes.length;
        }
        const response = structuredClone({ request_id: requestId, entry });
        state.receipts.set(requestId, response);
        if (fault === "unknown") {
          faults.delete(id);
          return failure("outcome_unknown");
        }
        return Response.json(response);
      } catch (error) {
        return failure(
          typeof error === "string" && error in errors ? error as keyof typeof errors : "invalid",
        );
      }
    }
    const nodeId = path === "/root" ? rootId : url.searchParams.get("id");
    if (nodeId !== null && path !== "/root" && url.searchParams.get("entry_workspace_id") !== id) {
      return failure("invalid");
    }
    const node = nodeId ? state.nodes.get(nodeId) : null;
    if (path === "/root" || path === "/metadata") {
      return node ? Response.json(node.entry) : failure("not_found");
    }
    if (path === "/list" || path === "/search") {
      if (path === "/list" && node?.entry.kind !== "folder") return failure("not_found");
      const all = [...state.nodes.values()].filter((n) =>
        path === "/search"
          ? n.entry.name.includes(url.searchParams.get("query") ?? "")
          : n.entry.parent?.node_id === nodeId
      ).sort((a, b) => a.entry.name.localeCompare(b.entry.name));
      const after = url.searchParams.get("after");
      const start = after ? all.findIndex((n) => n.entry.entry.node_id === after) + 1 : 0;
      const limit = Math.min(Number(url.searchParams.get("limit") ?? 50), 200);
      const entries = all.slice(start, start + limit).map((n) => n.entry);
      return Response.json({
        entries,
        next_after: start + limit < all.length ? entries.at(-1)!.entry.node_id : null,
      });
    }
    if (!node || node.entry.kind !== "file") return failure("not_found");
    if (path === "/read-text") {
      const text = new TextDecoder().decode(node.bytes.slice(0, 65536));
      return Response.json({ entry: node.entry, text, truncated: node.bytes.length > 65536 });
    }
    if (path === "/download" || path === "/read-chunk") {
      if (
        url.searchParams.has("expected_mutation_id") &&
        url.searchParams.get("expected_mutation_id") !== node.entry.last_mutation_id
      ) return failure("conflict");
      if (faults.get(id) === "image_error" && node.entry.content_type?.startsWith("image/")) {
        return failure("storage_unavailable");
      }
      const bytes = path === "/read-chunk"
        ? node.bytes.slice(
          Number(url.searchParams.get("offset")),
          Number(url.searchParams.get("offset")) + Number(url.searchParams.get("length")),
        )
        : node.bytes;
      return new Response(new Uint8Array(bytes).buffer, {
        headers: {
          "content-type": node.entry.content_type!,
          "content-disposition": "attachment",
          "x-content-type-options": "nosniff",
        },
      });
    }
    return failure("not_found");
  }
  return { handler, log };
}
