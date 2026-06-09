---
description: Ticket を読み、Orchestrator が planning return / spike / implementation / review / blocked / close へ明示的に routing する workflow
model_invokation: true
user_invocable: true
requires: []
---
# Ticket Orchestrator Routing Workflow

Yoi の multi-agent 運用で、Intake や人間が作成した Ticket を Orchestrator が読み、次に取るべき action を明示的に分類・記録するための Workflow。

これは scheduler ではない。目的は、Ticket の fields / body / thread / artifacts / 現在の repository/Pod 状態を明示的に確認し、隠れた会話状態ではなく Ticket に基づいて routing 判断を残すことである。

Panel Queue / queued notification は、人間が Orchestrator に routing を開始してよいと許可した signal であり、unattended scheduler ではない。implementation side effect に進む場合は、Orchestrator が Ticket と workspace state を再確認し、unblocked と判断してから `queued -> inprogress` を記録する必要がある。

`ready` は Orchestrator routing に十分な状態であり、実装戦術が事前にすべて固定されている状態ではない。Orchestrator は、recorded intent / binding decisions / invariants / implementation latitude / acceptance criteria / escalation conditions が揃っていれば、bounded implementation uncertainty を残したまま implementation-ready と判断してよい。

`ready` / `queued` を `planning` に戻す判断は、Ticket text や risk flags だけで決めない。Orchestrator は、Ticket thread/artifacts、関連 Ticket / orchestration plan、関連 workflow/docs/code、durable project context、現在の repository/Pod/worktree 状態のうち relevant なものを bounded に確認し、実装前に必要な concrete missing decision / information が残っている場合だけ planning return とする。risk flags と risky domain は context lookup と reviewer focus の signal であり、automatic stop gate ではない。

## 位置づけ

```text
TicketCreate / TicketComment
  -> Ticket Orchestrator Routing Workflow
  -> planning return / requirements sync / spike / implementation / review / blocked / close
  -> 必要に応じて他 Workflow へ接続
```

- Intake は Ticket の materialization と planning/clarification を担当する role であり、state 名ではない。
- state は `planning -> ready -> queued -> inprogress -> done` を基本遷移とする。
- `ticket-preflight-workflow` は legacy canonical id 互換の planning/requirements sync entry であり、`preflight` を独立 state / lane / long-lived operation として扱わない。
- `ready -> queued` は人間が Orchestrator routing を許可した状態であり、worktree 作成や Pod 起動の許可そのものではない。
- `multi-agent-workflow` は coder / reviewer Pod と worktree を使う実装・レビュー loop。
- この Workflow は自動 scheduler / lease / unattended maintainer ではない。

## Orchestrator の責務

Orchestrator は以下を行う。

- Ticket を `TicketShow` で読む。
- 必要に応じて関連 Ticket を `TicketList` / `TicketShow` で確認する。
- Ticket body / thread / artifacts / resolution / review / implementation report を読む。
- Ticket が Objective context と結びついている場合は、Objective を medium-term goal / motivation / strategy / success criteria / decision context として読む。ただし Objective context は判断背景であり、Ticket body/thread/artifacts や explicit Ticket relations / OrchestrationPlan records を読む代替ではない。
- repository 状態、関連 docs/code、既存 worktree、visible Pods を必要に応じて明示的に確認する。
- queued notification を受けた場合も、Ticket と workspace state を再確認してから routing する。
- next action を routing classification として決める。
- routing decision を `TicketComment` で Ticket thread に記録する。
- broad request や split/refinement では、long-lived umbrella/progress-container Ticket ではなく concrete implementable Ticket、Objective context、split decision record を使う。
- Objective-to-Ticket links は canonical opaque Ticket ID による non-blocking context link として扱い、dependency / blocking / ordering / ownership / scheduling relation と解釈しない。
- 既存 umbrella/progress-container Ticket が concrete follow-up Ticket / Objective context で置き換え済みなら、superseded/decomposed として退役・close する routing を検討する。
- implementation-ready の場合は `multi-agent-workflow` に渡す `IntentPacket` を作る。
- implementation-ready かつ Ticket が `queued` の場合は、worktree 作成 / implementation Pod `SpawnPod` / coder routing などの side effect の前に、既存の typed Ticket backend/tool path で `queued -> inprogress` を記録する。
- `ready` または `queued` に concrete missing decision / information がある場合だけ、typed state-change/routing event 付きで `planning` に戻す。その event/comment には missing item、checked context、implementation latitude では足りない理由、次の planning question/action を含める。

## Orchestrator がしないこと

- 自動 scheduler として unattended に実行しない。
- Panel Queue / queued notification だけを unattended scheduler trigger として扱わない。
- `queued -> inprogress` acceptance なしに worktree 作成、implementation Pod `SpawnPod`、coder/reviewer routing を行わない。
- 人間/上位 Orchestrator の許可または明示的な routing acceptance なしに coder / reviewer Pod や read-only investigation helper Pod を起動しない。
- 設計境界の未決定を勝手に implementation-ready として固定しない。
- merge / close / cleanup 権限を持たない場面で勝手に完了処理しない。
- Ticket tools があるからといって arbitrary filesystem write を行わない。
- broad multi-Ticket effort のために新しい umbrella/progress-container Ticket を作らない。
- parent/child、sub-ticket、umbrella、part-of、contains などの hierarchy/container relation を split/refinement の代替として扱わない。
- Notification だけを完了証拠にしない。Pod output / diff / validation / Ticket evidence を確認する。
- 具体的な不足項目を言語化できない場合に、単に risky という理由だけで `planning` に戻さない。その場合は IntentPacket に escalation / reviewer focus を明記して進める。
- risk flags / risky domain / authority-adjacent domain を automatic stop gate として扱わない。これらは bounded context lookup、IntentPacket invariant、reviewer focus、escalation condition に反映する signal である。

## 使用する Ticket tools

利用可能なら、以下を使う。

- `TicketList`: routing 候補や関連 Ticket の確認。
- `TicketShow`: 対象 Ticket の body / thread / artifacts / resolution / typed relation metadata と derived inverse/blocker view を確認。
- `TicketComment`: routing decision / intent packet / blocked reason / next question の記録。
- `TicketWorkflowState`: `queued -> inprogress` acceptance、`inprogress -> done`、または concrete missing decision/information reason を伴う `ready|queued -> planning` に使う。
- `TicketRelationQuery`: project-level の forward relation (`depends_on` / `blocks` / `related` / `supersedes` / `duplicate_of`) を読む。`depends_on` と incoming unresolved `blocks` は queue/acceptance blocker であり、`related` は blocker ではない。`supersedes` / `duplicate_of` は visible diagnostic として扱い、自動的な lifecycle 変更や scheduler 判断にはしない。
- `TicketOrchestrationPlanQuery`: 対象 Ticket や関連 Ticket の ordering / blocker / conflict / waiting-capacity / accepted-plan 記録を読む。queued acceptance 前に必ず確認する。
- `TicketOrchestrationPlanRecord`: Orchestrator が routing 中に project-relevant な ordering / dependency / conflict / capacity/waiting / accepted-plan decision を残す。これは queue reorder、自動起動、state 変更ではない。
- `TicketClose`: 完了権限と resolution が揃っている場合だけ使う。
- `TicketDoctor`: routing 前後の整合性確認。

`TicketCreate` は通常 Intake の責務だが、routing 中に follow-up Ticket が必要だと判断した場合は、ユーザー/上位 Orchestrator の合意後にだけ使う。

## Queued acceptance contract

- `queued -> inprogress` acceptance の直前に `TicketShow` / `TicketRelationQuery` の relation blockers を再確認する。unresolved `depends_on` や incoming unresolved `blocks` が残る場合は implementation side effect を始めず、理由を thread に残して `planning` へ戻すか blocked diagnostic として停止する。
- Relation metadata は project-level constraint であり、OrchestrationPlan は runtime ordering/capacity decision である。relation を OrchestrationPlan で代替しないし、OrchestrationPlan を durable dependency authority として扱わない。

`state = queued` は、Ticket が routing 対象として人間により Orchestrator へ渡された状態である。Orchestrator は queued notification を受けたら、Ticket、workspace state、対象 Ticket の `TicketOrchestrationPlanQuery` 記録、risk domain に応じた bounded project context を読んで、次のどちらかを行う。

- unblocked と判断する場合: `queued -> inprogress` を記録してから worktree 作成、implementation/review Pod spawn、その他の implementation side effect に進む。
  - `before` / `after` / `blocked_by` / `blocks` / `conflicts_with` / `do_not_parallelize` / waiting-capacity 記録がある場合、それを acceptance 判断の入力にする。記録は自動 scheduler ではないため、実際に進めるかどうかは Orchestrator が読んだうえで明示的に決める。
  - risk flags / risky domain がある場合は、IntentPacket に invariants / reviewer focus / escalation conditions を入れる。risk flag だけを `queued -> planning` の理由にしない。
- concrete missing decision / information がある場合: `TicketWorkflowState` で `queued -> planning` を記録し、reason/body と `TicketComment` に不足項目、checked context、なぜ coder の implementation latitude では解決できないか、次の planning question/action を残す。既存の claimed live/restorable Intake/Planning Pod があり、既存通知経路が使える場合は同じ理由を通知する。
- external action 待ちなど planning では解決しない blocker の場合: concise な理由を Ticket thread に記録し、必要に応じて attention / action-required frontmatter や `TicketOrchestrationPlanRecord` の blocker/waiting-capacity 記録で明示する。lifecycle 外の storage bucket を routing target にしない。

Invariant:

- `queued -> inprogress` は Orchestrator acceptance marker であり、coder Pod が既に動いていることの後追い記録ではない。
- `queued -> inprogress` に失敗した場合、implementation/review Pod を spawn しない。
- `inprogress` acceptance 後に worktree/spawn/first-run が失敗した場合は、queued に黙って戻さず、in-progress Ticket に failure/block/recovery note を記録する。
- Queue notification だけを根拠に blind spawn しない。必ず Ticket と workspace state を再確認する。

## Routing classification

Orchestrator は対象 Ticket を以下のいずれかに分類する。複数に見える場合は、次に必要な action が最も早いものを選ぶ。

### `requirements_sync_needed`

まだ `planning` に留めるべき、または `planning -> ready` に進める前に clarification が必要な状態。

条件:

- Ticket の目的は見えるが、observable な完了条件が書けない。
- ユーザー判断がないと product/API/UX を固定してしまう。
- acceptance criteria、binding decisions / invariants、または escalation conditions が不足している。
- existing decisions / docs / closed Tickets と矛盾する可能性がある。

Action:

- Intake / human / Planning sync に戻す。
- `TicketComment` で不足情報と質問を記録する。
- coder Pod は起動しない。

### `return_to_planning`

`ready` または `queued` とされているが、bounded project-context checks の後でも、実装 side effect 前に解決すべき concrete missing decision / information が残っている状態。

条件:

- product / API / UX / authority boundary / storage migration / security / secrets などについて、実装前に決めなければならない具体項目がある。
- 複数の自然な方針があり、human / Orchestrator decision なしでは固定できない。
- acceptance criteria、binding decisions、または escalation conditions に、実装可否を左右する具体的欠落がある。
- Ticket text / risk flags だけではなく、関連 Ticket/thread/artifacts、orchestration plan、関連 workflow/docs/code、durable project context、現在の workspace evidence の relevant subset を確認しても、既存 recorded decision が見つからない。
- その不足は coder の bounded implementation latitude / local tactic selection では安全に解消できない。

Action:

- `TicketWorkflowState` で `ready -> planning` または `queued -> planning` を記録する。
- reason/body と `TicketComment` に以下を明記する。
  - concrete missing decision / information。
  - context checked（Ticket thread/artifacts、関連 Ticket/plan、docs/workflow/code/durable context/workspace state のうち実際に確認したもの）。
  - why implementation latitude is insufficient（なぜ coder/reviewer の local tactic や escalation condition では足りないか）。
  - next planning question/action。
- risk flag / risky domain だけを return reason にしない。
- 既存の claimed live/restorable Intake/Planning Pod があり、既存通知経路が使える場合は同じ理由を通知する。実用的な経路がない場合は follow-up として report する。
- planning が再度 `ready` にするまで coder Pod は起動しない。

### `spike_needed`

実装前に read-only 調査が必要。

条件:

- 技術的実現性が不明。
- dependency / license / packaging / portability / performance / diagnostics の確認が必要。
- current code map が不足している。
- bug の再現条件や観測 evidence が不足している。

Action:

- read-only investigation を提案する。
- 許可があれば task-specific read-only helper Pod を普通の scoped Pod として起動できる。
- `TicketComment` に調査問い・scope・完了条件を記録する。
- 実装 worktree はまだ作らない。

### `implementation_ready`

既存 multi-agent workflow に渡せる状態。

条件:

- intent / binding decisions / invariants / implementation latitude / acceptance criteria が明確。
- binding decisions / invariants と implementation latitude が区別されている。
- reviewer が判断する basis と escalation conditions が明確。
- validation が書ける。
- design / authority boundary の未決定がない、または planning return / human decision で補われている。
- risk flags / risky domain は context lookup と reviewer focus に反映済みで、bounded context check 後に concrete missing decision / information が残っていない。
- 残る不確実性が bounded implementation investigation / local tactic selection に閉じている。
- IntentPacket を短く書ける。

Action:

- `IntentPacket` を作る。risky-but-specified Ticket では、risk を stop reason にせず、binding invariants / implementation latitude / escalation conditions / Critical risks / reviewer focus として記録する。
- project-relevant な ordering / blocker / conflict / capacity decision や accepted work plan がある場合は、`TicketOrchestrationPlanRecord` に bounded typed record として残す。local session/socket/raw model output は入れない。
- `TicketComment` に routing decision と IntentPacket を記録する。
- 許可があれば `multi-agent-workflow` に接続し、worktree + coder/reviewer sibling loop に進む。

### `review_needed`

実装はあるが、review / acceptance が未完了。

条件:

- implementation report / branch / commit / diff がある。
- external review がない、または reviewer blocker が未解決。
- validation evidence が足りない。

Action:

- reviewer Pod 起動または追加 validation を提案する。
- reviewer は recorded intent / binding decisions / invariants / implementation latitude / acceptance criteria / explicit escalation conditions に照らして判断する。不記録の好みや Orchestrator の未共有 preferred tactic を基準にしない。
- `TicketComment` に review target と確認観点を記録する。
- blocker 未解決のまま merge-ready としない。

### `blocked_action_required`

人間判断または外部イベント待ち。

条件:

- design/product/security 判断が必要だが、planning で同期すれば進められる種類ではない。
- credential / secret / environment / external service が必要。
- 別 Ticket / branch / upstream change の完了待ち。
- scope/permission が不足している。

Action:

- 必要な判断・外部 action を短く書く。
- `TicketComment` に blocked reason と next question を記録する。
- 必要に応じて attention / action-required frontmatter や orchestration plan の blocker/waiting-capacity 記録で、待ち理由を current state とは別に表す。lifecycle 外の storage bucket へ移す route は使わない。

### `close_ready`

完了処理に進める状態。

条件:

- requirements / acceptance criteria が満たされている。
- review / validation / merge / cleanup evidence が揃っている。
- resolution を書ける。
- close 権限がある。
- 既存 umbrella/progress-container Ticket については、concrete follow-up Ticket と必要な Objective context が存在し、container role を superseded/decomposed として退役できる。

Action:

- `TicketClose` または既存 close workflow で resolution を記録する。
- umbrella/progress-container Ticket を退役する close resolution では、関連作業がすべて完了したという意味ではなく container role を retired したことを明記し、完了済み concrete Ticket と残る follow-up Ticket / Objective を列挙する。
- close 権限がない場合は merge-ready / close-ready dossier を親/人間に提出する。

### `closed_or_noop`

routing 不要。

条件:

- closed Ticket。
- duplicate として既存 Ticket に統合済み。
- Ticket 自体が不要と判断済み。

Action:

- 追加 action なし。
- 必要なら related Ticket へ comment する。

## Routing 手順

### 1. 状態確認

- `git state --short --branch`
- `TicketShow <target>`
- 関連 Ticket の `TicketList` / `TicketShow`
- 必要に応じて docs/code/workflow/history
- 必要に応じて visible Pods / worktrees / branches

この段階で unrelated dirty changes がある場合、実装/merge には進まず、routing decision の記録だけに留めるかユーザーに確認する。

### 2. Ticket evidence を読む

最低限、以下を確認する。

- Background
- Requirements
- Acceptance criteria
- Binding decisions / invariants
- Implementation latitude
- Readiness / open questions / risk flags
- Escalation conditions
- Validation
- Thread の plan / decision / implementation_report / review
- Artifacts / branch / commit references
- risk flags が示す domain（authority-boundary / scope / permission / Pod / persistence / prompt-context / public-api など）

### 3. Bounded project-context checks を行う

Ticket evidence だけで planning return を決めない。risk flags や risky domain がある場合は、必要な最小限の project context を確認する。

代表的な確認先:

- Ticket thread/artifacts/resolution と関連 Ticket。
- `TicketOrchestrationPlanQuery` の accepted-plan / ordering / blocker / conflict 記録。
- 関連 workflow/docs/design/development docs。
- 現在の code map や既存 implementation pattern。
- durable memory/Knowledge/project decisions（利用可能で、判断に関係する場合）。
- repository / branch / worktree / visible Pod state。

目的は broad repository archaeology ではなく、以下を区別できるだけの bounded check である。

- genuinely missing binding decision / requirement / invariant / acceptance criterion。
- Ticket thread、関連 Ticket、docs/workflows/code、durable context にすでに記録された decision。
- coder が implementation latitude の中で選べる local tactic / bounded investigation。

### 4. Classification を決める

例:

- implementation-ready に見えるが authority boundary の explicit decision がなく、bounded context check でも recorded decision がない → concrete missing decision として `return_to_planning`; routing record に missing item / checked context / implementation latitude では足りない理由 / next question を書く。
- implementation-ready に見えるが単に risk が高い → `implementation_ready` とし、IntentPacket に escalation / reviewer focus を明記する。
- `allow-spawnpod-child-workspace-cwd` のように scope / Pod / authority-adjacent domain に触れるが、intent、authority invariants、implementation latitude、escalation conditions が指定済み → `implementation_ready`。context check で既存 SpawnPod scope/cwd policy と workflow invariants を確認し、reviewer focus に authority leakage / workspace-cwd separation / delegation-scope preservation を入れる。risk domain だけを理由に `planning` へ戻さない。
- 実装済みだが review がない → `review_needed`
- 要件が曖昧で spike も必要そう → `requirements_sync_needed` を優先し、調査問いを明確化する
- 完了しているが close 権限がない → `close_ready` として dossier を返す

### 5. Routing decision を Ticket に記録する

`TicketComment` の role は通常 `decision` または `plan` を使う。

標準形:

```markdown
Routing decision: <classification>

Reason:
- ...

Evidence checked:
- Ticket body / thread / artifacts
- related Ticket(s) / orchestration plan records
- code/docs/workflow paths
- branch/worktree/Pod state if relevant

If returning to planning:
- Missing decision/information: ...
- Context checked: summarize the relevant subset from Evidence checked above
- Why implementation latitude is insufficient: ...
- Next planning question/action: ...

Next action:
- ...

Escalate if:
- ...
```

### 6. IntentPacket を作る（implementation_ready の場合）

`multi-agent-workflow` に渡す前に、以下を短くまとめる。

```text
Intent:
- 何を実現するか。

Binding decisions / invariants:
- 人間/Orchestrator/Ticket に記録済みで coder / reviewer が従うべき decision と、壊してはいけない design / authority boundary。
- 具体的な除外・触れてはいけない場所が binding decision である場合はここに書く。

Requirements / acceptance criteria:
- 完了時に満たす observable な要件と reviewer が判断できる基準。

Implementation latitude:
- Coder が調査しながら選んでよい local tactic / file-local organization / bounded uncertainty。

Escalate if:
- 親/人間に戻す判断条件。特に product / API / UX / authority boundary / explicit design constraint を変える必要が出た場合。

Validation:
- 実行すべき format / build / test / doctor。

Current code map:
- 実装対象と触ってはいけない場所。

Critical risks / reviewer focus:
- reviewer にも見てほしい失敗パターン。reviewer は recorded intent / binding decisions / invariants / implementation latitude / acceptance criteria / explicit escalation conditions に照らして判断し、不記録の preferred tactic を基準にしない。
```

IntentPacket が短く書けない場合、`implementation_ready` ではなく `return_to_planning` または `requirements_sync_needed` に戻す。

### 7. 後続 Workflow へ接続する

- `requirements_sync_needed` → `ticket-intake-workflow` / human / planning sync
- `return_to_planning` → `ticket-preflight-workflow`（legacy compatibility canonical id の planning sync entry）
- `spike_needed` → read-only investigation plan / Pod（許可後）
- `implementation_ready` → `multi-agent-workflow`
- `review_needed` → reviewer Pod / review workflow
- `blocked_action_required` → human / parent Orchestrator
- `close_ready` → close workflow / maintainer decision

## 完了条件

この Workflow の完了条件は次のいずれかである。

- routing decision が Ticket に記録され、次に接続する Workflow / human action が明確である。
- `ready` / `queued` を `planning` に戻した場合、typed state-change/routing event に concrete missing decision / information、checked context、implementation latitude では足りない理由、次の planning question/action が残っている。
- implementation-ready Ticket について IntentPacket が Ticket に記録され、`multi-agent-workflow` に渡せる。
- requirements-sync / planning return / spike / blocked / review / close-ready の理由と次 action が Ticket に記録されている。
- routing 不要と判断され、その理由が明確である。

## この Workflow で固定しないもの

- unattended scheduler。
- LeaseStore / queue persistence。
- action-required dashboard UI。
- automatic Pod spawning policy。
- TicketUpdate tool の導入。
- external tracker integration。

これらは routing decision を人間/Orchestrator が明示的に扱えるようになった後の follow-up とする。
