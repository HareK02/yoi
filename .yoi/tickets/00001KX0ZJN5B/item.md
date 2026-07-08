---
title: 'Move Memory extraction and consolidation to Backend observation pipeline'
state: 'planning'
created_at: '2026-07-08T13:44:47Z'
updated_at: '2026-07-08T13:46:41Z'
assignee: null
---

## 背景

Memory / Knowledge は Workspace product state であり、更新頻度が高く、自動抽出・consolidation によって人間の明示操作なしに変化する。Runtime / Worker が `.yoi/memory`、staging、consolidation lock、memory record を直接読む・書く構造を続けると、Workspace filesystem authority が Runtime 側に漏れ、child WorkingDirectory / remote Runtime / Backend API 境界と衝突する。

既存実装では、Worker post-run hook が memory extract worker を動かし、`<workspace>/.yoi/memory/_staging/<id>.json` に staging を書き、別の consolidation worker が `_staging` を snapshot/lock して `memory/*` / `knowledge/*` に統合する。これは local Worker では動くが、Runtime を Workspace filesystem から切り離す設計とは合わない。

今後は Runtime / Worker は Memory 正本を持たず、turn / transcript / tool usage / source refs から bounded な `MemoryObservation` を Backend に push する。Backend は Memory/Knowledge の正本、staging、audit、extraction、consolidation、tool read/write/query authority を持つ。

Workflow / active workflow state も同様に Backend API 境界へ寄せる必要があるが、compaction / history / runtime state と絡むため本 Ticket の非目標とする。

## 実装順序

- depends_on: `00001KX0G06VA` — Runtime -> Backend resource fetch REST API / handle 境界。Memory tool read/query の基礎として利用できる。
- 本 Ticket: Runtime -> Backend memory observation push と Backend-owned extraction/consolidation/store を実装する。
- 後続: Workflow / active workflow state の Backend API 化と Runtime tool/state migration を別 Ticket で扱う。

## 実装目的

- Runtime / Worker は `.yoi/memory` / `.yoi/knowledge` / `_staging` を直接読まない・書かない。
- Runtime / Worker は memory extraction の材料を bounded `MemoryObservation` として Backend に push する。
- Backend が Memory/Knowledge store、staging、audit log、extraction、consolidation、tidy の authority を持つ。
- Memory / Knowledge tools は Backend API 経由で read/query/write/edit する。
- Backend は observation / extraction / consolidation の source provenance、redaction、audit、diagnostics を管理する。
- child WorkingDirectory や remote Runtime が Memory root を持たなくても memory 機能が動く。

## 実装内容

### 1. MemoryObservation model を追加する

Runtime / Worker から Backend へ送る memory extraction input を typed model として定義する。

含めるもの:

- workspace id。
- runtime id / worker id / session id / turn id。
- source segment refs / transcript range refs。
- bounded user / assistant / tool excerpts。
- tool call summaries / tool result summaries。
- active Ticket / Objective refs if known。
- model/provider/provenance metadata。
- extraction trigger reason。
- redaction / privacy flags。
- observed_at timestamp。

含めないもの:

- full unbounded transcript。
- raw host absolute path。
- secret values / credentials / tokens。
- Runtime endpoint / socket path / session store path。
- WorkingDirectory root path。
- raw binary tool outputs。

### 2. Runtime -> Backend observation push API を追加する

Runtime が Backend に MemoryObservation を push する API を追加する。

想定 endpoint:

```text
POST /internal/runtime/memory/observations
```

実装すること:

- embedded Runtime direct call path。
- remote Runtime HTTP path。
- request/response typed schema。
- runtime credential / workspace-runtime-worker binding validation。
- max bytes / max excerpts / max tool summaries / max pending observations。
- timeout / retry / idempotency key。
- rejected / accepted / queued diagnostics。

### 3. Backend memory observation store / audit を追加する

Backend は observation を append-only に受け取り、audit と extraction queue に接続する。

実装すること:

- observation validation。
- source provenance / correlation id。
- append-only observation log。
- duplicate idempotency handling。
- redaction before persistence。
- diagnostic / audit event。
- bounded retention policy。

### 4. Backend-owned extraction pipeline を実装する

Memory extraction worker は Runtime-local filesystem staging ではなく、Backend-owned observation queue を入力にする。

実装すること:

- observation batch selection。
- extraction prompt input builder。
- extracted payload validation。
- staging record creation in Backend-owned store。
- source refs are attached mechanically by Backend, not inferred by LLM。
- extraction failure diagnostics / retry policy。

### 5. Backend-owned consolidation pipeline を実装する

Consolidation は Backend-owned staging / memory / knowledge store に対して行う。

実装すること:

- Backend-owned staging snapshot / lock。
- consolidation threshold by file/count/bytes or observation count/bytes。
- Memory/Knowledge read/write/edit tools for consolidation worker backed by Backend service layer。
- consumed staging cleanup。
- tidy hints / outdated / superseded / unused / noisy handling。
- audit log and UI observability event。

### 6. Memory / Knowledge tools を Backend API 経由に移行する

Runtime / Worker tool implementations for memory and knowledge must call Backend API rather than local memory layout.

対象:

- `MemoryQuery`
- `KnowledgeQuery`
- `MemoryRead`
- `MemoryWrite`
- `MemoryEdit`
- `MemoryDelete`
- Knowledge write/edit/delete equivalents if present。

実装すること:

- tool context に Backend memory client / capability を渡す。
- read/query と mutation の authority を分ける。
- mutation は Backend service layer の validation / lint / audit を通す。
- local direct layout path は Backend internal implementation or compatibility tests に隔離する。

### 7. Compatibility and migration

既存 `.yoi/memory` repo-local store は Backend internal local provider として扱う。Runtime / Worker から直接使わない。

実装すること:

- `memory::WorkspaceLayout` は Backend local provider の内部実装として残してよい。
- `yoi memory lint` CLI は Backend local provider / compatibility command として維持する。
- existing records and staging compatibility migration policy を決める。
- child worktree が `.yoi/tickets` / `.yoi/workflow` を持っていても memory root として扱われない invariant を維持する。

## 受け入れ条件

- `MemoryObservation` typed model が実装されている。
- Runtime -> Backend memory observation push API が embedded direct path と remote HTTP path で実装されている。
- Backend が MemoryObservation を validate / redact / append-only persist / audit できる。
- Backend-owned extraction pipeline が observation から staging record を生成できる。
- Backend-owned consolidation pipeline が Backend-owned staging を Memory/Knowledge 正本へ統合できる。
- Memory / Knowledge tools は Runtime から local `.yoi/memory` filesystem を読まず Backend API/capability 経由で動く。
- Runtime が Memory root / `.yoi/memory` / `.yoi/knowledge` / `_staging` を持たない構成で focused tests が通る。
- source refs / provenance は Backend が機械付与し、LLM に推論させない。
- full unbounded transcript、secret values、raw host path、Runtime endpoint、WorkingDirectory root が observation / tool result / Browser-facing response に露出しない。
- duplicate observation、oversized observation、unauthorized runtime/worker/workspace binding、invalid extracted payload、consolidation lock conflict は typed diagnostic になる。
- Existing `yoi memory lint` and local memory linter behavior remain available as Backend-local/compatibility surface。
- `cargo test -p memory` が通る。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- Workflow / active workflow state API/tool migration。
- Ticket / Objective API/tool migration。これは `00001KX0V3DB0` で扱う。
- Runtime-to-Backend resource fetch REST API foundation。これは `00001KX0G06VA` で扱う。
- Memory schema redesign beyond what is required for observation/staging ownership。
- Vector search / embedding store。
- Multi-user auth / RBAC の完成。
- Browser Memory management UI の完成。
