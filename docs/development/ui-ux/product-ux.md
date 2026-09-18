# Workspace Web Product UX

この文書は、Workspace Webで扱うresource、navigationのinformation
architecture、resourceごとの主要taskを定義する。見せ方の共通文法は[`design-language.md`](design-language.md)、application
shellとSidebar
slotは[`application-architecture.md`](application-architecture.md)を参照する。

## Product position

Workspace
Webは、Workspace内の権威ある状態を確認し、注意が必要な箇所を判断し、一度に一つの操作を行うための作業面である。Marketing
dashboard、analytics画面、Backend DTOのraw viewerにはしない。

基本的な操作の流れは次とする。

```text
scopeを選ぶ
→ resourceを選ぶ
→ current stateを確認する
→ 次の有効なactionを行う
→ 結果を確認する
```

## Resource

主要resourceは次のとおり。

- Workspace
- Ticket
- Objective
- Merge Request
- Memory
- Worker
- Workdir
- Runtime
- Repository

通常の表示とURLにはcanonical resource keyを使う。内部UUIDをlabelやfallback
linkとして露出しない。

## Navigation IA

### Global

Workspaceを選ぶ前のscopeを扱う。

- Workspace catalog
- Account
- Device Login
- Workspace作成

### Workspace

一つのWorkspace内で日常的に扱うresourceを置く。

1. Tickets
2. Objectives
3. Merge Requests
4. Memory
   - Document
   - Staging
5. Workers

### Settings

Workspaceの管理と構成を扱う。

- Runtimes
- Configuration Sources
- Repositories
- Repository Access
- Profile Sources
- Workspace Identity

`Runtimes`、`Repositories`、`Repository Access`をWorkspace primary
navigationへ混ぜない。permissionにより利用できないSettings itemは表示しない。

## Page model

各routeは、主要な目的を一つだけ持つ。その目的は「Ticketを選ぶ」「このWorkerを確認する」「Repository
accessを設定する」のように、動詞と対象を一つずつ使って表現できなければならない。

Workspace pageは、次のいずれかを基本形とする。

1. **一覧またはboard** — resourceを探し、注意が必要な項目を見つける。
2. **詳細** — 一つのresourceを理解し、現在実行できる次の操作を行う。
3. **formまたはeditor** — 範囲の明確な設定変更を行う。
4. **Console** — 一つのWorkerを操作しながら、履歴とcurrent controlを確認する。

同じ目的を支える場合に限り、一つのrouteで複数の基本形を組み合わせてよい。

## Product pattern

### Tickets

- boardを一つの横方向の作業面として扱う。
- laneはworkflow stateを表し、隣接laneとの関係を維持する。
- Ticketはlane内でDesign Languageの`Resource row` patternを使う。
- Ticket key、title、優先判断を変えるmetadataだけを表示する。
- blockerとattentionはtextでも示す。
- narrow viewportでは、bounded horizontal scrollでlane比較を維持する。

### Resource index

- 同じfieldをresource間で比較する場合、flat listまたはtableを使う。
- 人が読むnameとcanonical keyを最初のcolumnに置く。
- columnは判断に必要な値だけに絞る。
- provider detail、revision、digestはdetailまたはtechnical disclosureへ移す。

### Resource detail

- identity、current state、attentionを最初に置く。
- narrativeまたはhistoryをprimary領域に置く。
- relation、target、review、workflowが別のauthorityを持つ場合、それぞれを独立したsectionにする。
- raw metadataはtechnical disclosureへ置く。

### Settingsとform

- 一つのSettings routeは一つの設定関心事を所有する。
- edit actionの前または同時に、現在有効な値を示す。
- 一つの判断に必要なfieldをgroup化する。
- narrow viewportでは一列にする。
- form内でfilled actionにするのはsave actionだけとする。

### Worker Console

- transcriptをprimary surfaceとする。
- direct SubWorkerを表示している場合も、Composerとrun
  controlは親Workerへbindしたままにする。
- OverviewとNormalはpresentationだけを変え、authorityを変えない。
- command outputはterminalに近いcompactな表示とし、prose messageから分離する。
- live stateは権威あるWorker snapshotから導出する。
- connection stateやtransient eventでcurrent stateを置き換えない。
- hidden reasoningとraw system promptはdebug modeでも表示しない。

## Stateとpermission

- 存在しない、閲覧できない、読み込みに失敗したWorkspaceを別の状態として扱う。
- 別のWorkspaceへ暗黙に切り替えない。
- owner-only mutation controlをnon-ownerへ表示しない。
- read-onlyの場合は、有用なread viewを残す。
- Workerのdomain stateとWebSocket connection stateを混同しない。
- Mergeなど不可逆または重要なoperationは、対象と結果を明示してから実行する。
