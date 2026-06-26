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
