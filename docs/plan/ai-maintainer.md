# AI maintainer 運用設計

## 位置づけ

この文書は、insomnia を単なる Coding Agent ではなく、開発プロジェクトを継続的に運用する AI maintainer として使うための上位設計である。

`/auto-maintain` はこの設計の最初の実行形であり、TODO / tickets から小さな実装作業を選んで実装・レビューを orchestration する Workflow に留まる。本設計はその一段上で、設計相談、ticket 整理、実装委譲、レビュー、運用課題の記録、改善提案までを一つの maintainer loop として扱う。

ただし、これは unattended 自動開発ではない。最終判断、危険な権限拡大、push、未合意の要件変更は人間に戻す。

また、このフレームワークの対象はプロジェクトの新規立ち上げ・設計ではなく、ある程度タスクの関心対象が分化し、機能追加・課題対応をイテレーションするフェーズにあるプロジェクトである。

## 目的

AI maintainer の目的は、コードを書くことそのものではなく、開発状態を前に進めることである。

具体的には以下を担う。

- TODO / tickets / docs / git history から現在の開発状態を把握する
- 要件が十分に固まった作業を実装可能な単位へ落とす
- 設計判断が必要な作業を実装前に人間へ戻す
- 実装 Pod / reviewer Pod / 親 Pod 自身を使い分ける
- 実装結果を ticket の完了条件に照らして review する
- merge / ticket lifecycle / TODO 整理の順序と branch placement を守る
- 作業中に見つかった運用上の障壁を `docs/report/` に残す
- 繰り返し発生する作業を Workflow / Knowledge / ticket 候補として提案する

## 役割モデル

### Human maintainer

人間は設計上の最終責任者である。

主な責務:

- プロダクト方針、設計方針、優先順位の決定
- AI が提示した選択肢の採否
- scope / permission / history persistence など安全モデルに関わる判断
- push や外部公開に関わる判断
- AI maintainer の運用方針そのものの変更承認

### AI maintainer / orchestrator

親 Pod が担う役割。コードを書くこともできるが、主な責務は制御面である。

主な責務:

- 状態把握: `TODO.md`, `tickets/`, `docs/`, git history, worktree 状態を読む
- 作業選定: 実装可能な ticket と設計相談が必要な ticket を分ける
// ↑ について、設計相談も含めてOrchestrator-Coder間で行わせて良いのでは
- 計画: 作業範囲、検証方法、child worktree / Pod scope を決める
- 委譲: 実装 Pod / reviewer Pod を spawn し、必要最小 scope を渡す
- 監督: Pod 出力、worktree diff、test 結果を確認する
- レビュー: ticket の背景・要件・完了条件に対して実装を確認する
- lifecycle: review artifact、ticket 完了、TODO 更新、merge の branch placement を守る
- 報告: 完了内容、検証結果、未解決事項、次の人間判断をまとめる

### Implementation Pod

狭い write scope を持つ作業者である。実装 Pod は ticket と worktree を渡され、その範囲で実装・検証・報告を行う。

禁止事項:

- `/auto-maintain` など上位 Workflow を実行しない
- ticket / TODO / review artifact / docs/report を勝手に編集しない
- git commit / merge / push をしない
- scope / permission / history persistence を勝手に再設計しない

### Reviewer Pod

原則 read-only の検証者である。実装 Pod とは分けるのが望ましいが、小さい作業や scope 衝突がある場合は親 Pod が review してよい。

主な確認項目:

- ticket の要件を満たしているか
- 既存設計を歪めていないか
- 不要な抽象化や過剰実装がないか
- test / build の結果が妥当か
- ticket lifecycle 上、どの branch に review / completion commit を置くべきか

## 状態と成果物

AI maintainer は、状態を会話内だけに閉じ込めない。継続的な運用に必要な情報は、用途ごとに既存のファイルへ残す。

### `TODO.md`

未完了 ticket の一覧。1 ticket = 1 行。作業選定の入口であり、完了したら feature branch 側で該当行を削除する。

AI maintainer は TODO と ticket の不整合を見つけた場合、勝手に ticket を作成・削除せず、人間に報告する。

### `tickets/`

実装可能な要件単位。作業の完了条件は ticket が基準になる。

AI maintainer は ticket を読むだけでなく、git history 上の作成・更新・削除も参照して前提を把握する。

ただし、`tickets/` は長い設計相談、実装 Pod の報告、review 指摘、修正依頼、lease、artifact を thread として扱うには弱い。長期的には WorkItem / Thread 抽象を導入し、ticket は WorkItem の linked artifact または backend view として扱う。

### WorkItem / Thread

AI maintainer の上位運用単位。`tickets/` より広く、design / feature / refactor / ops / investigation を含む project coordination record として扱う。

WorkItem は Git に依存しない domain model として定義し、初期 backend は repo-managed な `work-items/` または `issues/` directory にできるようにする。正本として残すのは description、acceptance criteria、discussion thread、decision、review、status history、linked branch / worktree / commit、durable artifact metadata である。

`.insomnia` に WorkItem 正本は置かない。`.insomnia/maintainer/` は Pod run、lease cache、polling cursor、temporary inbox、local-only trial log など runtime coordination state の置き場とする。

将来 network 越し workspace / remote maintainer hub を導入する場合も、AI maintainer は file path ではなく `WorkItemStore` / `LeaseStore` 相当の interface を通す。remote backend は後回しにするが、初期 file backend の時点で Git 前提の API に固定しない。

### `tickets/*.review.md`

review の判断根拠。review artifact は対象 feature branch 側に commit する。`develop` の first-parent に review / completion commit を直接置かない。

### `docs/plan/`

長期設計、概念設計、運用設計を置く。実装 ticket より上位の合意形成に使う。

本ファイルは `/auto-maintain` 単体ではなく、AI maintainer 全体の設計を置く場所である。

### `docs/report/`

実際の運用で発生した障壁や改善案を残す。明確な力不足、tool 問題、workflow 問題、ユーザーからの指示があった場合に作る。

report は愚痴ではなく、後から改善 ticket / Workflow 改訂 / Knowledge 化へつなげる観測記録である。

### `.insomnia/workflow/`

実行可能な手順。Workflow は人間が書く、または AI が提案して人間が承認する。AI が勝手に自律生成しない。

### memory / Knowledge

作業中の判断や設計を再利用可能な知識として扱う。ただし、turn を跨ぐ根拠を history に残さず context だけへ差し込むことは禁止する。

## 権限モード

AI maintainer の運用は、作業ごとに権限モードを明示する。

### Mode 0: Consultation

設計相談・方針整理のみ。ファイル編集しない。

使う場面:

- 要件が曖昧
- 複数の設計方針が自然に導ける
- ticket 化する前に合意したい

### Mode 1: Documentation / ticket maintenance

`docs/`, `tickets/`, `TODO.md` など制御面だけを編集する。実装コードは触らない。

使う場面:

- 設計文書作成
- ticket の詳細化
- report 作成
- TODO と ticket の整合確認

注意点:

- 新 ticket 作成や既存 ticket の大幅変更は、人間の合意後に行う
- 完了削除は対象 feature branch 側で行う

### Mode 2: Delegated implementation

child worktree を作り、実装 Pod に狭い write scope を渡して実装させる。親 Pod は orchestration と review を行う。

使う場面:

- 要件と完了条件が明確
- 影響範囲が限定されている
- test / build で確認可能

### Mode 3: Maintainer execution with explicit git authority

人間が明示的に許可した場合に、親 Pod が commit / merge / ticket 完了削除まで行う。

使う場面:

- 「レビューして問題なければマージして」など、明示的な実行指示がある
- 実装 branch / review / ticket lifecycle の配置が明確

制約:

- push は行わない
- unrelated dirty changes を commit に混ぜない
- review / completion commit は feature branch 側に置く
- `develop` first-parent には merge commit だけが載る形を目標にする

## 基本 loop

AI maintainer の基本 loop は以下である。

1. 状態把握
   - `git status --short --branch`
   - `TODO.md`
   - 対象 ticket
   - 関連 docs / report
   - 既存 review artifact

2. 分類
   - 実装可能
   - 設計相談が必要
   - ticket 整理が必要
   - 運用上の問題として report すべき

3. 方針提示または実行
   - 設計判断が必要なら、人間に質問する
   - 実装可能なら、worktree / Pod / scope / test 方針を決める

4. 実装
   - 原則 child worktree
   - 実装 Pod に任せるか、親 Pod が直接行うかを選ぶ
   - `.insomnia` は child worktree から除外する

5. 検証
   - focused test
   - 必要なら broader test
   - `git diff --check`
   - `cargo fmt --check` は既存 unrelated 差分を区別して扱う

6. Review
   - ticket の背景・要件・完了条件に照らす
   - 過剰実装や設計歪みを見る
   - 必要なら修正依頼

7. Lifecycle
   - implementation commit
   - review commit
   - ticket / review / TODO completion commit
   - develop への merge
   - first-parent 確認

8. 報告
   - 実装概要
   - 変更ファイル
   - test 結果
   - review 判断
   - 未解決事項
   - 残った dirty changes

## エスカレーション基準

以下では AI maintainer は実装を止め、人間に戻す。

- ticket の要件から複数の設計方針が自然に導ける
- 長期構造、crate boundary、protocol、permission、scope、history persistence に触れる
- prompt context 加工原則に関わる
- 新 ticket の作成、既存 ticket の大幅変更、ticket 完了削除について合意がない
- git commit / merge / push などの書き込み権限が明示されていない
- test 不能、再現不能、または作業範囲外の不具合に遭遇した
- child worktree に `.insomnia` が出てしまった
- SpawnedPod の完了状態が不明で、worktree 状態からも安全に判断できない

## Orchestration の制約

### SpawnedPod の完了は自動検知しない

現状、親 Pod は SpawnedPod の完了を push 通知として自動検知しない。完了確認は以下で行う。

- `ReadPodOutput` による polling
- Pod が出した完了報告
- `stopped` 状態
- worktree の `git status` / `git diff`
- build / test 結果

そのため、AI maintainer は「Pod が何も言わないが実は完了している」ケースと「Pod が止まっているが失敗した」ケースを区別するため、Pod 出力だけでなく worktree 状態を確認する。

### Scope は所有権として扱う

実装 Pod に write scope を渡している間、親 Pod は同じ path を編集しない。review artifact や ticket lifecycle を書く前に、必要なら `StopPod` して scope を回収する。

### main workspace は制御面

main workspace は orchestration / docs / ticket lifecycle / merge のための場所である。実装差分は原則 child worktree に隔離する。

## Git / ticket lifecycle

feature branch の ticket lifecycle は以下の形を正とする。

```text
*   merge: <topic>                 # develop
|\
| * docs(tickets): complete <topic>
| * review: <topic>
| * feat/fix/refactor: <topic>     # feature branch
|/
* previous develop
```

重要な検査:

- review / completion commit は feature branch 側にある
- `develop` first-parent に review / completion commit が直接載っていない
- unrelated dirty changes が staged / committed されていない
- ticket / review / TODO cleanup は merge 前に feature branch 側で完了している

## `/auto-maintain` との関係

`/auto-maintain` は、この設計の Mode 2 を安全側に制限した Workflow である。

現在の `/auto-maintain` は以下に留める。

- TODO / tickets を読む
- 将来的には WorkItemStore を入口にして、ticket は linked artifact または backend view として扱う
- 低リスク ticket を選ぶ
- worktree / implementation Pod / reviewer を orchestration する
- 完了候補を報告する
- 原則として commit / merge / ticket 完了削除は人間に戻す

一方、この文書が扱う AI maintainer は、ユーザーが明示的に許可した場合に Mode 3 として commit / merge / ticket lifecycle まで実行できる。ただし push はしない。

## 今後必要な機能

この設計を安定運用するには、以下が必要になる。

### WorkItemStore / LeaseStore

`tickets/` file を直接読むだけでなく、AI maintainer が WorkItem / Thread / Event / Artifact を扱うための store interface。初期実装は repo-managed file backend でよいが、network 越し workspace / remote maintainer hub へ差し替えられるよう、Git 操作や path layout を上位 Workflow に漏らさない。

Lease は thread の正本とは分け、`.insomnia/maintainer/` または local DB に置く runtime coordination state として扱う。remote hub を導入する場合は LeaseStore だけを先に集権化できる設計にする。

### Maintainer doctor

merge 前 / ticket 完了前に以下を検査するコマンド。

- current branch
- feature branch と develop の merge base
- review / completion commit の branch placement
- unrelated staged changes
- ticket / TODO の整合
- child worktree に `.insomnia` が含まれていないこと

### Pod completion tracking

SpawnedPod の状態を polling ではなく、親 Pod が扱いやすいイベントとして観測できる仕組み。

必要な情報:

- running / completed / failed / stopped
- 最終 assistant output
- 最終 tool result
- worktree path
- delegated scope

### Operation inbox / trial log

maintainer が見つけた改善候補、試走結果、未整理の気づきを一時的に集める場所。恒久的な ticket にする前の buffer として使う。

### Profile / role manifest

Orchestrator / Coder / Reviewer / Researcher で model・prompt・scope 既定値を分けるための manifest profile。人間が用途ごとに起動時設定を切り替えられるようにする。

### Workflow quality evaluation

Workflow 本文が実際に subagent に渡された時、どこで裁量補完が発生し、どこが曖昧だったかを構造化して report する仕組み。

## 非目標

当面やらないこと:

- 常駐 scheduler による unattended 開発
- AI による push
- AI による無承認の ticket 新規作成 / 大幅変更
- AI による Workflow 自律生成
- scope owner handoff の暗黙化
- history に残らない情報を context へ差し込む運用

## 現時点の方針

AI maintainer は「コードを書く agent」ではなく、「プロジェクト状態を読み、必要な作業単位へ分解し、適切な worker に委譲し、結果を検証し、履歴に残す agent」として扱う。

`/auto-maintain` はその最小実行単位であり、今後はこの文書を上位設計として、Workflow 本文・doctor・profile・Pod completion tracking を段階的に足していく。
