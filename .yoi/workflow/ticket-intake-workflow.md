---
description: ユーザーの曖昧な依頼を要件同期し、合意済みの Ticket として作成・更新する Intake workflow
model_invokation: true
user_invocable: true
requires: []
---
# Ticket Intake Workflow

Yoi の multi-agent 運用で、ユーザーの依頼をいきなり実装委譲せず、まず **合意済み Ticket** または「まだ Ticket 化しない」判断に変換するための Workflow。

この workspace workflow は bundled `resources/workflows/ticket-intake-workflow.md` を dogfooding 用に詳述した override である。Objective / split policy / local Ticket 運用の説明を追加するが、bundled workflow の調査ゲート、Ticket 作成前の user agreement、Intake の非 scheduler 境界を弱めてはならない。

Intake の目的は、ユーザーの意図・要件・制約・受け入れ条件・未決定点を明確にし、Orchestrator が次の routing を判断できる Ticket を作ることである。Intake は scheduler ではなく、coder / reviewer / read-only investigation helper Pod を起動しない。

## 位置づけ

```text
User request / conversation
  -> Ticket Intake Workflow
  -> TicketCreate / TicketComment
  -> Orchestrator routing
  -> planning sync / spike / implementation / review / blocked / close
```

- `Ticket` は durable orchestration record。
- `Objective` は medium-term goal / motivation / strategy / success criteria / decision context の project record。Objective context は判断背景であり、Ticket body/thread/artifacts を読む代替ではない。
- `Task` は session-local progress tracking。
- `Assignment` は Orchestrator から coder / reviewer Pod、または task-specific helper Pod への具体的委譲。
- `IntentPacket` は Ticket から抽出して Assignment に渡す短い実装・レビュー契約。

Intake は、要件同期と Ticket 化を担当する。実装の起動・worktree 作成・review 委譲・merge 判断は Orchestrator 側の責務である。`ready` は Orchestrator が routing できる状態を意味し、実装戦術がすべて事前固定されていることを意味しない。

Ticket に残す内容は、binding decisions / invariants、implementation latitude、escalation conditions を区別する。Coder が調査しながら局所的な実装手段を選べる余地は残してよいが、product / API / UX / authority boundary / explicit design constraint を silently 決める余地を残してはならない。

## Intake の責務

Intake は以下を行う。

- ユーザー依頼の主語と目的を確認する。
- 既存 Ticket を確認し、duplicate / related work を探す。
- 曖昧な依頼、現在挙動への claim、authority boundary、workflow/source-of-truth 変更では、Ticket 化前の最小調査ゲートとして関連 docs / code / workflow / history を読む。
- 不足している要件を質問する。
- 作成または refinement する Ticket が、実装・レビュー・検証・完了判断を単独で行える concrete work item であるか確認する。
- 広い依頼を分割する場合は、進捗コンテナとしての umbrella Ticket ではなく、concrete Ticket / Objective context / split decision record に責務を分ける。
- Objective-to-Ticket links を提案する場合は canonical opaque Ticket ID だけを使い、dependency / blocking / ordering relation として扱わない。
- Ticket の title / body/request snapshot / acceptance criteria / priority / readiness / risk flags を、現在の要件として意味がある範囲で提案する。
- canonical ID は Ticket 作成/storage が opaque な path-derived value として割り当てるため、Intake はユーザー向け metadata として提案しない。
- ユーザー主張、Intake が確認した事実、未確認仮説、未決定点を分けて整理する。
- background / requirements / acceptance criteria / escalation conditions を整理する。
- binding decisions / invariants と implementation latitude を分けて書く。
- 具体的な除外や触れてはいけない境界が binding decision である場合は、generic な除外リストではなく invariant / escalation condition として明記する。
- readiness / open questions / risk flags を明示する。
- ユーザー合意後にだけ official Ticket を作成する。
- 既存 Ticket の refinement を求められた場合は、TicketComment で経緯を残す。

## Intake がしないこと

- coder / reviewer Pod や read-only investigation helper Pod を起動しない。
- worktree を作らない。
- merge / close / branch cleanup をしない。
- implementation-ready でない Ticket を実装に投げない。
- unattended scheduler として自動実行しない。
- ユーザー合意なしに official Ticket を作らない。
- broad effort の進捗を保持するためだけの long-lived umbrella / progress-container Ticket を作らない。
- parent/child、sub-ticket、umbrella、part-of、contains などの hierarchy/container relation を split の代替として提案しない。
- secrets / private context を Ticket body / thread / artifacts / diagnostics に保存しない。
- arbitrary filesystem write で `work-items/` を編集しない。Ticket 操作は Ticket tools を使う。

## 使用する Ticket tools

利用可能なら、以下の typed Ticket tools を使う。

- `TicketList`: 既存 Ticket の一覧・重複確認。
- `TicketShow`: 関連 Ticket の詳細確認。
- `TicketCreate`: 合意済み Ticket の作成。
- `TicketComment`: 既存 Ticket refinement / decision / plan の記録。
- `TicketDoctor`: 必要に応じた整合性確認。

Intake は `MergeRequest*`, `TicketWorkflowState`, `TicketClose` を通常使わない。review authority は assigned Coder が起動した read-only direct-child Reviewer の immutable Merge Request attempt に属し、completion / merge / close は各guarded workflowの責務である。

Ticket tools が利用できない環境では、勝手に file write で代替しない。ユーザーまたは Orchestrator に「Ticket tools がないため materialize できない」と報告し、必要なら `yoi ticket` を使える人間/親 workflow に戻す。

## 手順

### 1. 依頼を受け取る

ユーザー依頼を短く言い換え、以下を分ける。

- 何を変えたいか。
- なぜ必要か。
- 影響を受けるユーザー / Pod / workflow / crate / docs。
- 既に決まっていること。
- まだ未決定のこと。

この段階では Ticket を作らない。ユーザー発話は request snapshot / claim として扱い、確認済み requirements と混同しない。

### 2. 既存 Ticket を確認する

`TicketList` / `TicketShow` で duplicate / related work を探す。

確認観点:

- 同じ目的の未完了 Ticket がないか。
- closed Ticket の判断・resolution と矛盾しないか。
- 既存の umbrella/progress-container Ticket が、superseded/decomposed として退役できる状態か。
- 既存 concrete follow-up Ticket や Objective context で足りるか、新規 concrete Ticket が必要か。

既存 Ticket の更新で足りる場合、新規 Ticket を作らず、ユーザーに更新案を提示する。

### 2.1. Ticket 化前の最小調査ゲート

`TicketCreate` または material な `TicketComment` の前に、以下の gate を通す。

必ず行うこと:

- duplicate / related / blocking-looking Ticket を確認する。
- 既存 Ticket を更新するなら、その Ticket の item/thread/artifacts を読む。
- ユーザー claim と、Intake が読んで確認した fact を分ける。

次のいずれかに当たる場合は、Ticket 作成前に関連 docs / code / workflow / prompt / config / history を読む。

- 依頼が曖昧、または複数の concrete work item を含む。
- 「現在の挙動」「既存仕様」「壊れている」「既にある」など、事実確認を要する claim がある。
- scope / permission / history / prompt context / persistence / public API など authority boundary に触れる。
- prompt / workflow resource、Ticket schema、source-of-truth 境界の変更に触れる。
- 既存実装の map がないと requirements / acceptance criteria を誤って固定しそうである。

Gate output は draft に以下を分けて残す。

- User claims / request snapshot: ユーザーが述べたこと。
- Confirmed facts / sources: Intake が読んで確認したことと source。
- Unverified hypotheses: ありそうだが未確認の推測。
- Undecided points / open questions: ユーザーまたは Orchestrator の判断が必要なこと。

調査が大きい、current-code map がない、または仕様同期が足りない場合は、official Ticket を作らず draft で止める。readiness は `spike_needed` / `requirements_sync_needed` / `blocked` のいずれかを付け、次に必要な調査や質問を報告する。確認できない claim を requirements / acceptance criteria として保存しない。

### 2.5. Broad request の split policy

1つの依頼が複数の implementable work item を含む場合、Intake は以下を提案する。

- broad effort を表す umbrella/progress-container Ticket は新規作成しない。
- concrete slice ごとに、単独で実装・レビュー・検証・close できる Ticket を作る。
- split した理由と残した範囲は、関連 Ticket の thread、必要なら Objective context に記録する。
- Objective は中期的な目的・方針・success criteria を保持するために使う。Objective record の実装や schema 追加はこの workflow の責務ではない。
- typed Ticket relation が利用可能になった場合も、dependency / related / blocking / replacement / supersedes などの非階層メタデータに限る。
- parent/child、sub-ticket、part-of、contains のような hierarchy/container relation を作らない。

ユーザーが「まず調査 Ticket」「設計 Ticket」「計画 Ticket」を concrete work item として求める場合は作成してよい。禁止されるのは、他の concrete Ticket の進捗を持つためだけに長期 open となる container Ticket である。

### 3. 要件を同期する

最低限、以下を確認する。

- ユーザー claim のうち、どれが確認済み fact で、どれが未確認仮説か。
- observable な完了条件は何か。
- 作業の種類・影響範囲は prose として body に書けばよいが、current Ticket core metadata として扱わない。
- 受け入れ条件は何か。
- binding decision として残す具体的な除外・authority boundary はあるか。
- 後方互換が必要か。
- authority boundary / scope / permission / history / prompt context に触れるか。
- validation は何で確認できるか。
- 人間判断が必要な論点は何か。

不足がある場合は、Ticket 作成前に質問する。質問は多すぎず、Ticket 作成に必要な最小限に絞る。調査が先に必要な場合は `spike_needed`、仕様同期が先に必要な場合は `requirements_sync_needed` として draft に留める。

### 4. readiness を分類する

Ticket 作成または更新前に、readiness を明示する。

```text
implementation_ready:
- 意図・受け入れ条件・binding decisions / invariants / implementation latitude が明確。
- Reviewer が判断できる基準と escalation conditions が明確。
- 実装調査や局所的な tactic 選択は残っていてよいが、product / API / UX / authority boundary / explicit design constraint を coder が silently 決める余地はない。
- validation が書ける。

requirements_sync_needed:
- 目的は見えているが、仕様・用語・UX・責務境界・受け入れ条件が未同期。
- ユーザー claim を requirements として固定するには合意や確認が足りない。

spike_needed:
- 技術調査、依存関係、性能、license、diagnostics、現在コード map が先に必要。
- どの files/workflows/Tickets を読むべきかは見えているが、Intake の最小調査では実装可能な要件まで確定できない。

blocked:
- 人間判断、外部イベント、別 Ticket の完了が必要。

unspecified:
- どうしても分類不能な時だけ使う。理由を Ticket に書く。
```

### 5. open questions / risk flags を付ける

以下に触れる Ticket は risk flags と reviewer/orchestrator focus を Ticket body に短く明記する。これは stop gate ではない。具体的な未決定 decision / information がある場合だけ、blocking open question として記録する。

- profile / manifest / scope / permission。
- session / history / Pod metadata / persistence。
- prompt context 加工原則。
- public API / plugin / feature boundary。
- security / secret / credential handling。
- storage migration / backward compatibility。
- 複数の自然な設計方針があるもの。
- reviewer が diff だけでは見落としやすい設計リスク。

risk flags は短い語でよい。missing boundary がすでに人間/Orchestrator の Ticket-recorded decision で補われている場合は、その decision を根拠に Orchestrator が routing できる。単に risk があるだけなら Orchestrator は Ticket を戻さず、IntentPacket に escalation / reviewer focus を明記して進める。

例:

```text
risk_flags: [authority-boundary, persistence, prompt-context, public-api]
```

### 6. Ticket draft を提示する

ユーザーに作成前 draft を提示する。

標準 draft:

```text
Title:

Priority:
Readiness:
Next Ticket operation: draft_only | create_after_user_agreement | update_existing_after_user_agreement | no_ticket
Risk flags:

User claims / request snapshot:

Confirmed facts / sources:

Unverified hypotheses:

Undecided points / open questions:

Background:

Requirements:

Acceptance criteria:

Binding decisions / invariants:

Implementation latitude:

Escalation conditions:

Validation:

Related tickets/docs/files:
```

canonical ID は作成時に storage が opaque/path-derived value として割り当てるため、draft では提案しない。

この時点ではまだ Ticket を作らない。`Next Ticket operation` が `draft_only` / `no_ticket` の場合は、ユーザー合意があっても `TicketCreate` ではなく追加同期または調査へ戻す。

### 7. ユーザー合意を取る

以下のどちらかがあるまで official Ticket を作らない。

- ユーザーが draft を明示的に承認する。
- ユーザーが「作って」「切って」「記録して」など、作成を明示する。

未決定のまま記録する場合は、`requirements_sync_needed` / `spike_needed` / `blocked` として未決定点を明示する。ユーザー合意は「この未決定状態で記録する」ことへの合意であり、未確認仮説を requirements 化する許可ではない。

### 8. Ticket を作成または更新する

新規 Ticket の場合:

- `TicketCreate` を使う。
- title / priority / body と、必要な readiness / risk flags を指定する。canonical ID は storage が割り当てる。
- body に readiness / open questions / risk flags と、binding decisions / invariants、implementation latitude、escalation conditions を Markdown で明記する。
- user claims、confirmed facts、unverified hypotheses、undecided points / open questions を分けて書き、未確認 claim を requirements / acceptance criteria として保存しない。

既存 Ticket refinement の場合:

- `TicketComment` を使う。
- role は内容に応じて `comment`, `plan`, `decision` を選ぶ。
- 既存 `item.md` の大幅変更が必要なら、Orchestrator / maintainer に戻す。

### 9. 作成後の報告

ユーザーへ以下を返す。

- 作成/更新した Ticket の system-assigned id / title。
- readiness。
- open questions / risk flags。
- 次に Orchestrator が取るべき routing 候補。
- 未決定点があれば、そのまま明示する。

Intake はここで止まる。implementation / worktree / coder / reviewer 起動は Orchestrator routing の責務である。

## Ticket body の推奨形

```markdown
## User claims / request snapshot

## Confirmed facts / sources

## Unverified hypotheses

## Undecided points / open questions

## Background

## Requirements

## Acceptance criteria

## Binding decisions / invariants

## Implementation latitude

## Readiness

- readiness: implementation_ready | requirements_sync_needed | spike_needed | blocked | unspecified
- risk_flags: [...]

## Escalation conditions

## Validation

## Related work
```

Ticket の body は Markdown/freeform を維持する。すべてを strict schema に押し込まない。

## secret / private context policy

以下は Ticket に保存しない。

- API keys / tokens / credentials。
- secret file contents。
- private prompts / model responses that should not become project records。
- user private context not needed for project history。
- raw logs containing secrets。

必要なら、保存せずに「secret/config が必要」とだけ書く。

## 完了条件

この Workflow の完了条件は次のいずれかである。

- ユーザー合意済みの新規 Ticket が作成され、Orchestrator が routing できる情報が揃っている。
- 既存 Ticket に refinement / decision / plan が記録され、次の routing が明確である。
- Ticket 化しない判断がユーザーに説明され、未決定の問いまたは関連 Ticket が明確になっている。

## 他 Workflow への接続

- `ticket-preflight-workflow`: legacy compatibility planning/requirements sync 入口。新規 routing は standalone preflight ではなく planning return/requirements sync として扱う。
- `multi-agent-workflow`: Orchestrator が implementation_ready と判断した後に接続する。
- `ticket-orchestrator-routing`: この Workflow が作った Ticket を routing する後続 Workflow。
