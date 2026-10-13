# Workspace WebのUX設計

Workspace Webの画面構成と、リソースごとの表示・操作を決める規則を定める。視覚表現と操作の共通規則は[Design Language](design-language.md)に従い、この文書では製品固有の判断を扱う。

## 作業面の目的

Workspace Webは、Workspaceの確定した状態を確認し、注意が必要な箇所を判断し、次の操作を行うための作業面とする。マーケティング用のdashboard、分析画面、Backend DTOをそのまま並べる画面にはしない。

操作の流れは、対象のWorkspaceとリソースを選び、現在の状態を確認し、操作を行い、その結果を確認する順とする。通常の表示とURLには各リソースのcanonical keyを使い、内部UUIDをlabelや代替リンクとして露出しない。

## ナビゲーションの区分

- Globalでは、Workspaceの選択・作成、Account、Device Loginなど、Workspaceを選ぶ前の操作を扱う。
- Workspaceでは、Tickets、Objectives、Merge Requests、Drive、Memory、Workersなど、選択中のWorkspaceで日常的に扱う対象へ移動する。
- Settingsでは、Runtime、Repositoryとそのアクセス設定、Configuration Sources、Profile Sources、Workspace Identityなど、Workspaceの管理と構成を扱う。これらを日常作業のナビゲーションへ混ぜない。

存在しないWorkspace、閲覧できないWorkspace、読込みに失敗したWorkspaceは区別し、別のWorkspaceへ暗黙に切り替えない。読取り専用の場合も、利用できる閲覧機能を残す。

ページ遷移はAPI応答を待たずに遷移先と読込み中表示を出す。旧ページを新しいURLの内容として残さず、遅れて届いた旧対象の応答で表示やcanonical遷移を上書きしない。同じ対象の再取得は別の更新中表示にし、Consoleの下書きなど編集中の内容を破棄しない。権限が必要な操作は確認済みのデータが届くまで表示しない。

## ページの目的

各routeの主要目的は、「Ticketを選ぶ」「Workerを確認する」「Repositoryのアクセスを設定する」のように、動詞と対象を一つずつ使って表現できるものにする。

- 一覧・boardは、対象を探し、注意が必要な項目を見つけるために使う。
- 詳細は、一つの対象を理解し、現在実行できる操作を行うために使う。
- form・editorは、範囲の明確な変更を行うために使う。
- Consoleは、一つのWorkerを操作しながら履歴を確認するために使う。

一つのrouteで複数の形式を組み合わせるのは、同じ目的を支える場合に限る。

## 一覧と詳細

同じ項目をリソース間で比較する一覧には、平坦なlistまたはtableを使う。人が読む名前とcanonical keyを最初の列に置く。providerの詳細、変更ID、digestは比較用の列へ常設せず、詳細表示や診断用の開閉領域へ置く。

詳細では、対象の識別情報、現在の状態、注意が必要な事項を最初に示し、本文や履歴を主領域に置く。関連、target、review、案件判断、担当のhandlingをそれぞれ別の主体が確定する場合は、独立したsectionに分ける。

## Ticket一覧

Ticketは平坦なリストで表示し、検索条件は単一のクエリ欄にまとめる。状態別のlaneや独立したfilter群は設けず、案件状態は`state:open`／`state:closed`で指定する。`open`を着手可能・queue済み、`closed`を要求実現済みと読み替えない。終了理由と説明を案件の判断として示し、再開・訂正でも過去の判断を残す。

各Ticketにはkey、title、優先判断を変える情報を表示する。分類タグ、担当principal、自己申告のhandlingを案件状態と分けて示す。検索と絞込みは取得済みの一部分ではなく、Backendのqueryへ適用してからページを取得する。

Orchestratorのhandlingは、担当ごとの自己申告として示す。`done`を案件の終了やMR承認と読み替えず、再依頼は新しい担当、回答・継続は現在の担当への入力として区別する。担当の保存、通知の受理、実作業の着手、Workerの現在状態は独立して示し、未観測・結果不明を成功で補わない。分類、案件判断、担当変更からWorker制御や権限付与を推測しない。

## Settingsと編集

一つのSettings routeは一つの設定関心事を扱う。編集操作の前または同時に現在有効な値を示し、一つの判断に必要な入力項目をまとめる。form内で塗りつぶしによって強調する操作は保存だけとする。

## Memory

Memoryは、明示的に選択したsubjectを起点に閲覧する。subjectの詳細では現在接続しているWorkerを示し、過去のWorkerやSessionの帰属情報を現在の接続として表示しない。

- `Surface`は生成されたresident contextの現在の状態を示す。生成済み、生成済みだが空、未生成、古くなった状態、生成失敗を区別する。取得要求の失敗を生成状態として扱わない。
- `Committed Memory`は現在の確定Memoryを一覧と詳細で示す。変更履歴は現在の内容とは別に閲覧できるようにし、過去の変更内容を現在値の代わりに表示しない。
- `Sources`では、候補とその処理結果、根拠となったMemoryと変更、Hostが記録したSessionへの帰属をたどれるようにする。Sessionの内容を表示するのは閲覧権限がある場合に限る。

## Worker Console

Consoleは会話と実行の履歴を主領域とする。コマンド出力はterminalに近い簡潔な表示にし、通常のメッセージ本文と分ける。

- 直接のSubWorkerの履歴を表示している間も、Composerと実行制御の対象は親Workerのままとする。
- OverviewとNormalの切替では見せ方だけを変え、操作対象や権限を変えない。
- Composerの下書きは、Workerの状態、切断、送信の受付待ちにかかわらず編集できるようにする。編集可能性と送信・実行制御の可否を分け、受付待ちの入力に後続の編集を混ぜない。
- rewind先は親Workerの会話履歴にあるユーザー入力から選び、適用前に対象を示す。履歴を戻しても、実行済みtoolの外部副作用は取り消されないことを明示する。
- 現在のWorker状態はWorkerの確定したsnapshotから表示し、WebSocketの接続状態や一時的なeventで置き換えない。
- 非公開の推論と生のsystem promptは、debug表示でも公開しない。
