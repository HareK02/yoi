<!-- event: create author: "yoi ticket" at: 2026-06-28T13:53:21Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-28T14:20:04Z -->

## Decision

調査結果:

- `crates/pod` は現 workspace には存在しない。`Cargo.toml` workspace members / workspace dependencies にも `crates/pod` / `pod = ...` はない。したがって「pod crate を消す」作業は既に過去の移行で遂行済みで、この Ticket では不在確認と再導入防止を扱う。
- 残っている旧 Pod 関連 crate は `crates/pod-registry` と `crates/pod-store`。
- これらが残っている理由は、旧 Pod UI crate ではなく、現 `worker` / `tui` / `yoi` の内部がまだ旧 store / registry を使っているため。

現時点の直接依存:

- `crates/worker/Cargo.toml`
  - `pod-store = { workspace = true }`
  - `pod-registry = { workspace = true }`
- `crates/tui/Cargo.toml`
  - `pod-store = { workspace = true }`
  - `pod-registry = { workspace = true }`
- `crates/yoi/Cargo.toml`
  - `pod-store = { workspace = true }`

主な残存理由:

1. `worker` crate の Spawn/Stop/peer discovery/delegated scope 経路が `pod-registry` を使っている。
   - `worker/src/spawn/tool.rs`: `delegate_scope`, `release_worker`, `default_registry_path`, `LockFileGuard`。
   - `worker/src/spawn/registry.rs`: delegated scope reclaim / release。
   - `worker/src/worker.rs`: top-level install / adopt allocation / segment update / reclaim。
   - `worker/src/discovery.rs`: segment lookup / peer discovery。
   これは旧 child Pod/Worker process orchestration の実行時 authority が残っているため。

2. `worker` crate の metadata/session persistence が `pod-store` を generic Worker store として使い続けている。
   - `worker/src/entrypoint.rs`: `FsWorkerStore`, `CombinedStore`。
   - `worker/src/runtime_adapter.rs`: `FsWorkerStore`, `CombinedStore`, `pod_store_dir`。
   - `worker/src/controller.rs`, `worker/src/worker.rs`, `worker/src/ticket_event_notify.rs`, tests。
   これは名前は Pod だが、`WorkerMetadataStore` / `WorkerMetadata` / `FsWorkerStore` が現 Worker 実行にも必要な metadata/session store として残っているため。削除前に Worker/Runtime store 境界へ move/rename が必要。

3. `tui` crate が旧 local Worker inventory / picker / dashboard で `pod-store` / `pod-registry` を読んでいる。
   - `tui/src/picker.rs`
   - `tui/src/worker_list.rs`
   - `tui/src/dashboard/mod.rs`
   current Runtime API / Backend Worker list へ置換するか、旧 TUI inventory を削除する必要がある。

4. `yoi` CLI の session / worker cleanup が `pod-store` を読んでいる。
   - `yoi/src/session_cli.rs`
   - `yoi/src/worker_cleanup_cli.rs`
   旧 metadata cleanup / referenced session discovery 用。Runtime/worker-runtime store へ置換するか、旧 cleanup CLI を削除する必要がある。

結論:

- `crates/pod` が残っているわけではない。既に消えている。
- `pod-registry` は旧 process/delegation/segment/socket 系 authority として `worker` と TUI に残っている。
- `pod-store` は旧名のまま generic Worker metadata/session store として `worker`、TUI、CLI に残っている。
- 削除作業は「pod crate の削除」ではなく、`pod-registry` の旧 orchestration authority 削除と、`pod-store` の必要部分を Worker/Runtime store に移管して crate を消す作業になる。


---

<!-- event: intake_summary author: hare at: 2026-06-28T14:24:09Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-28T14:24:09Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-28T16:51:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-28T16:51:22Z -->

## Decision

Routing decision:

Workspace Dashboard Queue authorized Orchestrator routing, so the Ticket and current workspace state were inspected.

Findings:
- Ticket state: `queued`
- Outgoing dependency: `depends_on 00001KW7726H9`
- Blocking Ticket `00001KW7726H9` is currently `inprogress`.
- Current inprogress Tickets: `00001KW7726H9`
- Orchestration worktree: clean

Decision:
- Leave `00001KW7835H0` queued and do not create a child worktree / role Pod yet.
- Reason: `pod-registry` / `pod-store` removal must follow the canonical Runtime Worker creation authority decision and implementation in `00001KW7726H9`. Starting both in parallel would risk deleting/moving store/registry paths while the canonical Worker creation API is still being shaped.

Next routing condition:
- Re-evaluate this Ticket after `00001KW7726H9` is merged, validated, and closed, or after an explicit replan changes the dependency.

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-28T19:17:30Z -->

## Decision

Routing decision:

The previously blocking dependency `00001KW7726H9` is now merged, validated, closed, and cleaned up. Related fs-store persistence Ticket `00001KW76E8EG` is also merged, validated, closed, and cleaned up. Current workspace state was inspected before implementation side effects.

Findings:
- Ticket state: `queued`
- Blocking dependency: `00001KW7726H9` is closed.
- Related persistence Ticket: `00001KW76E8EG` is closed.
- Current `inprogress` Tickets: 0
- Orchestration worktree: clean

Decision:
- Accept this Ticket for implementation and transition `queued -> inprogress` before child worktree / Pod side effects.

Plan:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KW7835H0-old-pod-crate-cleanup`
- Branch: `work/00001KW7835H0-old-pod-crate-cleanup`
- Coder Worker will remove old `pod-registry` / `pod-store` crates and migrate any remaining authority/persistence responsibilities to Worker/Runtime/session-store boundaries.
- Reviewer Worker will verify crate removal, dependency graph cleanup, Runtime authority invariants, stale compatibility avoidance, Browser/UI/CLI/prompt/test wording, and validation evidence.

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child worktree/branch unless explicitly reauthorized.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-28T19:17:36Z from: queued to: inprogress reason: dependencies_closed_and_queue_continues field: state -->

## State changed

Dependencies are closed, no Ticket is currently inprogress, and the orchestration worktree is clean. Accepting implementation before child worktree / role Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-28T19:18:26Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KW7835H0-old-pod-crate-cleanup`
- Branch: `work/00001KW7835H0-old-pod-crate-cleanup`
- Coder Worker spawned: `yoi-coder-00001KW7835H0-pod-cleanup`

Scope notes:
- Coder write scope covers the old crate directories `crates/pod-registry`, `crates/pod-store`, affected active crates (`crates/worker`, `crates/tui`, `crates/yoi`, `crates/worker-runtime`, `crates/session-store`, `crates/manifest`), prompt/profile/web workspace paths, and root Cargo/package files.
- `crates/pod` is already absent; Coder is instructed to verify absence and prevent reintroduction.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Implementation focus:
- remove `crates/pod-registry` and `crates/pod-store` from workspace
- migrate/rename any remaining Worker metadata/session persistence to Worker/Runtime/session-store boundaries without active Pod authority naming
- remove old pod-registry path APIs and old Pod metadata inventory/cleanup dependencies from worker/tui/yoi where active
- ensure Runtime Worker identity / creation / persistence remains `worker-runtime` fs-store + execution backend mapping authority
- do not use old Pod registry/store/socket/name as active authority
- keep historical docs only if clearly historical
- validate dependency graph and run targeted tests/checks

---

<!-- event: implementation_report author: yoi-coder-00001KW7835H0-pod-cleanup at: 2026-06-28T19:45:57Z -->

## Implementation report

Implementation report:

実装完了。Ticket ファイル編集、merge、close、cleanup は未実施。

Commit:
- `17a9488a4aa0a3dc83be2d3360b6ffd8ffcaeb5a refactor: remove old pod crates`

実装内容:

- `crates/pod-registry` / `crates/pod-store` を workspace から削除。
  - `Cargo.toml` / `Cargo.lock` / 各 crate dependency / `package.nix` から旧 crate 参照を除去。
  - `crates/pod` も引き続き存在しないことを確認。
  - `pod-registry` / `pod-store` / `crates/pod` 系の active 参照が残っていないことを `rg` で確認。
- `pod-store` の Worker metadata 永続化を `session-store` 境界へ移動。
  - `session_store::worker_metadata` として `FsWorkerStore`, `WorkerMetadataStore`, `CombinedStore` 等を提供。
  - active storage naming を Worker metadata に寄せ、metadata root も `data_dir/workers` 側へ変更。
- `pod-registry` の active crate 依存を削除。
  - Worker 内部の `runtime::worker_allocation` モジュールへ、scope allocation 用の実装として分類・移設。
  - module comment で「Runtime Worker identity / creation / durable persistence の authority ではない」ことを明記。
  - manifest path API は `worker_allocation_path` / `worker_socket_path` 名へ置換。
- `tui` / `yoi` / `worker` の旧 crate 依存を置換。
  - TUI の live worker inventory は旧 `pod-registry` crate に依存せず、renamed worker allocation table を read-only に読む形へ変更。
  - 読み取り時は `fs4` の shared lock を使い、writer との競合で partial read しにくい形に修正。
- `package.nix` の `cargoHash` を更新。

依存監査サマリ:
- `crates/pod`: absent
- `crates/pod-registry`: deleted
- `crates/pod-store`: deleted
- workspace/package/lock/source 内の active `pod-registry` / `pod-store` / `crates/pod` 参照: なし
- Runtime Worker identity/creation/persistence authority は `worker-runtime` fs-store + execution backend mapping のまま維持し、旧 registry/store/socket/name を authority として使わない構成に整理。

Validation:
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo test -p worker`: success
- `cargo test -p session-store`: success
- `cargo check -p yoi`: success
- `cd web/workspace && deno task check`: success
- `git diff --check`: success
- `git diff --cached --check`: success
- `nix build .#yoi`: success

Risks / notes:
- Worker metadata root を `data_dir/pods` 由来から `data_dir/workers` 側へ寄せたため、既存ローカル metadata の自動移行は入れていない。Ticket 方針どおり、旧 Pod authority naming 互換を増やさない実装。
- Historical docs は今回の active dependency cleanup の対象外だが、active crate/path/dependency 参照は除去済み。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-28T19:46:27Z -->

## Implementation report

Review kickoff:

Reviewer Worker spawned for `00001KW7835H0` implementation.

- Reviewer Worker: `yoi-reviewer-00001KW7835H0-pod-cleanup`
- Review target commit: `17a9488a4aa0a3dc83be2d3360b6ffd8ffcaeb5a refactor: remove old pod crates`
- Review mode: read-only implementation worktree review

Review focus:
- `crates/pod-registry` and `crates/pod-store` are removed from workspace / Cargo / package / lock / active source references
- `crates/pod` remains absent and is not reintroduced
- needed Worker metadata/session persistence moved to Worker/Runtime/session-store boundaries without active Pod authority naming
- old pod-registry APIs are not active authority
- Runtime Worker identity / creation / persistence remains `worker-runtime` fs-store + execution backend mapping
- UI/CLI/TUI active wording and tests compile
- no stale compatibility alias preserves old Pod store authority
- validation evidence is plausible and sufficient

---

<!-- event: review author: yoi-reviewer-00001KW7835H0-pod-cleanup at: 2026-06-28T19:51:02Z status: request_changes -->

## Review: request changes

request_changes

対象 commit `17a9488a4aa0a3dc83be2d3360b6ffd8ffcaeb5a` を read-only で確認した。worktree は clean、`git diff --check HEAD^ HEAD` は成功。重い `cargo` / `deno` / `nix` は reviewer では再実行せず、報告済み validation は diff/code 上で plausibility を確認した。

## 満たしている点

- `crates/pod` / `crates/pod-registry` / `crates/pod-store` のディレクトリは存在しない。
- `Cargo.toml` workspace members/default-members/dependencies、active crate `Cargo.toml`、`Cargo.lock`、`package.nix` から `pod-registry` / `pod-store` dependency は除去済み。
- `manifest::paths::pod_registry_path` / `pod_registry::...` / `pod_store::...` 参照は active source から消えている。
- `pod-store` 相当の `WorkerMetadataStore` / `FsWorkerStore` / `CombinedStore` は `session-store::worker_metadata` へ移され、`data_dir/workers` の Worker metadata と session log store に分離されている。
- `worker-runtime` 側は `RuntimeId + WorkerId` を authority とする fs-store / execution backend mapping が残っており、旧 pod path/socket/session path を authority にしない設計コメント・境界も確認した。

## Blockers

1. **active CLI output に旧 `pod` 名が残っている。**

   `crates/yoi/src/worker_cleanup_cli.rs:242` / `253` で `yoi worker delete` の refusal 出力が `pod: {}` になっている。これは Ticket の「TUI / yoi CLI / worker cleanup CLI から旧 Pod metadata inventory / cleanup 依存を削除」「Active UI / CLI / prompt / test output に旧 Pod concept が正規 concept として残らない」に反する。`worker:` に直す必要がある。

2. **LLM/tool-visible な active Worker allocation error が旧 Pod authority wording のまま。**

   `crates/worker/src/runtime/worker_allocation/error.rs:14,16,29,32` に以下が残っている:
   - `pod name ... is already registered`
   - `conflicts with pod ...`
   - `pod ... is not registered`
   - `session ... is already held by pod ...`

   これらは `crates/worker/src/spawn/tool.rs:867-875` で `ToolError` に `e.to_string()` として流れるため、単なる内部コメントではなく active tool/user-facing error。旧 `pod-registry` crate は消えているが、移管先の active scope allocation path が旧 Pod 名で authority/error を表現しており、Ticket intent に対して未完了。

3. **active e2e harness artifact に旧 pod registry / metadata path が残っている。**

   `tests/e2e/src/lib.rs:1624-1625` が `tested_yoi_pod_registry: ... pods.json` と `fixture_pod_metadata_root: ... pods` を書き出している。`tests/e2e` は workspace member なので、historical docs/report ではなく active test source/output と見なされる。現 Runtime/Worker path へ更新するか、legacy/historical fixture として明確に分類・隔離する必要がある。

上記を直したうえで、報告済み validation 一式（少なくとも `cargo check -p yoi`, `cargo test -p worker`, `cargo test -p worker-runtime --features ws-server`, `cargo test -p yoi-workspace-server`, `deno task check`, `git diff --check`, `nix build .#yoi --no-link`）を再確認すること。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-28T19:51:35Z -->

## Implementation report

Change-request routing:

Reviewer requested changes for active wording/test artifact leftovers. The original Coder scope did not include `tests/e2e`, so a focused fix Coder Worker was spawned with additional write scope for the specific blocker areas.

- Fix Coder Worker: `yoi-coder-00001KW7835H0-pod-fix`
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KW7835H0-old-pod-crate-cleanup`

Requested fixes:
- change active `yoi worker delete` refusal output from `pod:` to `worker:`
- rename active worker allocation tool/user-facing errors away from old Pod authority wording
- update active `tests/e2e` harness artifact names/paths away from old pod registry / pod metadata wording or clearly isolate as legacy if required
- run focused validation and commit fixes

---

<!-- event: implementation_report author: yoi-coder-00001KW7835H0-pod-fix at: 2026-06-28T19:55:08Z -->

## Implementation report

Change-response implementation report:

Reviewer blockers を修正し、追加 commit を作成済み。Ticket ファイル編集、merge、close、cleanup は未実施。

Commit:
- `c46e880b fix: finish worker wording cleanup`

変更内容:
- `crates/yoi/src/worker_cleanup_cli.rs`
  - `yoi worker delete` の refusal 出力を `pod:` から `worker:` に修正。
- `crates/worker/src/runtime/worker_allocation/error.rs`
  - active/user-facing error 文言を旧 Pod 表現から Worker/allocation 表現に修正。
  - `pod name`, `conflicts with pod`, `pod ... not registered`, `held by pod` を置換。
- `tests/e2e/src/lib.rs`
  - E2E artifact 名と fixture metadata path を Worker/allocation/metadata 表現に更新。
  - `tested_yoi_pod_registry` -> `tested_yoi_worker_allocation`
  - `fixture_pod_metadata_root` -> `fixture_worker_metadata_root`
  - fixture path `yoi/pods/...` -> `yoi/workers/...`
  - helper 名を `write_blocking_worker_metadata` に更新。

Validation:
- `cargo fmt`: success
- `cargo test -p worker`: success
- `cargo check -p yoi`: success
- `cargo check -p yoi-e2e`: success
- `git diff --check`: success
- 対象旧表現の `rg` 確認: success

Final state:
- child worktree is clean.

---

<!-- event: review author: yoi-reviewer-00001KW7835H0-pod-cleanup at: 2026-06-28T20:00:09Z status: request_changes -->

## Review: request changes

request_changes

再レビューは read-only で実施した。`c46e880b` は確認済みで、worktree は clean、`git diff --check HEAD^ HEAD` は成功。重い `cargo` / `deno` / `nix` は read-only 指示のため再実行せず、報告 validation はコード上の整合性を確認した。

前回指摘分のうち、以下は修正済み:

- `crates/yoi/src/worker_cleanup_cli.rs` の active refusal output は `pod:` ではなく `worker:` になっている。
- `crates/worker/src/runtime/worker_allocation/error.rs` の user-facing error は Pod wording から Worker/allocation wording に変更されている。
- `tests/e2e/src/lib.rs` の artifact key / helper 名は `tested_yoi_worker_allocation` / `fixture_worker_metadata_root` / `write_blocking_worker_metadata` に変更されている。
- `crates/pod` / `crates/pod-registry` / `crates/pod-store` の crate directory / workspace dependency / lock / package 参照は残っていない。

## Blocker

### E2E fixture の Worker metadata root が active store と一致していない

`tests/e2e/src/lib.rs` は現在も Worker metadata を `XDG_DATA_HOME` 側へ書いている。

- `tests/e2e/src/lib.rs:1353-1354`
  - `write_blocking_worker_metadata(&fixture.xdg_data_home, "workspace")?;`
  - `write_blocking_worker_metadata(&fixture.xdg_data_home, "workspace-orchestrator")?;`
- `tests/e2e/src/lib.rs:1625`
  - `"fixture_worker_metadata_root": self.xdg_data_home.join("yoi").join("workers")`
- `tests/e2e/src/lib.rs:2000-2003`
  - `data_home.join("yoi").join("workers").join(worker_name)`

しかし active Worker metadata store は `manifest::paths::data_dir()` ベース:

- `crates/manifest/src/paths.rs:41-46`
  - `data_dir()` は `YOI_DATA_DIR` / `YOI_HOME` / `HOME` を見る。`XDG_DATA_HOME` は使わない。
- `crates/worker/src/entrypoint.rs:487-494`
  - worker metadata dir は `paths::data_dir().join("workers")`
- `crates/yoi/src/worker_cleanup_cli.rs:209-237`
  - cleanup CLI も `paths::data_dir().join("workers")`

E2E harness は `HOME` と `XDG_DATA_HOME` を分けて渡しており、`YOI_DATA_DIR` は設定していない。そのため実際の active metadata root は fixture 上では概ね `$HOME/.yoi/workers` だが、現在の fixture/helper/artifact は `$XDG_DATA_HOME/yoi/workers` を指している。

これは表面上の naming は直っているが、移行後の Worker/session-store 境界と一致していないため、E2E fixture が正しい Worker metadata store をブロック・記録できない。Ticket の「必要な Worker metadata/session persistence を Worker/Runtime/session-store boundaries へ移す」「tests が旧 Pod metadata inventory に依存しない」に対して未完了。

修正案:

- `write_blocking_worker_metadata` に `fixture.home` または解決済み `data_dir` を渡し、`$HOME/.yoi/workers` に書く。
- または E2E subprocess に `YOI_DATA_DIR` を明示し、helper / artifact / subprocess env が同じ data root を使うように揃える。
- `fixture_worker_metadata_root` も同じ resolved active path を記録する。

補足: `YOI_POD_RUNTIME_COMMAND` の残存は既存設計上の dev executable-path override として扱い、今回の blocker にはしていない。

---

<!-- event: implementation_report author: yoi-coder-00001KW7835H0-pod-fix at: 2026-06-28T20:02:28Z -->

## Implementation report

Change-response implementation report:

Reviewer の E2E Worker metadata root blocker を修正し、追加 commit を作成済み。Ticket ファイル編集、merge、close、cleanup は未実施。

Commit:
- `fdd902d5 fix: align e2e worker metadata root`

変更内容:
- `tests/e2e/src/lib.rs`
  - E2E fixture の Worker metadata helper を active code の `manifest::paths::data_dir()` の既定解決に合わせた。
  - `XDG_DATA_HOME/yoi/workers` ではなく、fixture の `HOME` から解決される `$HOME/.yoi/workers` を使用。
  - `fixture_worker_metadata_root` artifact も同じ `$HOME/.yoi/workers` を記録するよう修正。
  - `active_worker_metadata_root(&home)` helper を追加し、書き込み先と artifact 記録を同一関数で揃えた。

Validation:
- `cargo fmt`: success
- `cargo check -p yoi-e2e`: success
- `git diff --check`: success
- `cargo test -p yoi-e2e`: success
- `cargo check -p yoi`: success

Final state:
- child worktree is clean.

---
