# Workspace Web Drive

The Web Drive is a latest-only shared Workspace resource, independent of Git, repositories, Worker sessions and Workdir occupancy. The Server's DB node tree, last committed request, receipt and current authorization are authoritative. The UI never uses physical blob keys/paths or stores a second persistent hierarchy.

## References and authorization

`/w/<workspace>/drive/<node-id>` is a human-facing stable latest URL. Its node ID remains a canonical positive decimal **string** (including values above JavaScript's safe-integer range). Rename/move retain the URL; deleting and recreating a name does not revive the deleted ID. Copy latest URL emits this Web route, not the API's safe binary download URL. Ticket/Objective/comment Markdown can link to this route. Possessing any URL/ID grants no access: metadata, content, mutations, download and receipt queries all use the current authenticated API.

Ordinary Drive usage retains the existing member-facing API policy, not owner-only settings policy. Worker grants are owner-controlled on the existing Workers page, using its current authoritative Worker catalog. Read-only/read-write are explicit Backend grants, not consequences of enabling a Profile tool. The UI does not invent a Web read-only ACL: Backend denial is displayed and suppresses mutations. Worker read-only denial and revoke are separately exercised against the real API.

## Drafts, observed changes and delayed operations

The optional node route reuses one controller across Workspace/entry switches. Drafts and mutation receipts are keyed by Workspace/node identity in this page session. Reads/writes/status/list/search/ancestor/image completion are generation fenced, including A→B→A. No delayed response can select a new target or change its draft/save destination. Leaving the Drive page prompts before destroying a dirty draft or unresolved receipt; they are not claimed to survive a browser reload.

Text edits retain the `last_mutation_id` returned with the read and send it as `expected_mutation_id`. A conflict preserves the draft and that original observation, displays an explicit conflict and never silently rebases, automerges or resubmits. Refresh can show the latest saved text alongside the draft. Discard/load-latest is an explicit destructive user decision. Partial previews cannot be saved. Save acknowledgments retain edits made while saving.

## Transfer and media policy

- Markdown/plain text: 64 KiB UTF-8 read/edit limit. Markdown uses the existing raw HTML-rejecting safe document renderer. Other active/binary media are download-only.
- Raster image preview: PNG/JPEG/GIF/WebP, authenticated committed-request-bound fetch, 256 KiB bounded bytes/Blob; decode/load failure is explicit. Object URLs are released on switching/destruction. Larger images remain downloadable.
- Download: native same-origin authenticated binary endpoint, bound to the observed committed request; Server's attachment/nosniff/sandbox/no-store headers remain authoritative.
- Markdown inline images never automatically fetch external/cross-Workspace/API resources. They display alternative text and require explicit navigation to an authorized Drive URL. Same-Workspace resource links navigate normally; external HTTP(S) links have no userinfo, open a new tab with `noreferrer`; script/data/file, protocol-relative, arbitrary API/relative paths are not links.
- Upload/replacement: at most 16 MiB raw Blob request, not JSON/base64 or DOM bytes. SHA-256 needs one bounded ArrayBuffer; there is no constant-memory browser hashing claim. Transfer/pending is distinct from the DB-published acknowledgment.
- A request ID is allocated before a mutation. Transport/response loss or abort after transmission is conservatively unknown, not success/failure. Its current authorized receipt endpoint is queried; no new request or automatic reupload is generated. `uncommitted` means no commit observed yet, not proof that an in-flight publication cannot still finish. Unknown requests block new writes to that entry.

Name conflict, changed-content conflict, limit, denied, missing node, storage/I/O failure and outcome unknown are not translated into empty contents/empty Drive/success. Initial folder deletion is empty-only with explicit confirmation; no recursive operation or blob-copy rename is introduced.

## Validation

Browser API fixtures and real authority evidence are intentionally separate:

- `web/workspace/src/lib/workspace/drive/*test.ts`: malformed payload/ID and transport/state races at the client/controller boundary.
- `drive-ui`/`drive-grants` component tests: actual safe rendering/interaction.
- `web/workspace/test/drive-browser`: production static shell + synthetic API fixtures, pinned Chromium interactions and visual evidence.
- `crates/workspace-server/src/server/tests/drive_web_roundtrip_tests.rs` and `web/workspace/test/drive-real-api`: hermetic real Server HTTP, SQLite, FeatureStorage and LocalFileSystem, repositoryless bootstrap and production Web adapter. Only the Runtime process boundary is fake; no live dogfood data is used.

See the test READMEs and T-724 validation record for exact commands/results, artifact paths, source commits and any deliberate failing fixture state.
