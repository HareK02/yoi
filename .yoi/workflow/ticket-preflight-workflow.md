---
description: ticket を実装委譲する前に、要件・前提・設計境界・反証観点を同期し、Ticket thread に記録する preflight フロー
model_invokation: true
user_invocable: true
requires: []
---
# Ticket Preflight Workflow

yoi プロジェクトで ticket を実装に渡す前に、要件・前提・設計境界・反証観点を同期するための Workflow。これは **実装前の gate** であり、worktree 作成や coder / reviewer Pod の起動は `multi-agent-workflow` / `worktree-workflow` 側で扱う。

目的は「ticket があるから実装する」状態を避け、ticket が **実装可能な仕様** なのか、**調査 ticket** なのか、**人間との仕様同期が必要な未決定 ticket** なのかを明確にすることである。

## 適用する場面

以下のいずれかに当てはまる ticket は、実装委譲前にこの Workflow を通す。

- profile / manifest / scope / permission / session / history / pod-store / prompt context など authority boundary に触れる。
- ticket の文面から複数の自然な設計方針が導ける。
- 「どう実装するか」以前に「何を仕様とするか」が曖昧である。
- 既存 implementation plan があるが、抽象化・責務境界・ユーザー体験に疑問がある。
- 過去 decision / memory / docs / ticket thread と矛盾しそうである。
- coder Pod に渡す intent packet を短く書けない。

小さなバグ修正や仕様が明確な局所変更では、この Workflow は省略してよい。ただし省略理由が曖昧な場合は preflight する。

## Ticket 記録方針

作業管理の authority は `.yoi/tickets/` に保存される Ticket と git history である。preflight の結果は、口頭の会話だけで終わらせず、Ticket tool または `yoi ticket ...` で ticket の `thread.md` または `item.md` に残す。

- 新規の前提・要件・受け入れ条件は、必要に応じて `item.md` を更新する。
- 調査結果・実装前 plan は `TicketComment` または `yoi ticket comment <ticket> --role plan --file <file>` で残す。
- 採用/却下した設計判断、実装停止判断、仕様同期の結論は `--role decision` で残す。
- 実装に入ってよい状態になったら、その根拠を intent packet として ticket thread に残す。
- 仕様が未決定なら、実装 ticket にせず requirements-sync / spike / design ticket として切り分ける。
- ticket の timestamp/frontmatter が更新される場合は、関連変更と一緒に commit する。
- ticket 作成・更新・レビュー・完了は git commit で記録する。push はしない。

## 手順

### 1. 状態確認

- `git status --short --branch`
- 対象 ticket の `item.md` / `thread.md` / artifacts
- 関連 ticket / docs / workflow / Knowledge / memory decision
- 既存 worktree / branch / running Pod の有無

この段階で unrelated dirty changes がある場合は、preflight の記録だけを行うか、人間に確認してから進める。

### 2. ticket の種類を分類する

以下のどれかに分類する。

```text
implementation-ready:
- 要件・受け入れ条件・invariant が明確で、実装方針が一意または十分絞れている。

requirements-sync-needed:
- ticket の目的は見えているが、仕様・用語・責務境界・ユーザー体験の同期が必要。

spike-needed:
- 技術的実現性・依存関係・ライセンス・性能・diagnostics などを先に調べる必要がある。

blocked-needs-human-decision:
- 複数方針があり、AI が勝手に決めると設計境界や product API を固定してしまう。
```

`implementation-ready` 以外は、coder Pod に実装を委譲しない。

### 3. 要件同期

最低限、以下を確認する。

- 完了時に observable に何が変わるか。
- ticket の主語は何か: user-facing behavior / internal architecture / cleanup / investigation。
- 用語が既存設計と一致しているか。
- 何をやらないか。
- 後方互換が必要か、不要な互換層を作ろうとしていないか。
- 既存の authority boundary を変えるか。
- runtime state / persisted state / config / profile / manifest / session log / pod metadata のどれが authority か。

必要なら ticket `item.md` の Background / Requirements / Acceptance criteria を更新する。

### 4. 現行コードと過去判断の map を作る

実装前に、少なくとも関連する current code paths を列挙する。

```text
Current code map:
- file/function: 現在の責務
- file/function: 変更候補
- file/function: 触ってはいけない境界
```

この map は簡潔でよい。目的は coder Pod が blind に探しながら設計を固定するのを防ぐこと。

### 5. 批判的 preflight

実装方針を一度疑う。以下の問いに答える。

- この ticket は本当に実装 ticket か、それとも仕様同期 ticket か。
- 最も自然に見える実装が失敗するとしたらどこか。
- 抽象化に失敗して「別名の同じもの」を作っていないか。
- runtime-bound な値を reusable config に混ぜていないか。
- profile / manifest / scope / session / pod-store などの authority を逆転させていないか。
- user-facing API を安易に public contract 化していないか。
- external dependency / license / portability / packaging の問題はないか。
- reviewer が diff だけ読んでも見落とす設計リスクは何か。

この問いへの答えを `plan` または `decision` として ticket thread に残す。

### 6. intent packet を作る

実装に入ってよい場合だけ、`multi-agent-workflow` に渡す intent packet を作る。

```text
Intent:
- 何を実現するか。

Requirements:
- observable な完了条件。

Invariants:
- 壊してはいけない authority boundary / design boundary。

Non-goals:
- 今回やらないこと。

Escalate if:
- 親/人間に戻す判断条件。

Validation:
- focused test / broader check / doctor / docs 更新。

Current code map:
- 実装対象と触ってはいけない場所。

Critical risks:
- reviewer にも見てほしい失敗パターン。
```

この intent packet が短く書けない場合は、実装委譲せず requirements-sync-needed とする。

## review への引き継ぎ

preflight で出た critical risks は reviewer Pod にも渡す。reviewer は diff だけでなく、ticket の前提・要件・invariant と preflight の反証観点を読む。

reviewer に期待すること:

- 実装が preflight の intent に対応しているか。
- 抽象化失敗や authority boundary 違反がないか。
- preflight で挙げた失敗パターンに落ちていないか。
- validation がリスクに対して十分か。

## 完了条件

この Workflow 自体の完了条件は、次のいずれかである。

- ticket が `implementation-ready` になり、intent packet が thread に記録されている。
- ticket が `requirements-sync-needed` / `spike-needed` / `blocked-needs-human-decision` として整理され、次に人間へ戻す問いまたは follow-up ticket が明確になっている。
- ticket 自体が不要/誤りと判断され、理由が decision として記録されている。

## この Workflow でしないこと

- worktree を作成しない。
- coder Pod に実装を委譲しない。
- merge / close しない。
- 仕様未決定のまま「小さく実装してみる」ことで public API を固定しない。
