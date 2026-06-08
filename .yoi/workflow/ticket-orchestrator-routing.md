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

## 位置づけ

```text
TicketCreate / TicketComment
  -> Ticket Orchestrator Routing Workflow
  -> planning return / requirements sync / spike / implementation / review / blocked / close / pending
  -> 必要に応じて他 Workflow へ接続
```

- Intake は Ticket の materialization と planning/clarification を担当する role であり、workflow_state 名ではない。
- workflow_state は `planning -> ready -> queued -> inprogress -> done` を基本遷移とする。
- `ticket-preflight-workflow` は legacy slug 互換の planning/requirements sync entry であり、`preflight` を独立 state / lane / long-lived operation として扱わない。
- `ready -> queued` は人間が Orchestrator routing を許可した状態であり、worktree 作成や Pod 起動の許可そのものではない。
- `multi-agent-workflow` は coder / reviewer Pod と worktree を使う実装・レビュー loop。
- この Workflow は自動 scheduler / lease / unattended maintainer ではない。

## Orchestrator の責務

Orchestrator は以下を行う。

- Ticket を `TicketShow` で読む。
- 必要に応じて関連 Ticket を `TicketList` / `TicketShow` で確認する。
- Ticket body / thread / artifacts / resolution / review / implementation report を読む。
- repository 状態、関連 docs/code、既存 worktree、visible Pods を必要に応じて明示的に確認する。
- queued notification を受けた場合も、Ticket と workspace state を再確認してから routing する。
- next action を routing classification として決める。
- routing decision を `TicketComment` で Ticket thread に記録する。
- implementation-ready の場合は `multi-agent-workflow` に渡す `IntentPacket` を作る。
- implementation-ready かつ Ticket が `queued` の場合は、worktree 作成 / implementation Pod `SpawnPod` / coder routing などの side effect の前に、既存の typed Ticket backend/tool path で `queued -> inprogress` を記録する。
- `ready` または `queued` に具体的な不足 decision / information がある場合だけ、typed state-change/routing event 付きで `planning` に戻す。

## Orchestrator がしないこと

- 自動 scheduler として unattended に実行しない。
- Panel Queue / queued notification だけを unattended scheduler trigger として扱わない。
- `queued -> inprogress` acceptance なしに worktree 作成、implementation Pod `SpawnPod`、coder/reviewer routing を行わない。
- 人間/上位 Orchestrator の許可または明示的な routing acceptance なしに coder / reviewer Pod や read-only investigation helper Pod を起動しない。
- 設計境界の未決定を勝手に implementation-ready として固定しない。
- merge / close / cleanup 権限を持たない場面で勝手に完了処理しない。
- Ticket tools があるからといって arbitrary filesystem write を行わない。
- Notification だけを完了証拠にしない。Pod output / diff / validation / Ticket evidence を確認する。
- 具体的な不足項目を言語化できない場合に、単に risky という理由だけで `planning` に戻さない。その場合は IntentPacket に escalation / reviewer focus を明記して進める。

## 使用する Ticket tools

利用可能なら、以下を使う。

- `TicketList`: routing 候補や関連 Ticket の確認。
- `TicketShow`: 対象 Ticket の body / thread / artifacts / resolution 確認。
- `TicketComment`: routing decision / intent packet / blocked reason / next question の記録。
- `TicketStatus`: pending/open などの状態整理が明示的に許可された場合だけ使う。
- `TicketWorkflowState`: `queued -> inprogress` acceptance、`inprogress -> done`、または concrete missing decision/information reason を伴う `ready|queued -> planning` に使う。
- `TicketOrchestrationPlanQuery`: 対象 Ticket や関連 Ticket の ordering / blocker / conflict / waiting-capacity / accepted-plan 記録を読む。queued acceptance 前に必ず確認する。
- `TicketOrchestrationPlanRecord`: Orchestrator が routing 中に project-relevant な ordering / dependency / conflict / capacity/waiting / accepted-plan decision を残す。これは queue reorder、自動起動、workflow_state 変更ではない。
- `TicketClose`: 完了権限と resolution が揃っている場合だけ使う。
- `TicketDoctor`: routing 前後の整合性確認。

`TicketCreate` は通常 Intake の責務だが、routing 中に follow-up Ticket が必要だと判断した場合は、ユーザー/上位 Orchestrator の合意後にだけ使う。

## Queued acceptance contract

`workflow_state = queued` は、Ticket が routing 対象として人間により Orchestrator へ渡された状態である。Orchestrator は queued notification を受けたら、Ticket、workspace state、対象 Ticket の `TicketOrchestrationPlanQuery` 記録を読んで、次のどちらかを行う。

- unblocked と判断する場合: `queued -> inprogress` を記録してから worktree 作成、implementation/review Pod spawn、その他の implementation side effect に進む。
  - `before` / `after` / `blocked_by` / `blocks` / `conflicts_with` / `do_not_parallelize` / waiting-capacity 記録がある場合、それを acceptance 判断の入力にする。記録は自動 scheduler ではないため、実際に進めるかどうかは Orchestrator が読んだうえで明示的に決める。
- concrete missing decision / information がある場合: `TicketWorkflowState` で `queued -> planning` を記録し、reason/body に不足項目を残す。既存の claimed live/restorable Intake/Planning Pod があり、既存通知経路が使える場合は同じ理由を通知する。
- external action 待ちなど planning では解決しない blocker の場合: concise な理由を Ticket thread に記録し、queued のまま待つか、既存の Ticket status/state mechanism で明示的に defer/block する。

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

`ready` または `queued` とされているが、実装 side effect 前に具体的な不足 decision / information が見つかった状態。

条件:

- product / API / UX / authority boundary / storage migration / security / secrets などについて、実装前に決めなければならない具体項目がある。
- 複数の自然な方針があり、human / Orchestrator decision なしでは固定できない。
- acceptance criteria、binding decisions、または escalation conditions に、実装可否を左右する具体的欠落がある。

Action:

- `TicketWorkflowState` で `ready -> planning` または `queued -> planning` を記録する。
- reason/body に具体的な不足項目を含める。
- `TicketComment` で routing decision と質問を記録する。
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
- 残る不確実性が bounded implementation investigation / local tactic selection に閉じている。
- IntentPacket を短く書ける。

Action:

- `IntentPacket` を作る。
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
- 必要なら `TicketStatus` で pending に移す（許可がある場合だけ）。

### `close_ready`

完了処理に進める状態。

条件:

- requirements / acceptance criteria が満たされている。
- review / validation / merge / cleanup evidence が揃っている。
- resolution を書ける。
- close 権限がある。

Action:

- `TicketClose` または既存 close workflow で resolution を記録する。
- close 権限がない場合は merge-ready / close-ready dossier を親/人間に提出する。

### `defer_pending`

今は進めないが blocked ではない。

条件:

- 優先度・タイミングの理由で後回し。
- 依存はあるが active blocker として扱うほどではない。
- umbrella / roadmap 的に保持する。

Action:

- defer reason を `TicketComment` に記録する。
- 必要なら `TicketStatus` で pending に移す（許可がある場合だけ）。

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

- `git status --short --branch`
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

### 3. Classification を決める

例:

- implementation-ready に見えるが authority boundary の explicit decision がない → concrete missing decision として `return_to_planning`; explicit decision が Ticket/thread にあるなら binding として IntentPacket に載せる。
- implementation-ready に見えるが単に risk が高い → `implementation_ready` とし、IntentPacket に escalation / reviewer focus を明記する。
- 実装済みだが review がない → `review_needed`
- 要件が曖昧で spike も必要そう → `requirements_sync_needed` を優先し、調査問いを明確化する
- 完了しているが close 権限がない → `close_ready` として dossier を返す

### 4. Routing decision を Ticket に記録する

`TicketComment` の role は通常 `decision` または `plan` を使う。

標準形:

```markdown
Routing decision: <classification>

Reason:
- ...

Evidence checked:
- Ticket body / thread / artifacts
- related Ticket(s)
- code/docs/workflow paths
- branch/worktree/Pod state if relevant

Next action:
- ...

Escalate if:
- ...
```

### 5. IntentPacket を作る（implementation_ready の場合）

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

### 6. 後続 Workflow へ接続する

- `requirements_sync_needed` → `ticket-intake-workflow` / human / planning sync
- `return_to_planning` → `ticket-preflight-workflow`（legacy compatibility slug の planning sync entry）
- `spike_needed` → read-only investigation plan / Pod（許可後）
- `implementation_ready` → `multi-agent-workflow`
- `review_needed` → reviewer Pod / review workflow
- `blocked_action_required` → human / parent Orchestrator
- `close_ready` → close workflow / maintainer decision
- `defer_pending` → pending / roadmap management

## 完了条件

この Workflow の完了条件は次のいずれかである。

- routing decision が Ticket に記録され、次に接続する Workflow / human action が明確である。
- `ready` / `queued` を `planning` に戻した場合、typed state-change/routing event に concrete missing decision / information reason が残っている。
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
