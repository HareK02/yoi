---
description: worktree と sibling の coder / reviewer Pod を使い、下位 orchestrator が複数 ticket の実装・外部レビュー・修正・完了準備を管理する orchestration フロー
model_invokation: true
user_invocable: true
requires: []
---
# Multi-agent Worktree Workflow

yoi を yoi で開発する際の、worktree + coder Pod + 外部 reviewer Pod + orchestrator Pod の標準フロー。これは **最上位 Pod が細かい code review を抱えず、下位 orchestrator が実装と外部レビューの loop を完了状態まで運ぶためのフロー** である。

worktree の機械的作成手順は `$user/worktree-workflow`、ユーザー依頼の Ticket 化は `$user/ticket-intake-workflow`、Ticket の next action 分類は `$user/ticket-orchestrator-routing`、実装前の要件同期・反証 preflight は `$user/ticket-preflight-workflow` に分ける。

この Workflow は、対象 ticket が implementation-ready であることを前提にする。設計境界・仕様・authority boundary が未同期の場合は、worktree 作成や coder Pod 起動の前に `ticket-preflight-workflow` を通す。

## 目的

- 実装差分を ticket ごとの child worktree に隔離する。
- coder Pod に narrow write scope を渡して実装させる。
- reviewer Pod を coder の子ではなく **同じ orchestrator 配下の sibling** として立て、外部レビューを行わせる。
- orchestrator は coder / reviewer のやり取り、修正 loop、validation、merge-ready dossier 作成に責任を持つ。
- 最上位 orchestrator は、コードを直接理解し切ることではなく、委譲した intent / 要件 / invariant に沿って下位 orchestrator が完了まで運んだかを acceptance する。

## 階層モデル

基本形は以下。

```text
最上位 orchestrator Pod
  - 人間との会話相手
  - intent / 要件 / invariant / escalation 条件を定義
  - 複数の作業群を並列管理
  - final merge / ticket close / main workspace validation を行う
  - 原則として line-by-line code review を主業務にしない

下位 orchestrator Pod（area / epic / ticket-group coordinator）
  - 連続した複数 ticket または大きめの ticket 群を完了状態まで運ぶ
  - worktree / branch / coder / reviewer / validation / 修正 loop を管理する
  - coder と reviewer を sibling として扱う
  - 親には merge-ready dossier と残論点だけを返す

coder Pod
  - 指定 worktree / branch に実装する
  - ticket 外判断や設計衝突は orchestrator に戻す
  - reviewer に直接反論・修正依頼を完結させず、orchestrator に報告する

reviewer Pod
  - 原則 read-only
  - ticket / intent packet / diff / validation 結果を読む
  - 実際のコード変更が概念的に何を変えたかを説明する
  - intent / 要件 / invariant に反する blocker を分類して返す
```

一段だけで足りる小さい ticket では、最上位 orchestrator が直接 coder / reviewer sibling を扱ってよい。複数 ticket や設計境界をまたぐ作業では、最上位の下に下位 orchestrator を挟む。

## 開始条件

以下が揃っている時に使う。

- 対象 ticket または ticket 群が決まっている。
- ticket の背景・要件・完了条件から実装方針が概ね導ける。
- worktree 作成と git 書き込み操作について、人間の許可がある。
- main workspace の unrelated dirty changes を把握している。
- 下位 orchestrator に渡す intent / invariant / non-goals / escalation 条件を短く書ける。
- 設計境界・仕様・authority boundary に不確定要素がある場合、`ticket-preflight-workflow` の結果が ticket thread に記録されている。

設計方針が複数自然に導ける場合、protocol / scope / permission / history persistence に触れる場合、ticket 自体の再定義が必要な場合は、実装委譲前に `ticket-preflight-workflow` を通し、必要なら人間へ戻す。ただし下位 orchestrator に探索だけを委譲することはできる。

## Intent packet

階層化すると人間の意図が劣化しやすい。下位 orchestrator や coder / reviewer へは、自然文の依頼だけでなく intent packet を渡す。

標準形:

```text
Intent:
- 何を実現するか。

Requirements:
- 完了時に満たすべき observable な要件。

Invariants:
- 壊してはいけない設計境界。
- 残してはいけない旧概念や互換層。

Non-goals:
- 今回やらないこと。

Escalate if:
- 親へ戻すべき判断条件。

Validation:
- 実行すべき format / build / test / doctor。
```

reviewer には coder の実装方針ではなく、この intent packet と diff を中心に読ませる。

## orchestrator の責務

下位 orchestrator を挟まない場合は、以下を最上位 orchestrator が行う。下位 orchestrator を挟む場合は、最上位は intent packet を渡し、以下の実務を下位に委譲する。

1. 状態確認
   - `git status --short --branch`
   - 対象 ticket / ticket 群
   - 関連 TODO / docs / 既存 worktree
   - preflight が必要な ticket では、`ticket-preflight-workflow` の分類・要件同期・critical risks

2. worktree 作成
   - `$user/worktree-workflow` に従い `./.worktree/<task-name>` を作る。
   - `.yoi` を sparse checkout で除外する。

3. coder Pod spawn
   - read scope: main workspace 全体。
   - write scope: child worktree、または必要最小 directory。
   - task には以下を明示する。
     - child worktree path / branch
     - 対象 ticket path
     - intent packet
     - Bash は必ず child worktree に `cd` すること
     - main workspace の `TODO.md` / `tickets/` / `docs/report/` / `.yoi` は編集しないこと
     - 範囲外事項
     - 実行すべき build / test / format
     - 完了報告項目

4. coder 完了確認
   - `ReadPodOutput` で報告を読む。
   - 通知が来ない場合でも、worktree の `git status` / `git diff` / test で完了状態を確認する。
   - coder が止まった場合、worktree 状態を見て再 spawn / rollback / 親 escalation を判断する。

5. reviewer Pod spawn
   - reviewer は coder の子ではなく orchestrator 配下の sibling として立てる。
   - 原則 read-only scope にする。
   - reviewer に渡すもの:
     - ticket / intent packet
     - branch / commit / diff の読み方
     - 実行済み validation
     - blocker / non-blocker / acceptable の分類基準
   - reviewer の主目的は「コードの詳細を親の代わりに説明し、intent / invariant 違反を見つけること」。

6. review → 修正 loop
   - reviewer finding を orchestrator が読む。
   - blocker は coder に修正依頼する。
   - reviewer の指摘を却下する場合は、orchestrator が理由を dossier に残す。
   - 修正後は focused validation を実行し、必要なら reviewer に再確認させる。
   - reviewer の blocker が未解決のまま親に提出しない。

7. merge-ready dossier 作成
   - 親がコードを直接理解しなくても判断できるよう、変更の概念的説明と evidence をまとめる。

8. merge / lifecycle
   - 最上位 orchestrator または人間の許可を持つ orchestrator が main workspace へ merge する。
   - `TODO.md` から該当行を削除し、ticket を完了処理して commit する。
   - main workspace で必要な test / `cargo check --workspace` / `cargo fmt --check` を再実行する。

## coder Pod の責務

- child worktree 内でのみ実装する。
- main workspace の管理ファイルを書かない。
- intent / requirements / invariants / non-goals を読んでから実装する。
- 指定された build / test / format を実行する。
- ticket 要件外の設計変更、依存関係追加、scope / permission / history persistence / prompt context 加工原則に触れる変更が必要なら止めて orchestrator に報告する。
- 完了時に以下を報告する。
  - worktree path / branch
  - commit hash（commit した場合）
  - 変更ファイル
  - 実装概要
  - 実行した build / test / format
  - 未解決事項
  - review に回せるか

## reviewer Pod の責務

reviewer は coder の subordinate ではない。orchestrator 配下の sibling として、実装 diff を外部から読む。

- 原則 read-only で作業する。
- ticket / intent packet / diff / validation 結果を読む。
- 実際のコード変更が概念的に何を変えたかを説明する。
- 親や上位 orchestrator が line-by-line diff を読まずに判断できるよう、以下を整理する。
  - 変更の概念モデル
  - intent / requirements との対応
  - invariant 違反の有無
  - 旧概念・禁止語彙・不要な互換層の残存
  - validation の妥当性
  - blocker / non-blocker / follow-up
- reviewer は直接 merge しない。
- reviewer は coder に直接作業指示を出さず、orchestrator に finding を返す。

## commit 方針

coder Pod には child worktree 内での commit を許可してよい。

- commit は ticket 内で意味のある粒度にする。
  - 例: `feat: ...`、`fix: ...`、`test: ...`、`docs: ...`
- coder Pod は merge / push / branch deletion / worktree remove をしない。
- coder Pod は `TODO.md` / ticket の完了処理 commit をしない。
- orchestrator は review 時に commit 粒度も確認する。
- 必要な修正は、原則追加 commit として積む。履歴改変や squash は人間の明示指示がある時だけ行う。

## Review → 修正 → 完了の標準形

### Approve

1. coder Pod / reviewer Pod を停止し、scope を回収する。
2. orchestrator が merge-ready dossier を確認する。
3. 最上位 orchestrator が必要最小限の spot check を行う。
4. main workspace で `git merge --no-ff <branch>` する。
5. `TODO.md` と ticket を完了処理して commit する。
6. main workspace で検証コマンドを再実行する。
7. 変更内容・commit・検証結果・残 dirty changes を報告する。

### Request changes

1. reviewer finding または orchestrator finding を blocker / non-blocker に分ける。
2. blocker はファイル / 行 / 理由 / 修正方針つきで coder に戻す。
3. coder が停止済みなら、同じ worktree / branch / scope で再 spawn する。
4. 修正後に focused test と必要な broader test を再実行する。
5. 必要なら reviewer に再確認させる。
6. blocker が解消したら dossier を更新する。

### Non-blocking comments

- ticket 要件外の改善はその場で混ぜない。
- 必要なら後続 ticket / docs/report にする。
- non-blocking を理由に completion を遅らせない。

## 並列実装時の注意

- 1 ticket = 1 worktree = 1 branch を基本にする。
- 複数 Pod に同じ write scope を渡さない。
- parent / orchestrator は coder の write scope 配下を直接編集しない。
- reviewer は read-only を基本にする。review artifact を書かせる場合は ticket artifacts など限定 scope にする。
- 依存関係がある ticket は、土台 branch を merge してから次 worktree を切る。
- parallel に走らせた Pod の完了通知は取りこぼしうるため、`ReadPodOutput` と worktree 状態で確認する。

## merge-ready dossier の標準形

```text
Status:
- completed / blocked / needs parent decision

Scope:
- ticket(s): <path>
- branch: <name>
- commits:
  - <hash> <subject>

Intent check:
- invariant 1: ok / violated / not applicable, evidence ...
- invariant 2: ok / violated / not applicable, evidence ...

Implementation summary:
- 変更の概念的説明: ...
- 主要変更ファイル: ...
- compatibility / migration: ...

Coder loop:
- coder pod: <name>
- produced commits: ...
- unresolved coder notes: ...

External review:
- reviewer pod: <name>
- blockers found: ...
- blockers fixed: ...
- rejected findings and reasons: ...
- non-blocking follow-ups: ...

Validation:
- cargo fmt --check
- cargo check --workspace
- cargo test ...
- ./tickets.sh doctor

Parent decision needed:
- none / specific question

Residual risk:
- ...

Dirty state:
- ...
```

## 最上位 orchestrator の acceptance

最上位 orchestrator は、下位 orchestrator の成果を code review するのではなく acceptance する。

確認するもの:

- intent packet が保持されているか。
- reviewer が coder と独立した sibling として機能したか。
- blocker が未解決のまま握りつぶされていないか。
- 変更の概念的説明が要件と対応しているか。
- validation が ticket のリスクに対して十分か。
- escalation すべき判断を下位が勝手に決めていないか。

必要なら spot check するが、常態的な line-by-line review に戻らない。

## この Workflow で扱わないもの

以下は `$user/ticket-intake-workflow`、`$user/ticket-orchestrator-routing`、または別の設計相談で扱う。

- ユーザー依頼を Ticket 化すること。
- Ticket の next action を分類すること。
- QA feedback / AI feedback を Ticket / report / workflow に落とす判断。
- 長期 maintainer loop / scheduler / LeaseStore の設計。
- reviewer Pod の品質評価を機械的に採点する仕組み。
