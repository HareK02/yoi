---
description: ユーザーの曖昧な依頼を要件同期し、合意済みの Ticket として作成・更新する Intake workflow
model_invokation: true
user_invocable: true
requires: []
---
# Ticket Intake Workflow

Yoi の multi-agent 運用で、ユーザーの依頼をいきなり実装委譲せず、まず **合意済み Ticket** に変換するための Workflow。

Intake の目的は、ユーザーの意図・要件・制約・受け入れ条件・未決定点を明確にし、Orchestrator が次の routing を判断できる Ticket を作ることである。Intake は scheduler ではなく、coder / reviewer Pod を起動しない。

## 位置づけ

```text
User request / conversation
  -> Ticket Intake Workflow
  -> TicketCreate / TicketComment
  -> Orchestrator routing
  -> planning sync / spike / implementation / review / blocked / close
```

- `Ticket` は durable orchestration record。
- `Task` は session-local progress tracking。
- `Assignment` は Orchestrator から coder / reviewer Pod、または task-specific helper Pod への具体的委譲。
- `IntentPacket` は Ticket から抽出して Assignment に渡す短い実装・レビュー契約。

Intake は、要件同期と Ticket 化を担当する。実装の起動・worktree 作成・review 委譲・merge 判断は Orchestrator 側の責務である。`ready` は Orchestrator が routing できる状態を意味し、実装戦術がすべて事前固定されていることを意味しない。

Ticket に残す内容は、binding decisions / invariants、implementation latitude、escalation conditions を区別する。Coder が調査しながら局所的な実装手段を選べる余地は残してよいが、product / API / UX / authority boundary / explicit design constraint を silently 決める余地を残してはならない。

## Intake の責務

Intake は以下を行う。

- ユーザー依頼の主語と目的を確認する。
- 既存 Ticket を確認し、duplicate / related work を探す。
- 必要に応じて関連 docs / code / workflow / history を読む。
- 不足している要件を質問する。
- Ticket の title / slug / kind / priority / labels を提案する。
- background / requirements / acceptance criteria / escalation conditions を整理する。
- binding decisions / invariants と implementation latitude を分けて書く。
- 具体的な除外や触れてはいけない境界が binding decision である場合は、generic な除外リストではなく invariant / escalation condition として明記する。
- readiness / open questions / risk flags を明示する。
- ユーザー合意後に Ticket を作成する。
- 既存 Ticket の refinement を求められた場合は、TicketComment で経緯を残す。

## Intake がしないこと

- coder / reviewer Pod や read-only investigation helper Pod を起動しない。
- worktree を作らない。
- merge / close / branch cleanup をしない。
- implementation-ready でない Ticket を実装に投げない。
- unattended scheduler として自動実行しない。
- ユーザー合意なしに official Ticket を作らない。
- secrets / private context を Ticket body / thread / artifacts / diagnostics に保存しない。
- arbitrary filesystem write で `work-items/` を編集しない。Ticket 操作は Ticket tools を使う。

## 使用する Ticket tools

利用可能なら、以下の typed Ticket tools を使う。

- `TicketList`: 既存 Ticket の一覧・重複確認。
- `TicketShow`: 関連 Ticket の詳細確認。
- `TicketCreate`: 合意済み Ticket の作成。
- `TicketComment`: 既存 Ticket refinement / decision / plan の記録。
- `TicketDoctor`: 必要に応じた整合性確認。

Intake は `TicketReview`, `TicketStatus`, `TicketClose` を通常使わない。review / status transition / close は Orchestrator または reviewer / maintainer workflow の責務である。

Ticket tools が利用できない環境では、勝手に file write で代替しない。ユーザーまたは Orchestrator に「Ticket tools がないため materialize できない」と報告し、必要なら `yoi ticket` を使える人間/親 workflow に戻す。

## 手順

### 1. 依頼を受け取る

ユーザー依頼を短く言い換え、以下を分ける。

- 何を変えたいか。
- なぜ必要か。
- 影響を受けるユーザー / Pod / workflow / crate / docs。
- 既に決まっていること。
- まだ未決定のこと。

この段階では Ticket を作らない。

### 2. 既存 Ticket を確認する

`TicketList` / `TicketShow` で duplicate / related work を探す。

確認観点:

- 同じ目的の open / pending Ticket がないか。
- closed Ticket の判断・resolution と矛盾しないか。
- umbrella Ticket / follow-up Ticket が既にあるか。
- 既存 Ticket の refinement で足りるか、新規 Ticket が必要か。

既存 Ticket の更新で足りる場合、新規 Ticket を作らず、ユーザーに更新案を提示する。

### 3. 要件を同期する

最低限、以下を確認する。

- observable な完了条件は何か。
- Ticket の種類は何か: feature / bug / cleanup / design / spike / workflow / docs / release / orchestration。
- 受け入れ条件は何か。
- binding decision として残す具体的な除外・authority boundary はあるか。
- 後方互換が必要か。
- authority boundary / scope / permission / history / prompt context に触れるか。
- validation は何で確認できるか。
- 人間判断が必要な論点は何か。

不足がある場合は、Ticket 作成前に質問する。質問は多すぎず、Ticket 作成に必要な最小限に絞る。

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

spike_needed:
- 技術調査、依存関係、性能、license、diagnostics、現在コード map が先に必要。

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

Kind:
Priority:
Labels:
Readiness:
Needs planning sync:
Risk flags:

Background:

Requirements:

Acceptance criteria:

Binding decisions / invariants:

Implementation latitude:

Escalation conditions:

Validation:

Related tickets/docs:
```

この時点ではまだ Ticket を作らない。

### 7. ユーザー合意を取る

以下のどちらかがあるまで official Ticket を作らない。

- ユーザーが draft を明示的に承認する。
- ユーザーが「作って」「切って」「記録して」など、作成を明示する。

未決定のまま記録する場合は、`requirements_sync_needed` / `spike_needed` / `blocked` として未決定点を明示する。

### 8. Ticket を作成または更新する

新規 Ticket の場合:

- `TicketCreate` を使う。
- title / slug / kind / priority / labels / body を指定する。
- body に readiness / open questions / risk flags と、binding decisions / invariants、implementation latitude、escalation conditions を Markdown で明記する。

既存 Ticket refinement の場合:

- `TicketComment` を使う。
- role は内容に応じて `comment`, `plan`, `decision` を選ぶ。
- 既存 `item.md` の大幅変更が必要なら、Orchestrator / maintainer に戻す。

### 9. 作成後の報告

ユーザーへ以下を返す。

- 作成/更新した Ticket id / slug / title。
- readiness。
- open questions / risk flags。
- 次に Orchestrator が取るべき routing 候補。
- 未決定点があれば、そのまま明示する。

Intake はここで止まる。implementation / worktree / coder / reviewer 起動は Orchestrator routing の責務である。

## Ticket body の推奨形

```markdown
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

- `ticket-preflight-workflow`: legacy compatibility slug の planning/requirements sync 入口。新規 routing は standalone preflight ではなく planning return/requirements sync として扱う。
- `multi-agent-workflow`: Orchestrator が implementation_ready と判断した後に接続する。
- `ticket-orchestrator-routing`: この Workflow が作った Ticket を routing する後続 Workflow。
