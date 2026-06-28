<!-- event: create author: "yoi ticket" at: 2026-06-28T13:35:21Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-28T14:42:12Z -->

## Decision

Runtime Worker creation API から Backend/Orchestrator intent と raw workspace context を外す。

決定:
- `intent` は Runtime の authority ではなく Backend-side launch context とする。Workspace Companion / Ticket role / Orchestrator routing / Objective は Backend が Profile 選択、ConfigBundle 選択、ExecutionEnvironment 準備、initial context/tool authority 付与に使う。
- ConfigBundle 本体は create payload で送らない。Runtime は事前同期済み ConfigBundle store を ref/digest で参照し、create 時に availability / digest / profile compatibility / provider / secret / authority を検証する。
- working directory / workspace root / tool scope は Runtime create request の raw field にしない。Backend が worktree/cwd/tool authority/project record access/ticket backend access を準備し、Runtime には opaque `ExecutionEnvironmentRef` を渡す。
- Ticket / Objective は Runtime Worker identity metadata ではなく、initial context と tool-readable records として Worker に与える。Worker が知るべきなのは対象 Ticket/Objective の内容と使える tools/scope であり、Backend claim id や RuntimeRegistry routing ではない。

Runtime create request の中心は Profile selector、ConfigBundle ref、ExecutionEnvironmentRef、labels/role、execution/acceptance requirement に絞る。


---

<!-- event: decision author: hare at: 2026-06-28T15:07:01Z -->

## Decision

前回の記述は「API に入れないもの」の列挙に寄りすぎていたため、Ticket 本体を API 設計中心に書き直した。

修正内容:
- 横文字中心の見出しと説明を減らし、起動手順と作る API の形を前面に出した。
- ConfigBundle 同期、ExecutionEnvironment 準備、InitialInput 準備、Worker 作成を別 API として整理した。
- `CreateWorkerRequest` の具体的なフィールド案を明記した。
- `WorkerWorkspaceRef` は Worker 作成 API に入れず、working directory は `ExecutionEnvironmentRef` として execution backend だけが解決する内部参照にした。
- Ticket / Objective は Worker identity ではなく、initial input と tool-readable records で渡す形にした。後続 decision で initial input は `CreateWorkerRequest` に inline する方針へ更新した。


---

<!-- event: decision author: hare at: 2026-06-28T15:11:43Z -->

## Decision

前回の修正で domain term まで不自然に和訳していたため、Ticket 本体の用語を修正した。

修正方針:
- ConfigBundle / ExecutionEnvironment / InitialInput / execution backend / tool authority / provider / secret / Browser-facing API などの domain term はそのまま使う。
- 説明文は日本語にするが、API 名・crate/domain 用語は訳語を作らない。
- 独自訳は作らず、`tool authority` に統一する。


---

<!-- event: decision author: hare at: 2026-06-28T15:36:39Z -->

## Decision

Worker 起動 protocol の整理を更新した。

決定:
- Browser-facing launch は Workspace Backend への 1 request とする。Browser / Web Console は ConfigBundleRef、ExecutionEnvironmentRef、cwd、tool authority、secret、Runtime endpoint を知らない。
- Runtime の Worker creation は `CreateWorker` 1 request / 1 transaction とする。WorkerId を発行するのは `CreateWorker` だけ。
- ConfigBundle sync は create 前の冪等な前提準備であり、Worker を作らない。
- ExecutionEnvironment 準備は Backend / Runtime 内部の前提準備であり、Worker に見せる protocol ではない。未使用参照には lease / TTL / cleanup を持たせる。
- initial input は `CreateWorkerRequest` に inline する。`PrepareInitialInput` / `InitialInputRef` は作らない。
- `launch_id` を `CreateWorkerRequest` に入れ、retry / duplicate request を安定して扱えるようにする。


---

<!-- event: decision author: hare at: 2026-06-28T16:03:14Z -->

## Decision

embedded Runtime の authority 境界を踏まえて、ExecutionEnvironmentRef 方針を撤回し、ExecutionBindingRef 方針に更新した。

決定:
- Runtime は file operation authority / tool authority を持たない。embedded Runtime でも例外にしない。
- working directory / cwd / tool scope / Ticket backend authority は Runtime create request に入れない。
- Backend は execution host / tool host 側に execution binding を作る。これは Runtime API ではなく、Browser-facing API にも Worker の conversation context にも出さない。
- `CreateWorkerRequest` は `ExecutionEnvironmentRef` ではなく opaque な `ExecutionBindingRef` を受け取る。
- `ExecutionBindingRef` は Runtime に file access を許可する ref ではなく、Runtime が execution backend に接続するときに渡す handle とする。
- binding の cwd / tool authority / Ticket backend access の enforcement は execution backend / tool host 側で行う。
- remote Runtime が binding を使えない場合は、binding の中身を Runtime create request に展開せず、typed error で拒否する。


---

<!-- event: decision author: hare at: 2026-06-28T16:05:07Z -->

## Decision

InitialInput の表現を protocol Segment に寄せる。

決定:
- initial input 用の独自 message 型は作らず、`CreateWorkerRequest.initial_input` は `Vec<Segment>` とする。
- `System` message は作らない。role prompt / system-level instruction は Profile / ConfigBundle 側で解決する。
- readable record 用の別 field は作らない。Ticket / Objective などの読み直し可能な参照は Segment variant として表す。
- Ticket / Objective の要約文は `Segment::Text`、参照は `Segment::TicketRef` / `Segment::ObjectiveRef` 相当で表し、通常の user submission と同じ解決経路で扱う。


---

<!-- event: decision author: hare at: 2026-06-28T16:16:17Z -->

## Decision

未検証の pseudo API field を Ticket 本体から外し、API 確定を実装時の検証事項として委譲する方針に更新した。

決定:
- この Ticket 本文では `CreateWorkerRequest` の最終 field を固定しない。
- API field を定義する場合は、各 field ごとに source / authority / visibility / persistence / retry / validation / existing type / failure を検証する。
- `ConfigBundleRef` のような参照を create request に渡す案は、caller が Runtime 内部 config store を把握する必要を生む可能性があるため未採用とする。
- ConfigBundle の識別方法は、content identity / digest / 既存 store contract を確認して実装時に確定する。
- initial input は既存 Segment / user submission 表現に寄せる方針だけを残し、独自 pseudo type は置かない。
- ExecutionBinding も最終型名ではなく、Runtime が file authority を持たず execution backend へ渡す opaque handle が必要、という境界だけを残す。


---

<!-- event: decision author: hare at: 2026-06-28T16:26:48Z -->

## Decision

`authority` を未実装の API 概念として扱っていた箇所を修正した。

決定:
- 現段階で専用システム化されていない authority / capability を Runtime Worker creation API の field として新設しない。
- Ticket 本体では `authority` 検証ではなく、既存の source / scope-access / visibility / persistence / retry / validation / existing type / failure を検証する方針にする。
- Runtime が持たないものは `file operation authority` ではなく、より具体的に workspace filesystem access / tool execution scope と表現する。
- file/tool access の制限は、既存の execution backend / tool host / scope 設定の境界を確認して実装時に接続する。


---

<!-- event: decision author: hare at: 2026-06-28T16:30:04Z -->

## Decision

Worker creation field の検証観点にある persistence の意味を修正した。

決定:
- 通常 Worker creation で「永続化しない」は選択肢にしない。
- field 検証の persistence は yes/no ではなく、既存の正規 store の record / projection に何を保存し、restart 後に何を復元・診断するかを確認する `persistence/projection` とする。
- raw handle などを保存できない場合でも、対応する durable identity / projection / diagnostic record を持つ。
- Runtime catalog / transcript / observation / persistence initialization が Worker creation と一貫していることを受け入れ条件に残し、永続化しない通常 Worker 作成経路を禁止する。


---

<!-- event: decision author: hare at: 2026-06-28T16:33:05Z -->

## Decision

`persistence/projection` の意味を再修正した。

決定:
- store は任意選択の対象ではない。
- Worker creation の保存先は既存の正規 store / projection に従う。主対象は worker-runtime fs-store、transcript store、Runtime catalog など既存 contract で決まるもの。
- field 検証では store を選ぶのではなく、既存の正規 store のどの record / projection に何を保存し、restart 後に何を復元・診断するかを確認する。


---

<!-- event: decision author: hare at: 2026-06-28T16:38:02Z -->

## Decision

現在実装に存在する Worker creation / launch 関連 field を調査し、`artifacts/api-field-audit.md` に記録した。

検証した主な対象:
- Runtime `CreateWorkerRequest` の `intent` / `profile` / `config_bundle` / `requested_capabilities` / `workspace_refs` / `mount_refs`。
- Browser-facing `WorkerSpawnRequest` の `intent` / `requested_worker_name` / `acceptance` / `profile` / `config_bundle` / `requested_capabilities`。
- ConfigBundle sync/store/digest の現行 contract。
- protocol `Segment` と Runtime `WorkerInput` / transcript の現行差分。
- execution backend spawn request と per-worker cwd/scope boundary の不在。
- worker-runtime fs-store / worker snapshot / transcript persistence と現在の create/spawn 順序。

主な結論:
- 未検証 field を API 案として固定しない。
- 現行 Runtime `CreateWorkerRequest` の `intent` / `requested_capabilities` / `workspace_refs` / `mount_refs` は canonical Runtime create request には残さない。
- Browser-facing `config_bundle` / `requested_capabilities` は Runtime create にそのまま流さない。
- `ExecutionBindingRef` は現行実装に存在しないため、最終 field として固定しない。
- initial input を `Vec<Segment>` に寄せる方針は妥当だが、現行 Runtime input/transcript は String なので field だけ先に固定しない。


---

<!-- event: intake_summary author: hare at: 2026-06-28T16:41:17Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-28T16:41:17Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-28T16:47:42Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-28T16:48:19Z -->

## Decision

Routing decision:

Workspace Dashboard Queue authorized Orchestrator routing. Current state was inspected before implementation side effects.

Findings:
- Ticket state: `queued`
- Blocker relations: none
- Related Tickets `00001KW55B32Y`, `00001KW55B33B`, `00001KW55B33H` are prior execution-boundary / adapter / Companion work and are closed.
- Current `inprogress` Tickets: 0
- Orchestration worktree: clean

Decision:
- Accept this Ticket for implementation and transition `queued -> inprogress` before child worktree / Pod side effects.

Plan:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KW7726H9-runtime-worker-launch-unification`
- Branch: `work/00001KW7726H9-runtime-worker-launch-unification`
- Coder Worker will inspect `.yoi/tickets/00001KW7726H9/item.md` and `artifacts/api-field-audit.md`, then implement the canonical Worker launch/creation path using current code as authority.
- Reviewer Worker will verify API boundary separation, ConfigBundle sync / Runtime creation separation, execution binding boundary, transaction/persistence/retry behavior, Browser-facing non-leak, no fake/providerless response, and focused tests.

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child worktree/branch unless explicitly reauthorized.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-28T16:48:25Z from: queued to: inprogress reason: authorized_unblocked_queue_acceptance field: state -->

## State changed

Workspace Dashboard Queue authorized routing. The Ticket has no blocker relations, related prerequisite Tickets are closed, no other Ticket is inprogress, and the orchestration worktree is clean. Accepting implementation before child worktree / role Pod side effects.

---
