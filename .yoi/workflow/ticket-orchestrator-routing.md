---
description: Ticket を読み、Orchestrator が preflight / spike / implementation / review / blocked / close へ明示的に routing する workflow
model_invokation: true
user_invocable: true
requires: []
---
# Ticket Orchestrator Routing Workflow

Yoi の multi-agent 運用で、Intake や人間が作成した Ticket を Orchestrator が読み、次に取るべき action を明示的に分類・記録するための Workflow。

これは scheduler ではない。目的は、Ticket の fields / body / thread / artifacts / 現在の repository/Pod 状態を明示的に確認し、隠れた会話状態ではなく Ticket に基づいて routing 判断を残すことである。

## 位置づけ

```text
TicketCreate / TicketComment
  -> Ticket Orchestrator Routing Workflow
  -> requirements sync / preflight / spike / implementation / review / blocked / close / pending
  -> 必要に応じて他 Workflow へ接続
```

- Intake は Ticket の materialization を担当する。
- Orchestrator は Ticket の next action を分類する。
- `ticket-preflight-workflow` は実装前の設計・要件 gate。
- `multi-agent-workflow` は coder / reviewer Pod と worktree を使う実装・レビュー loop。
- この Workflow は自動 scheduler / lease / unattended maintainer ではない。

## Orchestrator の責務

Orchestrator は以下を行う。

- Ticket を `TicketShow` で読む。
- 必要に応じて関連 Ticket を `TicketList` / `TicketShow` で確認する。
- Ticket body / thread / artifacts / resolution / review / implementation report を読む。
- repository 状態、関連 docs/code、既存 worktree、visible Pods を必要に応じて明示的に確認する。
- next action を routing classification として決める。
- routing decision を `TicketComment` で Ticket thread に記録する。
- implementation-ready の場合は `multi-agent-workflow` に渡す `IntentPacket` を作る。
- preflight-needed の場合は coder Pod に直投げせず、`ticket-preflight-workflow` に接続する。

## Orchestrator がしないこと

- 自動 scheduler として unattended に実行しない。
- 人間/上位 Orchestrator の許可なしに coder / reviewer / investigator Pod を起動しない。
- 設計境界の未決定を勝手に implementation-ready として固定しない。
- merge / close / cleanup 権限を持たない場面で勝手に完了処理しない。
- Ticket tools があるからといって arbitrary filesystem write を行わない。
- Notification だけを完了証拠にしない。Pod output / diff / validation / Ticket evidence を確認する。

## 使用する Ticket tools

利用可能なら、以下を使う。

- `TicketList`: routing 候補や関連 Ticket の確認。
- `TicketShow`: 対象 Ticket の body / thread / artifacts / resolution 確認。
- `TicketComment`: routing decision / intent packet / blocked reason / next question の記録。
- `TicketStatus`: pending/open などの状態整理が明示的に許可された場合だけ使う。
- `TicketClose`: 完了権限と resolution が揃っている場合だけ使う。
- `TicketDoctor`: routing 前後の整合性確認。

`TicketCreate` は通常 Intake の責務だが、routing 中に follow-up Ticket が必要だと判断した場合は、ユーザー/上位 Orchestrator の合意後にだけ使う。

## Routing classification

Orchestrator は対象 Ticket を以下のいずれかに分類する。

### `requirements_sync_needed`

仕様・用語・UX・責務境界・受け入れ条件が未同期。

条件:

- Ticket の目的は見えるが、observable な完了条件が書けない。
- ユーザー判断がないと product/API/UX を固定してしまう。
- acceptance criteria または non-goals が不足している。
- existing decisions / docs / closed Tickets と矛盾する可能性がある。

Action:

- Intake / human に戻す。
- `TicketComment` で不足情報と質問を記録する。
- coder Pod は起動しない。

### `preflight_needed`

実装前に設計境界・要件・反証観点を同期すべき状態。

条件:

- profile / manifest / scope / permission / session / history / Pod metadata / prompt context に触れる。
- public API / plugin / feature boundary / storage migration / security / secrets に触れる。
- 複数の自然な実装方針がある。
- implementation-ready に見えるが、reviewer が diff だけでは見落としやすい設計リスクがある。
- `needs_preflight: true` または同等の記述が Ticket にある。

Action:

- `ticket-preflight-workflow` に接続する。
- `TicketComment` で preflight reason を記録する。
- preflight が implementation-ready にするまで coder Pod は起動しない。

### `spike_needed`

実装前に read-only 調査が必要。

条件:

- 技術的実現性が不明。
- dependency / license / packaging / portability / performance / diagnostics の確認が必要。
- current code map が不足している。
- bug の再現条件や観測 evidence が不足している。

Action:

- read-only investigation を提案する。
- 許可があれば investigator Pod を read-only scope で起動できる。
- `TicketComment` に調査問い・scope・完了条件を記録する。
- 実装 worktree はまだ作らない。

### `implementation_ready`

既存 multi-agent workflow に渡せる状態。

条件:

- background / requirements / acceptance criteria が明確。
- non-goals / invariants / escalation conditions が必要十分。
- validation が書ける。
- design / authority boundary の未決定がない、または preflight 済み。
- IntentPacket を短く書ける。

Action:

- `IntentPacket` を作る。
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
- `TicketComment` に review target と確認観点を記録する。
- blocker 未解決のまま merge-ready としない。

### `blocked_action_required`

人間判断または外部イベント待ち。

条件:

- design/product/security 判断が必要。
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
- Non-goals / invariants
- Readiness / needs_preflight / risk flags
- Escalation conditions
- Validation
- Thread の plan / decision / implementation_report / review
- Artifacts / branch / commit references

### 3. Classification を決める

1つに決める。複数に見える場合は、次に必要な action が最も早いものを選ぶ。

例:

- implementation-ready だが authority boundary に触れる → `preflight_needed`
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

Requirements:
- 完了時に満たす observable な要件。

Invariants:
- 壊してはいけない design / authority boundary。

Non-goals:
- 今回やらないこと。

Escalate if:
- 親/人間に戻す判断条件。

Validation:
- 実行すべき format / build / test / doctor。

Current code map:
- 実装対象と触ってはいけない場所。

Critical risks:
- reviewer にも見てほしい失敗パターン。
```

IntentPacket が短く書けない場合、`implementation_ready` ではなく `preflight_needed` または `requirements_sync_needed` に戻す。

### 6. 後続 Workflow へ接続する

- `requirements_sync_needed` → `ticket-intake-workflow` / human
- `preflight_needed` → `ticket-preflight-workflow`
- `spike_needed` → read-only investigation plan / Pod（許可後）
- `implementation_ready` → `multi-agent-workflow`
- `review_needed` → reviewer Pod / review workflow
- `blocked_action_required` → human / parent Orchestrator
- `close_ready` → close workflow / maintainer decision
- `defer_pending` → pending / roadmap management

## 完了条件

この Workflow の完了条件は次のいずれかである。

- routing decision が Ticket に記録され、次に接続する Workflow / human action が明確である。
- implementation-ready Ticket について IntentPacket が Ticket に記録され、`multi-agent-workflow` に渡せる。
- requirements-sync / preflight / spike / blocked / review / close-ready の理由と次 action が Ticket に記録されている。
- routing 不要と判断され、その理由が明確である。

## この Workflow で固定しないもの

- unattended scheduler。
- LeaseStore / queue persistence。
- action-required dashboard UI。
- automatic Pod spawning policy。
- TicketUpdate tool の導入。
- external tracker integration。

これらは routing decision を人間/Orchestrator が明示的に扱えるようになった後の follow-up とする。
