---
description: TODO / tickets / docs / git history から次の作業候補を見繕い、課題発見や方針決定を半自動でイテレーションする WIP maintainer workflow
model_invokation: false
user_invocable: true
requires: []
---
# Auto Maintain Workflow (WIP)

insomnia を AI maintainer として運用するための半自動 loop。TODO / tickets から「今進められそうな作業」を選ぶだけでなく、課題の発見、設計判断の切り分け、次に人間へ戻すべき問いの整理までを扱う。

これは unattended 自動開発ではない。実装の並列委譲は `multi-agent-workflow`、worktree の機械的作成は `worktree-workflow` に任せる。本 Workflow はその前段として、何を進めるべきか、何をまだ決めるべきか、下位 orchestrator にどの intent packet を渡すべきかを整理する。

参照:

- `docs/plan/ai-maintainer.md`
- `tickets/auto-maintain-workflow.md`

## 位置づけ

AI maintainer の目的は、コードを書くこと自体ではなく、プロジェクト状態を前に進めることである。

この Workflow は WIP として、以下を行う。

- TODO / tickets / docs / git history を読んで現在地を把握する。
- 実装可能な ticket と、方針決定が必要な ticket を分ける。
- 小さく実装できる候補を提案する。
- 複数 ticket からなる作業群は、下位 orchestrator に任せる単位として整理する。
- 設計相談が必要な論点を人間に戻す。
- 運用上の問題や繰り返し発生する詰まりを report / ticket / workflow 改訂候補として整理する。

## 非目標

現時点では以下をしない。

- 常駐 scheduler として自動実行する。
- 人間の合意なしに新規 ticket を作る。
- 人間の合意なしに既存 ticket を大幅変更する。
- 人間の合意なしに ticket 完了削除を行う。
- push する。
- Workflow を自律生成・自律改訂する。
- scope / permission / history persistence / prompt context 加工原則に関わる判断を勝手に決める。

## 入力として読むもの

必要に応じて以下を読む。

1. `TODO.md`
2. `tickets/*.md`
3. `docs/plan/`
4. `docs/report/`
5. `git log --oneline` / ticket file の git history
6. 既存 worktree / branch 状態
7. 最近の失敗や通知、ユーザーからの観測

TODO と ticket の不整合を見つけたら、勝手に修正せず、まず報告する。ただしユーザーが明示的に「直して」と言った場合は Mode 1 として整理してよい。

## 分類

候補を以下に分ける。

### A. 実装委譲可能

- 要件と完了条件が具体的。
- 影響範囲が限定的。
- test / build で確認できる。
- 大きな設計判断が不要。
- scope を狭く切れる。

この場合は、人間に候補として提示する。人間が実行を許可したら `$user/multi-agent-workflow` に進む。複数 ticket や連続した作業群では、最上位 Pod が直接 coder を抱えず、下位 orchestrator に intent packet を渡して coder / reviewer sibling loop を管理させる。

### B. 方針決定が必要

- 複数の設計方針が自然に導ける。
- protocol / permission / scope / persistence / prompt context に触れる。
- UX の仕様が未確定。
- 既存 ticket の要件が古い。

この場合は、実装せず、決めるべき問いを短く提示する。

### C. ticket 整理が必要

- TODO にあるが ticket がない。
- ticket があるが TODO にない。
- 完了済みに見えるが残っている。
- ticket の前提が変わっている。

この場合は、不整合と修正案を提示する。修正は人間の許可後に行う。

### D. report / workflow 改善候補

- 同じ tool 問題が繰り返し出る。
- Workflow の指示が曖昧で実装 Pod が迷った。
- coder / reviewer / orchestrator の責務が混ざり、親 Pod が細かい code review に戻ってしまった。
- AI が過剰に Task tool を使うなど、運用上の癖が出た。
- 通知や Pod completion tracking など、開発基盤の不足が観測された。

この場合は、すぐ ticket 化するか、`docs/report/` に観測として残すか、人間に確認する。

## 半自動 iteration

1. 状態把握
   - TODO / tickets / git status を読む。
   - 最近完了した流れや未完了 branch を確認する。

2. 候補抽出
   - 実装可能そうな ticket を 2〜5 件挙げる。
   - correctness / developer experience / user-visible UX / cleanup で分類する。

3. 推奨順位
   - blocking correctness を最優先。
   - 実害が出ている運用問題を次点。
   - 小さく完了できる UX / cleanup を次点。
   - 大きな設計変更は方針相談に回す。

4. 人間への提示
   - 「次に進めるなら X」を1つ推奨する。
   - 理由を短く述べる。
   - 実装委譲する場合の scope / test 方針を添える。
   - 複数 ticket の作業群なら、下位 orchestrator に任せる単位として提示する。

5. 実行への接続
   - 人間が「進めて」と言ったら `$user/multi-agent-workflow` に接続する。
   - 単発 ticket か、下位 orchestrator に任せる ticket 群かを明示する。
   - 下位 orchestrator に渡す intent / requirements / invariants / non-goals / escalation 条件を短くまとめる。
   - worktree 作成は `$user/worktree-workflow` に従う。

## エスカレーション基準

以下では実装に進まず、人間へ戻す。

- ticket の要件から複数の設計方針が自然に導ける。
- 長期構造、crate boundary、protocol、permission、scope、history persistence に触れる。
- prompt context 加工原則に関わる。
- 新 ticket の作成、既存 ticket の大幅変更、ticket 完了削除について合意がない。
- test 不能、再現不能、または作業範囲外の不具合に遭遇した。
- WorkItem / Thread / Lease / maintainer state など、まだ設計中の概念が必要になる。
- 下位 orchestrator に委譲するには intent / invariant / escalation 条件が曖昧すぎる。

## まだ固定しないもの

以下は `docs/plan/ai-maintainer.md` の上位設計に残し、本 Workflow では詳細を固定しない。

- WorkItemStore / LeaseStore。
- operation inbox / trial log。
- QA feedback を ticket / review / report のどれに落とすか。
- AI 自身の feedback を Knowledge / report / ticket / workflow 改訂のどれにするか。
- maintainer doctor。
- reviewer Pod の評価基準の機械化。
