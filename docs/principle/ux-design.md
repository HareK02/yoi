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

## ページの目的

各routeの主要目的は、「Ticketを選ぶ」「Workerを確認する」「Repositoryのアクセスを設定する」のように、動詞と対象を一つずつ使って表現できるものにする。

- 一覧・boardは、対象を探し、注意が必要な項目を見つけるために使う。
- 詳細は、一つの対象を理解し、現在実行できる操作を行うために使う。
- form・editorは、範囲の明確な変更を行うために使う。
- Consoleは、一つのWorkerを操作しながら履歴を確認するために使う。

一つのrouteで複数の形式を組み合わせるのは、同じ目的を支える場合に限る。

## 一覧と詳細

同じ項目をリソース間で比較する一覧には、平坦なlistまたはtableを使う。人が読む名前とcanonical keyを最初の列に置く。providerの詳細、変更ID、digestは比較用の列へ常設せず、詳細表示や診断用の開閉領域へ置く。

詳細では、対象の識別情報、現在の状態、注意が必要な事項を最初に示し、本文や履歴を主領域に置く。関連、target、review、workflowの状態をそれぞれ別の主体が確定する場合は、独立したsectionに分ける。

## Ticket board

boardは横方向にlaneを比較する作業面とする。laneはworkflow stateを表し、隣接laneとの関係を維持する。狭い画面でもboard内の横スクロールで比較できるようにする。

各Ticketにはkey、title、優先判断を変える情報を表示する。blockerとattentionは文字でも示す。

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
- 現在のWorker状態はWorkerの確定したsnapshotから表示し、WebSocketの接続状態や一時的なeventで置き換えない。
- 非公開の推論と生のsystem promptは、debug表示でも公開しない。
