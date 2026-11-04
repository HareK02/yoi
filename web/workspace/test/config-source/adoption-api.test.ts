/// <reference lib="deno.ns" />

import { assertEquals, assertRejects, assertThrows } from "jsr:@std/assert";
import {
  commitConfigTree,
  ConfigSourceApiError,
  fetchConfigTree,
  parseConfigurationAdoptionReceipt,
  parseConfigurationReconciliation,
  parseConfigurationSourceResponse,
  previewConfigTree,
  reconcileConfigTree,
} from "../../src/lib/workspace/config-source/api.ts";
import type { ConfigurationAdoptionRequest } from "#lib/generated/configuration-adoption.generated.ts";
import type { WorkspaceConfigTreeResponse } from "../../src/lib/workspace/config-source/types.ts";

const state: WorkspaceConfigTreeResponse = {
  snapshot: { digest: "sha256:content-A", entries: {} },
  contract: {
    contract_version: 1, decodal_version: "0.4.0", schema_version: 1,
    entrypoints: ["main.dcdl"], import_policy_version: 1,
    schema_bundle: { contributions: [], source: "builtin", fingerprint: "sha256:schema" },
    fingerprint: "sha256:contract",
  },
  projection_digest: "sha256:projection",
};
const source = {
  state, history_head: null, source_stored: false,
  catalog: { workspace: "w/one", namespaces: [] },
  catalog_digest: "sha256:catalog", expected_legacy_export: "sha256:legacy",
  policies: [], migration_diagnostics: [],
};
const request: ConfigurationAdoptionRequest = {
  operation_id: "operation:one", expected_head: "head:A-after-B",
  expected_catalog_digest: source.catalog_digest,
  expected_legacy_export: source.expected_legacy_export,
  source: { base_digest: state.snapshot.digest, changes: [], entrypoints: ["main.dcdl"] },
};
const receipt = {
  workspace_id: "w/one", operation_id: request.operation_id, history_head: "head:adopted",
  state, source_stored: true, policy_committed: true, runtime_application: "next_current_policy_check",
};
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const replies = (...bodies: unknown[]) =>
  (() => Promise.resolve(json(bodies.shift()))) as typeof fetch;

Deno.test("adoption source parser retains nullable observed metadata separately from content identity", () => {
  const parsed = parseConfigurationSourceResponse(source);
  assertEquals(parsed.state, state);
  assertEquals(parsed.history_head, null);
  assertEquals(parsed.source_stored, false);
  assertEquals(parsed.expected_legacy_export, "sha256:legacy");
  // A -> B -> A can retain content-A while having a different editing head.
  assertEquals(parseConfigurationSourceResponse({ ...source, history_head: "head:A-after-B" }).history_head, request.expected_head);
});

Deno.test("adoption source parser rejects missing metadata, unknown envelope fields and unbounded opaque collections", () => {
  const { history_head: _head, ...missingHead } = source;
  const { expected_legacy_export: _export, ...missingExport } = source;
  for (const invalid of [
    state, missingHead, missingExport,
    { ...source, unexpected: true }, { ...source, history_head: 1 },
    { ...source, catalog_digest: "x".repeat(257) },
    { ...source, expected_legacy_export: false }, { ...source, source_stored: null },
    { ...source, policies: {} }, { ...source, migration_diagnostics: Array(257).fill({}) },
    { ...source, catalog: { namespaces: Array(257).fill({}) } },
    { ...source, state: { ...state, snapshot: { ...state.snapshot, unexpected: true } } },
  ]) assertThrows(() => parseConfigurationSourceResponse(invalid), ConfigSourceApiError);
});

Deno.test("adoption transport posts the full unchanged request to owner adoption routes, including reconcile", async () => {
  const calls: Array<{ url: string; body?: BodyInit | null }> = [];
  const fetcher = ((input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, body: init?.body });
    const body = url.endsWith("/preview")
      ? { expected_head: request.expected_head, state, changed_paths: [], changed_subjects: [] }
      : url.endsWith("/commit") ? receipt
      : url.endsWith("/reconcile") ? { status: "adopted", receipt }
      : source;
    return Promise.resolve(json(body));
  }) as typeof fetch;
  await fetchConfigTree("w/one", fetcher);
  await previewConfigTree("w/one", request, fetcher);
  await commitConfigTree("w/one", request, fetcher);
  await reconcileConfigTree("w/one", request, fetcher);
  assertEquals(calls.map(({ url }) => url), [
    "/api/w/w%2Fone/config/adoption", "/api/w/w%2Fone/config/adoption/preview",
    "/api/w/w%2Fone/config/adoption/commit", "/api/w/w%2Fone/config/adoption/reconcile",
  ]);
  for (const call of calls.slice(1)) assertEquals(JSON.parse(String(call.body)), request);
});

Deno.test("preview does not refresh an editing head from an unexpected response", async () => {
  await assertRejects(() => previewConfigTree("w/one", request, replies({
    expected_head: "head:new", state, changed_paths: [], changed_subjects: [],
  })), ConfigSourceApiError, "invalid response");
  assertEquals(request.expected_head, "head:A-after-B");
});

Deno.test("receipt requires committed source and policy and matches the requested workspace and operation", async () => {
  for (const invalid of [
    { ...receipt, source_stored: false }, { ...receipt, policy_committed: false },
    { ...receipt, runtime_application: "already_running" },
    { ...receipt, history_head: null }, { ...receipt, extra: true },
  ]) assertThrows(() => parseConfigurationAdoptionReceipt(invalid), ConfigSourceApiError);
  for (const invalid of [
    { ...receipt, workspace_id: "other" }, { ...receipt, operation_id: "other" },
  ]) {
    await assertRejects(() => commitConfigTree("w/one", request, replies(invalid)), ConfigSourceApiError);
    await assertRejects(() => reconcileConfigTree("w/one", request, replies({ status: "adopted", receipt: invalid })), ConfigSourceApiError);
  }
});

Deno.test("not_observed and pending_transfer remain distinct unknown evidence, never a failed-save receipt", () => {
  for (const evidence of [
    { status: "not_observed" },
    { status: "pending_transfer", transfer_identity: "transfer:one", export_digest: "sha256:legacy" },
    { status: "adopted", receipt },
  ]) assertEquals<unknown>(parseConfigurationReconciliation(evidence), evidence);
  for (const invalid of [
    { status: "failed" }, { status: "not_observed", receipt },
    { status: "pending_transfer", transfer_identity: "transfer:one" },
    { status: "adopted" },
  ]) assertThrows(() => parseConfigurationReconciliation(invalid), ConfigSourceApiError);
});

Deno.test("lost or invalid responses do not retry or change request identity inside the adapter", async () => {
  for (const failure of ["network", "malformed", "oversized", "server"]) {
    let calls = 0;
    const fetcher = (() => {
      calls++;
      if (failure === "network") return Promise.reject(new TypeError("response lost"));
      return Promise.resolve(failure === "malformed" ? new Response("not JSON")
        : failure === "oversized" ? new Response("{}", { headers: { "content-length": String(8 * 1024 * 1024 + 1) } })
        : new Response("outcome unavailable", { status: 500 }));
    }) as typeof fetch;
    await assertRejects(() => commitConfigTree("w/one", request, fetcher));
    assertEquals(calls, 1);
    assertEquals(request.operation_id, "operation:one");
    assertEquals(request.expected_head, "head:A-after-B");
  }
});
