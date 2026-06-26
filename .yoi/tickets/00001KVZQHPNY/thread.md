<!-- event: create author: "yoi ticket" at: 2026-06-25T15:49:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T16:34:16Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:34:16Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:44:39Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:45:08Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue による routing 許可を受けて Ticket / relations / orchestration plan / queue state を確認した。
- 本 Ticket は `00001KVZBCQH4` (`worker-runtime core crateと組み込みRuntime APIを作る`) に `depends_on` relation を持つ。
- `00001KVZBCQH4` は現在 `inprogress` で、implementation/package fix commits はあるが reviewer review 中。merge / Orchestrator validation / done ではない。
- Config bundle sync は worker-runtime core の `CreateWorkerRequest` / Profile selector / `ConfigBundleRef` boundary に依存するため、core API 確定前に implementation side effect を開始しない。

Evidence checked:
- Ticket body: config bundle model、Runtime sync API、Worker creation integration、Backend responsibility、Plugin/host policy boundary、Non-goals。
- Relations: outgoing `depends_on -> 00001KVZBCQH4`; incoming related `00001KVZSGT14`。
- Orchestration plan: blocker record `orch-plan-20260625-164457-1` を追加。
- Queue state: queued は本 Ticket を含む6件。inprogress は `00001KVZBCQH4` 1件。
- Workspace state: orchestration worktree is clean at queue commit; core implementation is under reviewer Worker.

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZBCQH4` が reviewer approve / merge / validation / done になった後、再 routing する。

Escalate if:
- core の `CreateWorkerRequest` / `ConfigBundleRef` placeholder が bundle sync requirements を満たさない。
- bundle sync のために REST server / FS store / Plugin manager 実装を同時に要求する形になりそうな場合。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-26T06:32:31Z -->

## Decision

Routing decision: implementation_ready

Reason:
- `00001KVZBCQH4` worker-runtime core、Backend RuntimeRegistry foundation、embedded/remote Runtime connection、REST/WS foundation は done。
- 前回の waiting-capacity reason は解消済みで、現在 `inprogress` は 0 件。
- Ticket body は config bundle model、sync API、worker creation integration、Backend responsibility、Plugin/host policy boundary、Non-goals、acceptance criteria を明記している。
- Plugin package bytes vs package ref の詳細などは実装時の local design latitude として扱える。Secret value sync / full Plugin manager / actual host mount path sync は Non-goals。

Evidence checked:
- Ticket body: digest/revision/provenance、bundle content model、secret/raw path exclusion、Runtime sync API、CreateWorkerRequest integration、typed errors、Backend responsibility、Non-goals、validation。
- Relations: only blocking dependency `00001KVZBCQH4` is done。Remote Runtime integration has related relation only and is now done.
- Orchestration plan: accepted plan `orch-plan-20260626-063205-5` を記録。
- Workspace state: orchestration worktree clean、no active inprogress。

IntentPacket:

Intent:
- `worker-runtime` と Backend に Profile/config bundle sync boundary を追加し、Runtime が bundle digest / profile selector を検証して Worker creation に使えるようにする。

Binding decisions / invariants:
- Bundle に secret values、raw socket/session path、runtime-local mount actual path、host-local cache path を含めない。
- Secret/mount/network/shell/git availability は host-local policy / grant / secret ref として表現し、Runtime host が最終判断する。
- Backend は Runtime credential / direct endpoint / raw bundle storage path を Browser に渡さない。
- Backend が巨大な fully-resolved WorkerSpec を毎回送る設計にはしない。Worker creation は intent + profile selector + bundle ref + capabilities の境界を保つ。
- Builtin/default fallback は残すが、synced bundle mode と責務を区別する。
- Full Plugin package manager / registry / signature policy / secret value sync / Web Console completion は実装しない。

Requirements / acceptance criteria:
- Config bundle domain type が digest / revision / workspace id / created_at / provenance を持つ。
- Runtime は bundle を保存・一覧/確認し、digest を検証できる。
- `CreateWorkerRequest` / worker creation path が profile selector + config bundle ref を扱う。
- Missing bundle / digest mismatch / invalid profile / unsupported declaration は typed error。
- Embedded Runtime は direct lib API で sync 可能。
- Networked Runtime 用 REST sync API shape または endpoint がある。
- Backend は Runtime へ bundle sync し、bundle availability を確認できる。
- Tests distinguish builtin/default fallback vs synced bundle mode。

Implementation latitude:
- Module split, exact JSON/domain structs, digest computation details, minimal profile resolution depth, package ref vs inline descriptor representation are Coder choices within the invariants.
- For v0, `ResolvedWorkerSpec` / host-local policy enforcement may be a typed boundary with focused validation rather than full Plugin execution.
- If Profile/resource loading from existing project crates is too broad for this slice, define stable bundle structs and sync/validation semantics first, then escalate for deeper integration.

Escalate if:
- Secret values or host-local actual paths appear necessary in bundle content.
- Full Plugin package manager/signature policy is required to satisfy acceptance.
- Existing `worker-runtime` / Backend API must be redesigned broadly.
- Browser-facing API would need to receive Runtime credentials or raw bundle storage paths.

Validation:
- `cargo fmt --all`
- `cargo test -p worker-runtime`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Secret/raw path leakage。
- Bundle digest/provenance instability or mismatch bugs。
- Blurring builtin/default fallback with synced bundle mode。
- Over-implementing Plugin manager / policy enforcement beyond v0 boundary。
- Remote Runtime sync endpoint leaking endpoint/token/path details to Browser。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T06:32:50Z from: queued to: inprogress reason: routing_accepted_after_runtime_foundation_done field: state -->

## State changed

Routing decision: implementation_ready。

worker-runtime core / Registry foundation / embedded+remote Runtime connection / REST+WS foundation は done。Ticket body / relations / workspace state / accepted plan を確認し、Profile/config bundle sync slice は unblocked と判断した。accepted plan と IntentPacket は thread / orchestration plan に記録済み。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T06:35:25Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KVZQHPNY-runtime-config-bundles`
- Branch: `work/00001KVZQHPNY-runtime-config-bundles`
- Coder Worker spawned: `yoi-coder-00001KVZQHPNY-config-bundles`

Scope notes:
- Coder has read access to the implementation worktree and non-recursive workspace root identity path needed by runtime startup.
- Coder write scope is limited to `crates/worker-runtime/**`, `crates/workspace-server/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` if dependency/package changes are needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Queued capacity notes:
- `00001KVZ9JGK0` remains queued to avoid overlapping Web Console/API surface changes while Profile/config sync is inprogress.
- `00001KW04A8K6` remains queued to avoid TUI migration against unstable worker creation/profile semantics.

---

<!-- event: implementation_report author: yoi-coder-00001KVZQHPNY-config-bundles at: 2026-06-26T07:08:31Z -->

## Implementation report

Implementation report:

実装完了。指定 worktree / branch 内で commit 済み。

Commit:
- `abab1af2 feat: add runtime config bundle sync`

変更ファイル:
- `Cargo.lock`
- `package.nix`
- `crates/worker-runtime/Cargo.toml`
- `crates/worker-runtime/src/lib.rs`
- `crates/worker-runtime/src/catalog.rs`
- `crates/worker-runtime/src/config_bundle.rs`
- `crates/worker-runtime/src/error.rs`
- `crates/worker-runtime/src/fs_store.rs`
- `crates/worker-runtime/src/http_server.rs`
- `crates/worker-runtime/src/runtime.rs`
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`

実装概要:

### worker-runtime

- `config_bundle` module を追加。
  - `ConfigBundle`
  - `ConfigBundleMetadata`
  - `ConfigBundleProvenance`
  - `ConfigProfileDescriptor`
  - `ConfigDeclaration`
  - `ConfigBundleSummary`
  - `ConfigBundleAvailability`
- Bundle metadata に digest / revision / workspace id / created_at / provenance/source metadata を保持。
- `ConfigBundleRef` を `id + digest` に拡張。
- Runtime direct lib API を追加。
  - `store_config_bundle`
  - `list_config_bundles`
  - `check_config_bundle`
- Worker creation で profile selector + config_bundle ref を検証。
  - bundle missing
  - digest mismatch
  - invalid profile selector
  - unsupported declaration
  を typed `RuntimeError` として返す。
- builtin/default fallback と synced bundle mode を区別。
  - bundle なしでは `RuntimeDefault` / `Builtin` fallback を許可。
  - `Named` profile は synced bundle ref 必須。
- FS snapshot に config bundle store を永続化。
- Runtime REST API を追加。
  - `GET /v1/config-bundles`
  - `POST /v1/config-bundles`
  - `GET /v1/config-bundles/{bundle_id}/availability?digest=...`
- Bundle validation で secret values / host-local path 的な値を bundle content に持ち込まない前提の ref/grant/policy boundary を enforced する最小チェックを追加。

### workspace-server / Backend

- `WorkerSpawnRequest` に `profile` / `config_bundle` / `requested_capabilities` を追加。
- `RuntimeRegistry` / runtime host boundary に bundle sync/check/list を追加。
- Embedded Runtime backend で direct lib sync/check/create が動作。
- Remote Runtime backend で Runtime REST sync/check endpoint を利用。
- Workspace API に backend-facing endpoint を追加。
  - `POST /api/runtimes/{runtime_id}/config-bundles`
  - `GET /api/runtimes/{runtime_id}/config-bundles/{bundle_id}/availability?digest=...`
- Browser/API 側に Runtime credentials / direct endpoint / raw bundle storage path は出していない。

Validation:
- `cargo fmt --all`: success
- `cargo test -p worker-runtime`: success（10 passed）
- `cargo test -p yoi-workspace-server`: success（34 passed）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Notes:
- `sha2` を `worker-runtime` direct dependency に追加したため、`Cargo.lock` と `package.nix` の `cargoHash` も更新。

Residual risks / review focus:
- Bundle digest canonicalization は v0 の typed domain fields ベース。将来 declaration schema が増える場合、digest 対象フィールドの拡張方針を確認してほしい。
- Bundle content の secret value / host-local path 禁止は v0 として文字列フィールドへの conservative validation。完全な policy/signature/package-manager ではない。
- Remote Runtime の bundle availability check は query string に digest を載せる最小 REST shape。bundle id の URL encoding 方針は今後必要なら強化対象。
- Backend API endpoint は sync/check のみ追加し、Browser 完成 UX や full Plugin package manager / signature policy は実装していない。

---

<!-- event: review author: yoi-reviewer-00001KVZQHPNY-config-bundles at: 2026-06-26T07:18:50Z status: request_changes -->

## Review: request changes

Review result: request_changes

読取専用レビュー結果。`abab1af2` の差分・該当ソースを確認し、追加で `git diff --check HEAD^ HEAD` のみ実行した（cargo/nix はファイル生成を避けるため再実行せず、報告値として扱った）。

ブロッカー:

1. **bundle content の禁止境界がまだ満たせていない。**
   `ConfigDeclaration.reference` は自由文字列で、検証は `validate_boundary_text` の限定的な substring 判定のみ。
   - `crates/worker-runtime/src/config_bundle.rs:124-129`
   - `crates/worker-runtime/src/config_bundle.rs:266-294`
   例えば `.cache/yoi`、`.yoi/sessions/foo.jsonl`、`pods/foo/sock` のような相対 cache/session/socket 形や、`SecretRef.reference` に平文 secret 風文字列を入れても拒否されない。Ticket の invariant「Bundles must not contain secret values / raw socket/session paths / host-local cache paths」を API 型・検証境界として保証できていないため修正が必要。型を分けるか、少なくとも ref/grant/policy の許可形式を明示して拒否テストを追加すること。

2. **Backend remote availability check の bundle id canonicalization/encoding が不十分。**
   Runtime REST は `{bundle_id}` path segment を使うが、bundle id 側の検証は `/`, `?`, `&` などを拒否せず、remote client は URL encode せずに埋め込んでいる。
   - REST route: `crates/worker-runtime/src/http_server.rs:172-177`
   - remote client: `crates/workspace-server/src/hosts.rs:1784-1788`
   direct/POST store では受け入れられる id が remote check では経路破壊・誤解釈され得るため、Remote/Embedded の sync/check semantics が一致しない。path-safe id へ正規化/検証するか、ID を安全にエンコード/別ボディ化し、対応テストを追加すること。

3. **Browser-facing diagnostics に remote Runtime の raw store path が漏れる経路がある。**
   Runtime REST error は `RuntimeError::to_string()` をそのまま返し、`StoreIo/StoreMissing/StoreCorrupt` は path を Display する。Backend remote client はその remote message を診断として Browser-facing `ConfigBundleSyncResult/CheckResult` に載せる。
   - path を含む Display: `crates/worker-runtime/src/error.rs:65-80`
   - REST error message: `crates/worker-runtime/src/http_server.rs:785-789`
   - Backend diagnostic relay: `crates/workspace-server/src/hosts.rs:2476-2505`
   「Backend must not expose ... raw bundle storage paths to Browser」に反する。remote Runtime の internal path は Backend 側でサニタイズし、embedded と同様に内部 path 非公開の診断へ畳む必要がある。

確認できた良い点:
- digest/revision/workspace_id/created_at/provenance を持つ bundle domain type は追加されている。
- Runtime store/list/check、profile + bundle ref の Worker 作成検証、missing/mismatch/invalid profile/unsupported declaration の typed error は概ね入っている。
- builtin/default fallback と synced bundle mode のテスト分離はある。
- Embedded direct sync と remote REST sync/check の骨格はある。
- package.nix の cargoHash は Cargo.lock 変更に合わせて更新されている。

---

<!-- event: implementation_report author: yoi-coder-00001KVZQHPNY-config-bundles at: 2026-06-26T07:34:04Z -->

## Implementation report

Follow-up implementation report after review request_changes:

Commit:
- `4867ab21 fix: harden runtime config bundle boundary`

対応内容:

### 1. Bundle content boundary 強化

- `ConfigBundle` / `ConfigBundleRef` validation を強化。
- Bundle id を path-safe stable identifier に制限。
  - `/`, `?`, `&`, `#`, whitespace などを拒否。
- digest を lowercase SHA-256 hex 64 chars として検証。
- `ConfigDeclaration.reference` は free string のままにせず、kind ごとに許可 prefix を限定。
  - `SecretRef`: `secret:`, `secret-ref:`, `vault:`, `keyring:`
  - `MountGrant`: `mount:`, `mount-grant:`
  - `NetworkPolicy`: `network:`, `network-policy:`
  - `ShellPolicy`: `shell:`, `shell-policy:`
  - `GitPolicy`: `git:`, `git-policy:`
  - `CapabilityGrant`: `capability:`, `capability-grant:`
- reference token では `/`, `\`, `?`, `&`, `#`, `%`, `=`, whitespace を拒否。
- relative cache/session/socket/path-like forms を拒否するテストを追加。
  - `.cache/yoi`
  - `.yoi/sessions/foo.jsonl`
  - `pods/foo/sock`
  - `password=hunter2`
  - `hunter2-secret-value`

### 2. Remote availability check の path safety

- Runtime 側で unsafe bundle id を拒否。
- Remote client 側の bundle availability path construction で bundle id / digest を percent-encode。
  - `bundle/1?x` が `/v1/config-bundles/bundle%2F1%3Fx/availability?...` になることを mock HTTP test で確認。
- Direct/Embedded と Remote で path segment 解釈が分岐しにくいようにした。

### 3. Browser-facing diagnostics の raw path leak 防止

- Remote Runtime HTTP error diagnostic で remote error message をそのまま転送しないように変更。
- Browser-facing diagnostic は remote code / HTTP status のみを含む sanitized message に変換。
- mock remote response に raw store path / session path を含め、`ConfigBundleSyncResult` / `ConfigBundleCheckResult` に漏れないことをテスト追加。

Validation:
- `cargo fmt --all`: success
- `cargo test -p worker-runtime`: success（13 passed）
- `cargo test -p yoi-workspace-server`: success（35 passed）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Residual risks / review focus:
- `ConfigDeclaration.reference` は v0 として prefix/token allowlist で制限。将来、より構造化された enum payload に移す余地はある。
- Secret ref は「値を同期しない」境界として typed prefix 必須にしたが、ref 名自体に何を許すかは今後 policy 化できる。
- Remote error message は全体的に sanitized されるため、詳細調査は backend/runtime logs 側を見る前提。

---

<!-- event: review author: yoi-reviewer-00001KVZQHPNY-config-bundles at: 2026-06-26T07:38:26Z status: approve -->

## Review: approve

Review result: approve

再レビュー結果: 前回ブロッカーは解消されていると判断する。

確認内容:
- `4867ab21 fix: harden runtime config bundle boundary` の差分と関連実装を読取専用で確認。
- `git diff --check HEAD^ HEAD` は問題なし。
- cargo/nix 系はファイル生成を避けるため再実行せず、報告された成功結果を前提確認扱い。

確認できた解消点:
- bundle id は path-safe に制限され、`/`, `?`, `&`, `#`, whitespace 等が拒否される。
- digest は lowercase SHA-256 hex 64 chars として検証される。
- `ConfigDeclaration.reference` は kind 別 prefix + token 制限になり、cache/session/socket/path 風・secret value 風の拒否テストも追加されている。
- remote availability path は percent-encode され、Runtime 側でも unsafe id が拒否されるため remote/embedded の意味差が縮んでいる。
- remote Runtime error の raw message は Backend 診断へ中継されず、Browser-facing result は HTTP status / remote code ベースにサニタイズされている。raw store/session path 非漏洩テストも確認。
- builtin/default fallback と synced bundle mode の区別、typed error、sync/check API 境界は維持されている。
- Plugin package manager/signature/WebConsole 方面の scope creep は見当たらない。
- `package.nix` の `cargoHash` 更新も反映済み。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T07:39:30Z -->

## Implementation report

Integration / validation report:

Integration:
- Implementation branch `work/00001KVZQHPNY-runtime-config-bundles` を orchestration branch に merge 済み。
- Merge commit: `7e8a8cfa merge: 00001KVZQHPNY runtime config bundles`

Included implementation commits:
- `abab1af2 feat: add runtime config bundle sync`
- `4867ab21 fix: harden runtime config bundle boundary`

Validation in Orchestrator worktree:
- `cargo fmt --all --check`: success
- `cargo test -p worker-runtime`: success（13 tests passed）
- `cargo test -p yoi-workspace-server`: success（35 tests passed）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Review:
- Reviewer approve 済み。前回 blockers だった bundle content boundary、bundle id path safety、remote diagnostics raw path non-leak は解消済み。

Outcome:
- Acceptance criteria を満たしたため `done` へ進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T07:39:39Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Runtime Profile/config bundle sync の実装、review、merge、Orchestrator validation が完了した。

Done evidence:
- Merge commit: `7e8a8cfa merge: 00001KVZQHPNY runtime config bundles`
- Reviewer approve 済み。
- Orchestrator validation:
  - `cargo fmt --all --check`: success
  - `cargo test -p worker-runtime`: success（13 tests passed）
  - `cargo test -p yoi-workspace-server`: success（35 tests passed）
  - `cargo check -p yoi`: success
  - `git diff --check`: success
  - `nix build .#yoi --no-link`: success

Scope:
- Config bundle domain/sync/check、worker creation integration、embedded/remote Backend sync/check boundary を追加。
- Secret value sync / full Plugin package manager/signature policy / Web Console completion は Non-goals として未実装。

---
