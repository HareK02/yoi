---
description: ユーザーの曖昧な依頼を要件同期し、合意済みの Ticket として作成・更新する Intake workflow
model_invokation: true
user_invocable: true
requires: []
---
# Ticket Intake Workflow

Yoi の multi-agent 運用で、ユーザーの依頼をいきなり実装委譲せず、まず **合意済み Ticket** に変換するための Workflow。

Intake の目的は、ユーザーの意図・要件・受け入れ条件・未決定点を明確にし、Orchestrator が次の routing を判断できる Ticket を作ることである。Intake は scheduler ではなく、coder / reviewer Pod を起動しない。

## 位置づけ

```text
User request / conversation
  -> Ticket Intake Workflow
  -> TicketCreate / TicketComment
  -> Orchestrator routing
  -> preflight / spike / implementation / review / blocked / close
```

- `Ticket` は durable orchestration record。
- `Task` は session-local progress tracking。
- `Assignment` は Orchestrator から coder / reviewer / investigator Pod への具体的委譲。
- `IntentPacket` は Ticket から抽出して Assignment に渡す短い実装・レビュー契約。

## Intake の責務

Intake は以下を行う。

- ユーザー依頼の主語と目的を確認する。
- 既存 Ticket を確認し、duplicate / related work を探す。
- 必要に応じて関連 docs / code / workflow / history を読む。
- 不足している要件を質問する。
- Ticket の title / slug / kind / priority / labels を提案する。
- background / requirements / acceptance criteria / non-goals / escalation conditions を整理する。
- readiness / needs_preflight / risk flags を明示する。
- ユーザー合意後に Ticket を作成する。
- 既存 Ticket の refinement を求められた場合は、TicketComment で経緯を残す。

## Intake がしないこと

- coder / reviewer / investigator Pod を起動しない。
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
- やらないことは何か。
- 後方互換が必要か。
- authority boundary / scope / permission / history / prompt context に触れるか。
- validation は何で確認できるか。
- 人間判断が必要な論点は何か。

不足がある場合は、Ticket 作成前に質問する。質問は多すぎず、Ticket 作成に必要な最小限に絞る。

### 4. readiness を分類する

Ticket 作成または更新前に、readiness を明示する。

```text
implementation_ready:
- 要件・受け入れ条件・non-goals / invariants が明確。
- 実装方針が一意または十分絞れている。
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

### 5. needs_preflight / risk flags を付ける

以下に触れる Ticket は `needs_preflight: true` 相当として扱い、Ticket body に明記する。

- profile / manifest / scope / permission。
- session / history / Pod metadata / persistence。
- prompt context 加工原則。
- public API / plugin / feature boundary。
- security / secret / credential handling。
- storage migration / backward compatibility。
- 複数の自然な設計方針があるもの。
- reviewer が diff だけでは見落としやすい設計リスク。

risk flags は短い語でよい。

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
Needs preflight:
Risk flags:

Background:

Requirements:

Acceptance criteria:

Non-goals:

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
- body に readiness / needs_preflight / risk flags を Markdown で明記する。

既存 Ticket refinement の場合:

- `TicketComment` を使う。
- role は内容に応じて `comment`, `plan`, `decision` を選ぶ。
- 既存 `item.md` の大幅変更が必要なら、Orchestrator / maintainer に戻す。

### 9. 作成後の報告

ユーザーへ以下を返す。

- 作成/更新した Ticket id / slug / title。
- readiness。
- needs_preflight / risk flags。
- 次に Orchestrator が取るべき routing 候補。
- 未決定点があれば、そのまま明示する。

Intake はここで止まる。implementation / worktree / coder / reviewer 起動は Orchestrator routing の責務である。

## Ticket body の推奨形

```markdown
## Background

## Requirements

## Acceptance criteria

## Non-goals

## Readiness

- readiness: implementation_ready | requirements_sync_needed | spike_needed | blocked | unspecified
- needs_preflight: true | false
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

- `ticket-preflight-workflow`: needs_preflight が true、または implementation_ready か不安な場合に接続する。
- `multi-agent-workflow`: Orchestrator が implementation_ready と判断した後に接続する。
- `ticket-orchestrator-routing`: この Workflow が作った Ticket を routing する後続 Workflow。
