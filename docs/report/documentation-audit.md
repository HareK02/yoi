# docs 全体監査 — 2bf68a5b

この文書は対象時点を固定した監査レポートであり、現行仕様・操作手順の正本ではない。下記の更新・統合・履歴化は提案であり、既存文書の移動・削除・書き換えは実施していない。

## 結論

問題の中心はファイル数そのものではなく、**現行契約・過去の提案・実装時点の検証記録・操作手順が混在し、変更時に追従すべき正本が不明瞭なこと**である。現行コードと異なる設定・権限説明も確認できた。一括削除ではなく、誤設定や権限理解に影響する記述の訂正を優先し、その後に索引と文書の役割を整理することを推奨する。

残す価値が高いのは、authorityの所在、context/historyの不変条件、unknown outcome、transaction/durabilityの限界、権限を越えない復旧条件など、コードだけから理由を復元しにくい説明である。長さだけで削除しない。過去の報告も当時の失敗・観測・判断の証拠として扱い、現在の不具合一覧には転用しない。

## 対象・方法・限界

- 対象：`yoi-origin`、`hare/develop`、HEAD `2bf68a5b`。最新develop全体に対する判定ではない。
- 開始時点で `docs/development/rust-testing-strategy.md` と `docs/development/validation.md` に、この会話で合意した未コミット変更があった。両文書はその内容を基準とした。
- 6つの読み取り専用SubWorkerへ重複しない主担当範囲を割り当てた。補助的な相互参照はある。親が各Workerのcommitted sessionと主要指摘の文書・コードを確認した。
- ファイルの存在、件数、サイズ、行数は親側で実ファイルを列挙して計測。文書内容の評価は全文読取りを基本とし、コード照合は重要な主張への選択的な静的確認である。
- テスト、CLI起動、TOML parse、migration、実環境の認証・復旧・削除、外部URLの到達性は実行していない。文書中の過去の成功件数を再実証したものではない。
- 秘密値、個人環境の具体値、ローカルバックアップ内容は本レポートへ転記しない。
- 読取範囲：202件のうち200件を全文読了（空READMEを含む）。機密候補のignored reportメモ1件は冒頭5/299行のみ、ZIP1件はmetadata限定で、内容展開していない。ローカルMarkdown43件は追加読取りで全件本文を確認済み。

### 規模

以下の「維持文書側」は内容が正しいという判定ではなく、`report` と `.local` を除いた配置上の区分。本レポート自身は母数に含めない。

| 区分 | ファイル数 | テキスト行数 | bytes |
| --- | ---: | ---: | ---: |
| 維持文書側（設計・開発・索引・例・release note） | 67 | 7,601 | 576,349 |
| report（旧テスト監査21件、ignoredのローカルメモ1件を含む） | 91 | 6,580 | 468,301 |
| .local | 44 | 5,886 | 3,442,359 |
| 合計 | 202 | 20,067 | 4,487,009 |

`.local` のうち1件は3,082,949 bytesのZIPで、行数から除外した。全テキストは約1.40 MB。最初の探索でZIPもテキスト換算した行数は誤りとして破棄しており、表には使用していない。

通常のGlobに出る157件と、除外されたローカル資料45件を区別した。バイト数の大半はZIPであり、「大量の現行説明文」がすべてを占めるわけではない。

### 分担

| 担当 | 範囲 | 件数 |
| --- | --- | ---: |
| docs-design-core | architectureと基盤design | 25 |
| docs-design-surfaces | WIP・Standalone・subjektiv design | 13 |
| docs-development-index | development、docs直下、examples、release-notes | 29 |
| docs-report-history | report直下 | 70 |
| docs-test-audit-history | 旧テスト監査群 | 21 |
| docs-local-material | .local | 44 |

## 優先して訂正する現行文書

P1は設定・認可・操作判断への影響が大きい文書上の不整合、P2は構造理解・運用・保守の問題を示す。実装の脆弱性を判定するseverityではない。行番号は上記作業ツリーの監査時点。

| ID | 優先度 | 確認した不整合と証拠 | 推奨 |
| --- | --- | --- | --- |
| D01 | P1 | `docs/manifest.toml:134–150` は出力・添付上限を `worker` 配下に置くが、`crates/manifest/src/config.rs:663–695,1326–1339` は `engine` 配下で読む。 | 設定例を実際の所有節へ訂正。現状の例を有効化しても意図した値に届かない静的根拠がある。parse実行による再現は未実施。 |
| D02 | P1 | `docs/backend-auth-account-model.md:11` はownerを任意と説明。`crates/workspace-server/src/latest_schema.sql:1138–1145` はNOT NULL、`src/store.rs:318–322` も必須。 | 初期導入時の説明と現行owner必須・owner認可を分ける。 |
| D03 | P1 | `docs/development/web-state-preconditions.md:15` に保存されたRuntime verificationを前提とする説明が残る。`web/workspace/src/lib/workspace/api/runtime-management.ts:564–592` はそのfieldを持たず、`docs/development/server-runtime-auth.md:3–5,75–79` も毎要求の認証と履歴の分離を説明。 | binding identity、今回の接続試行結果、毎要求の認証を区別する。旧保存状態をreadiness条件へ戻さない。 |
| D04 | P1 | `docs/design/tool-permissions-scope.md:19–21` はcanonical targetでの判定を一律に説明。`crates/manifest/src/scope.rs:304–327` はallowのresolved/logicalを区別し、denyは両identityを検査。 | policy別の判定とdelegation条件を説明し、「symlinkが権限を作る」こととは区別する。 |
| D05 | P1 | `docs/design/session-observation.md:41` はOrchestrator identityとWorkspace全体の集合で認可すると説明。`crates/workspace-server/src/server.rs:22444–22455,22498–22509` はactiveな対象別observe grantを使う。 | role名・feature enablement・対象列挙・capture認可を分離。grantを現行の根拠として記述する。 |
| D06 | P1 | `docs/design/workdir-wip-projection.md:101–106` はencoded suffix形式。`crates/worker/src/wip.rs:1109–1114` と `docs/design/worker-wip-mode.md:163` は構造化したscope/name参照。 | 共通参照契約へ統一し、旧形式は必要な履歴だけに残す。 |
| D07 | P1 | `docs/design/ticket-wip-projection.md:31–57` は `complete` を欠き、work-reportをread/comment限定とする。`crates/worker/src/feature/builtin/ticket.rs:634–643,1634–1639` は現行公開面が異なる。 | 操作表・アクセス別一覧を同時に訂正。公開と実行権限を混同しない。 |
| D08 | P1 | `docs/design/subjektiv-store.md:3–20` はServer/Workspace専用の所有境界。`crates/subjektiv/src/lib.rs:1–7` はHost-neutralなdomain、`crates/standalone/src/subjektiv.rs:127–129` も共有storeを使う。 | 共通domain契約とBackend/Standaloneそれぞれの物理scope・外部認可を分ける。 |
| D09 | P1 | `docs/design/orchestrator-worktree-layout.md:9–14,25–27` の旧launch型・四rootを現行実装として案内。現行 `crates/workspace-server/src/hosts.rs:510–549` は別のattachmentモデルで、`resources/prompts/role/orchestrator.md:5,23,31` は個別のreturn path・結論・cleanup authorityを要求。 | 当時のdecisionとして履歴化。実行cwdとauthorityを混同しない原則は現行文書へ保持。 |
| D10 | P1 | `docs/design/workspace-kanban-orchestrator-runtime.md:5` は提案と明記するが、`docs/README.md:20` は現行動作として紹介。提案の `:17` の起動前state更新は `resources/prompts/role/orchestrator.md:7` の受理順序と異なる。 | 提案を誤りとして消さず、導線を提案・履歴扱いへ直す。 |

## 次に整理する不整合・保守負担

- **system mapの旧所有crate**：`docs/design/overview.md:12–13` の `pod-store` とname-keyed metadataは、`crates/session-store/src/lib.rs:137–142` のWorker aggregateと `docs/design/worker-session-state.md:14–44` の説明に合わない。入口文書を先に直す。
- **schema一覧の写経**：`docs/development/workspace-schema-migrations.md:14–19` はcurrent chainを50～53とするが、`crates/workspace-server/src/store.rs:30–31` のlatestは90。`docs/design/standalone-jobs.md:102–111` のschema 1も `crates/standalone/src/job_store.rs:19,179–193` の3と異なる。全DDLの再列挙より、正本と移行保証へ案内する。
- **可変モデル設定の固定化**：`docs/design/ticket-item-checker.md:25`、`subjektiv-surface.md:19` のモデル名が対応Profileと異なる。保証は「実効Profileを使用」とし、具体値は時点付き検証記録へ置く。
- **startup errorの過剰な安全保証**：`docs/design/standalone-agent-host.md:31` のcategory-only/path非表示の断定に対し、`crates/standalone/src/host.rs:62–70,727–743` はpathを表示するvariantなどを持つ。実際のUI・logへの露出は未追跡で、credential漏洩を断定しない。
- **破壊的runbookの不足**：`docs/development/subjektiv-product-cutover.md:54,83` だけではowner条件・endpoint・confirmation payloadを特定できない。根拠は `crates/server-api/src/lib.rs:1121–1134,9931–9937` と `crates/workspace-server/src/server.rs:18868–18880`。書き直す際は前提・対象確認・backup・停止条件を自己完結させる。本監査ではresetを実行していない。
- **UXの規範と実装差**：`docs/development/ui-ux/product-ux.md:26–36,54–65` にDriveがなく、`:101` のResource row参照も定義先にない。`web/workspace/src/lib/workspace/sidebar/WorkspaceSidebar.svelte:81–86` と `docs/design/workspace-drive-web.md:3–9` を照合。文書とUIのどちらを正すかは別途判断する。
- **重複の正本が不明**：WIP共通契約とprojection別文書、Pluginの2設計文書、Driveのstorage/Web/tool説明、subjektivのstore/Host別説明は、共通契約と固有差分へ分ける余地がある。固有のauthority・crash/unknown境界は削除しない。
- **案内先の内容不足**：`docs/development/plugin-development.md:39` はarchive formatをplugin-packagesへ案内するが、現行31行に該当する説明がない。ファイルが存在しても導線として成立していない。
- **development内の履歴**：`wip-integration-validation.md` と `ui-ux/subject-memory-information.md` は特定sourceでの検証・監査記録。現行契約と区別し、履歴として配置する。診断文書にもincidentやTicket経過が混在している。
- **周辺READMEとの矛盾**：root `README.md:15,73–80` のLocal Tickets・広い検証例は、現在の `AGENTS.md:39–56`、`docs/development/work-items.md:5`、更新済みvalidationと整合させる必要がある。今回の監査主対象外だが連携修正候補。

## 導線の機械的確認

維持文書側のMarkdownは65件。`docs/README.md`の直接リンクは33件で、それらの文書内リンクを辿って到達できるのはREADME自身を含め38件。27件はこの経路では到達できない。architecture 2件、Standalone、認証、Drive、Workdir削除・診断などの重要文書も含む。

この計測はMarkdownのinline相対ファイルリンクのみを対象とし、fenced code、外部URL、anchor-only、report、.localを除外した。backtickのpath参照、root READMEやコードからの導線は数えていない。したがって「27件が一切参照されていない」という意味ではない。

同じ条件で相対ファイル参照79件を確認し、存在しないファイルは0件だった。ただし見出しanchorや説明内容は別問題である。履歴側では次を確認した。

- `docs/report/test-validity-20260612/README.md:22` が案内する `protocol.md` は存在しない。評価を捏造して補完せず、当時の成果物の所在を確認する。
- `docs/report/2026-10-06-t704-validation.md:79` のT-701への `#validation-results` は、参照先の見出しにない。

33項目の単一Reading orderは必読列に見えやすい。少数の共通入口と、開発・運用/診断・領域設計・履歴の用途別索引へ変えることを推奨する。最初からディレクトリを全面移動する必要はない。

## reportの扱い

`docs/README.md:3–5,54–66` は単発debugやTicket履歴を維持面から外す方針だが、`AGENTS.md:71` は障壁・ツール問題・ユーザー指示に基づくreport作成を認めている。reportを「時点固定の観測・提案・検証記録」と定義すれば両方を維持できる。現行規範と履歴保存の規則を混ぜない。

通常のreport直下69件は、全文会話dumpではなく説明・集計・限定抜粋が中心だった。長文だけを理由に削除しない。

- **旧操作手順**：5月のファイルTicket削除・停止・cleanup提案や6月の手修復記録は、現行runbookではないと明記し、現在のauthorityへ案内する。
- **同テーマの分散**：起動CLI不整合、cwd、attachment不足、tool projection、delegation拒否、severity入力、capture freshnessは、テーマ索引を付ける。別の障壁を一つの「scope問題」に潰さず、個別観測は保持する。
- **古いred結果**：6月のtools doctest報告の4引数不一致は、現在の `crates/tools/src/tracker.rs:29–43` と `crates/tools/src/lib.rs:44–48` では該当しない。これだけで現在の全testがgreenとは言えない。過去結果を現在値へ上書きもしない。
- **解決追記の時系列**：`2026-08-12-dogfood-restart-self-termination-and-runtime-restore-panic.md:107–172` は後続解決の後に初期診断のcurrent stateが続く。初期checkpointと後続確認の時点を明記する。
- **review再送の誤一般化**：severity訂正成功の記録と再提出しなかった記録が併存する。過去の成功は現行の一回制約を解除する根拠ではない。なおWorker入力のStringとBackend enumの差は静的照合でも確認されており、解決済みとはしない。
- **MRとの二重更新**：source固定の検証台帳は保持し、routine review/fixの現行証拠はcanonical MRへ案内する。ローカルlogのpathだけでは再確認可能性を保証しない。

## ローカル資料・機密候補

`.local`は既に維持文書面の外と明記されている。古いprivate researchを現行手順の欠陥として数えない。空のREADME、移動前pathの残存、外部資料の取得時点/対象source不足、入口メモと分析の混在が整理対象となる。

- 独自の採否理由・観測・分析は保管価値がある。外部URLがあるだけで再生成可能とは判断しない。
- 入口リンクだけのメモや固有情報のないinventoryは、索引への統合・削除候補になり得る。ただし今回削除はしていない。
- ZIPとreport内のignoredローカルメモは、機密性と復旧価値を未判定とする。ignore設定は公開安全性を保証しない。内容展開・履歴改変・削除・秘密の失効は本監査では行わない。
- この公開可能な報告では、ローカル資料の具体的な機密候補・バックアップ内容を列挙しない。通常面157件の一覧を付録に置き、ローカル45件は確認水準と限界を別記する。

## 整理する順序の提案

1. **P1の設定・権限・操作案内を訂正**する。文書を現行実装に合わせるのか、意図した保証に実装を戻すのかを必要箇所で判断する。
2. **READMEの用途別索引と文書の状態表示**を整える。現行契約、提案、履歴、runbook、referenceの区別を明示する。
3. **共通契約の正本を絞る**。各領域は固有差分と例外に集中し、API/DDL/可変設定の全面写経を減らす。
4. **履歴を整理する**。当時の失敗・根拠を保ち、対象commit、後続変更、現行未確認を表示する。重複事例はテーマ索引で結ぶ。
5. **ローカル資料を別扱いにする**。索引・保管目的・公開区分を決め、機密候補は所有者の判断を得て扱う。

新しい文書を増やすことや削除件数を目的にしない。既存の小さく明確な文書は維持し、必要な安全境界・判断理由を失わないことを完了条件とする。

## 未確認事項

- 各文書の全主張が実装と一致すること、全テストやCIの実行結果、live配備との一致。
- 過去のTicket/MRの現在状態、当時の全artifactやlogの現存性、欠落protocol資料の所在。
- Docker/Runtime文書のprimary Workdir前提と現行alias-keyed attachmentsとの完全な対応。
- startup errorの各UI/log経路での最終表示、全秘密の流れ、全migration/crash window。
- ZIP内部と機密候補メモの全内容、過去の公開履歴・復旧可能性。

## 付録：通常面157件の全件評価

全件本文読了。維持は「有用で主要な不整合を今回確認しなかった」、更新は「現行説明/導線の修正」、統合は「固有情報を残して重複や索引を整理」、履歴化は「現行規範と分離」、要確認は「追加照合が必要」を意味する。単なる古さだけによる削除判定はない。

| path | 判定 | 根拠・理由 |
| --- | --- | --- |
| `docs/README.md` | 更新 | 33項目の読む順序を用途別索引へ |
| `docs/architecture/worker-restore-observations.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/architecture/workspace-worker-operations.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/backend-auth-account-model.md` | 更新 | D02：owner必須条件を訂正 |
| `docs/design/checkout-wip-projection.md` | 統合 | 共通WIP・provider固有保証・検証手順を分離 |
| `docs/design/compaction.md` | 更新 | 原則とmetrics比較手順を分離 |
| `docs/design/context-history.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/durable-operations.md` | 更新 | 共通原則とdomain別実装明細を分離 |
| `docs/design/feature-chat-invocations.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/flow-state-graph.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/memory-knowledge.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/merge-request-wip-projection.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/objective-wip-projection.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/orchestrator-worktree-layout.md` | 履歴化 | D09：旧launch契約と現行方針を分離 |
| `docs/design/overview.md` | 更新 | 旧crate地図・metadata所有境界を訂正 |
| `docs/design/plugin-component-model.md` | 統合 | offline契約の重複と将来提案を分離 |
| `docs/design/plugin-packages.md` | 更新 | 共通契約を集約しformat導線を修正 |
| `docs/design/profiles-manifests-prompts.md` | 更新 | 現行契約と移行・運用・経緯を分離 |
| `docs/design/provider-model-boundary.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/server-feature-storage.md` | 更新 | 共有crateとServer facadeの所有関係を補足 |
| `docs/design/session-observation.md` | 更新 | D05：observe grantによる認可へ訂正 |
| `docs/design/standalone-agent-host.md` | 更新 | startup errorの安全保証を出力層ごとに確認 |
| `docs/design/standalone-jobs.md` | 更新 | 旧schemaと実装前設計の位置付けを訂正 |
| `docs/design/standalone-subjektiv.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/subjektiv-consolidation-jobs.md` | 更新 | 共有domainへの参照とBackend固有境界を明確化 |
| `docs/design/subjektiv-store.md` | 更新 | D08：Host-neutralなdomainを正本にする |
| `docs/design/subjektiv-surface.md` | 更新 | 可変Profile値とHost別適用範囲を区別 |
| `docs/design/ticket-evidence.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/ticket-item-checker.md` | 更新 | モデル固定値を実効Profile参照へ |
| `docs/design/ticket-wip-projection.md` | 更新 | D07：completeとアクセス別操作表を訂正 |
| `docs/design/tool-permissions-scope.md` | 更新 | D04：resolved/logicalとdenyの違いを明示 |
| `docs/design/workdir-wip-projection.md` | 更新 | D06：構造化Interface参照へ訂正 |
| `docs/design/worker-session-state.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/worker-wip-mode.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/workspace-config-wip-projection.md` | 更新 | 現行契約とoperator手順・実装検証記録を分離 |
| `docs/design/workspace-drive-web.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/workspace-drive.md` | 更新 | 固有保証を残し運用・生成・移行詳細を分離 |
| `docs/design/workspace-kanban-orchestrator-runtime.md` | 履歴化 | D10：提案を現行手順として案内しない |
| `docs/design/workspace-optional-repository.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/design/workspace-runtime-docker.md` | 要確認 | primary Workdirと現行attachmentsの対応を追加照合 |
| `docs/development/dogfooding.md` | 更新 | 再起動主体をAGENTSのユーザー操作限定へ整合 |
| `docs/development/environment.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/external-workdir-sharing.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/file-operations.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/plugin-development.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/runtime-request-proof-diagnostics.md` | 更新 | 診断契約と未再現incident記録を分離 |
| `docs/development/rust-testing-strategy.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/server-runtime-auth.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/subjektiv-product-cutover.md` | 更新 | owner・操作契約・backup前提を補う |
| `docs/development/ui-ux/application-architecture.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/ui-ux/design-language.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/ui-ux/product-ux.md` | 更新 | Driveと未定義Resource row参照を整理 |
| `docs/development/ui-ux/subject-memory-information.md` | 履歴化 | 特定sourceのUI監査を現行規範と分離 |
| `docs/development/ui-ux/visual-review.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/validation.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/web-state-preconditions.md` | 更新 | D03：廃止verification前提を訂正 |
| `docs/development/wip-integration-validation.md` | 履歴化 | 特定sourceの検証台帳として保存 |
| `docs/development/work-items.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/workdir-refusal-diagnostics.md` | 更新 | 診断契約を残しincident・実装検証を分離 |
| `docs/development/workdir-removal.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/worker-drive.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/development/workspace-request-admission.md` | 更新 | 現行admissionと末尾incident記録を分離 |
| `docs/development/workspace-schema-migrations.md` | 更新 | 古いcurrent chain一覧を正本参照へ |
| `docs/examples/plugin-component-tool/lib.rs` | 統合 | starterとの役割を整理、既存test参照を保持 |
| `docs/manifest.toml` | 更新 | D01：上限設定のTOML節を訂正 |
| `docs/release-notes/repository-local-yoi-removal.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
| `docs/report/00001KZXWKD01-internal-subworker-feature-installation.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-05-05-file-ticket-scope.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-05-10-ticket-lifecycle-branch-placement.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-05-22-pod-event-callback-delivery.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-05-22-read-pod-output-singular-log-entry.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-05-24-compact-retained-tail-oversize.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-05-25-spawnpod-post-tool-hang.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-05-31-child-pod-visibility-restore-loss.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-06-14-session-jsonl-partial-write-restore-failure.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-07-28-ticket-cli-instructions-drift.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-07-30-restored-worker-workspace-credential-stale.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-07-30-spawn-worker-runtime-cli-drift.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-07-30-spawn-worker-runtime-cli-mismatch.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-08-03-lan-http-randomuuid-blocks-websocket.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-08-05-session-enospc-partial-jsonl.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-08-06-request-local-image-bypassed-history-pruning.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-08-06-subworker-completion-idle-auto-run.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-08-06-ticket-search-audit-visibility.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-08-06-web-console-notify-double-projection.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-08-11-flow-verifier-stale-session-capture.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-08-12-dogfood-restart-self-termination-and-runtime-restore-panic.md` | 更新 | 初期診断checkpointと後続復旧の時系列を整理 |
| `docs/report/2026-08-19-coder-toolchain-missing-rustfmt.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-08-19-subworker-spawn-missing-worker-control.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-08-31-workspace-migration-retention-generalization.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-09-19-svelte-validation-rewrites-live-route-manifest.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-09-22-repository-local-yoi-final-authority-audit.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-09-23-remote-worker-repository-access-restore.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-09-24-compaction-image-retention-budget.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-09-24-runtime-workdir-attachment-scope-and-command-cwd.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-09-24-standalone-workspace-symlink-policy-regression.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-09-24-worker-lifecycle-restore-atomicity.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-10-05-review-severity-tool-contract.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-05-review-verdict-schema-rejection.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-05-review-verdict-validation.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-05-reviewer-subworker-stopped-before-run.md` | 要確認 | 開始前停止の解決状況・現在再現は未確認 |
| `docs/report/2026-10-05-subworker-cwd-attachment-root.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-05-subworker-ticket-context.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-06-review-finding-severity.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-06-t701-validation.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-10-06-t702-command-output-readback.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-06-t702-stale-checker-validation.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-10-06-t704-validation.md` | 更新 | T-701の欠落anchorとMR証拠参照を整理 |
| `docs/report/2026-10-06-t708-validation.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/2026-10-09-parallel-workdir-read-busy.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-09-runtime-http-only-feature-dependencies.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-10-09-subworker-workdir-recovery-ergonomics.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/2026-10-10-compaction-user-directive-loss.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-10-10-develop-async-audit.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-10-10-repository-key-migration-timestamp-mismatch.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/2026-10-10-workdir-removal-mount-and-timeout.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/T-695-subworker-path-coordinates.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/T-700-subworker-followup-observation.md` | 要確認 | 通知とcaptureの差の原因・解決は未確認 |
| `docs/report/T-711-import-cycle.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/merge-request-open-ticket-target-discovery.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/spawn-worker-delegation-scope-mismatch-20260730.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/spawn-worker-runtime-cli-mismatch-20260730.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/state-contract-cleanup-recovery-checkpoint.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/state-contract-name-audit.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/subworker-cwd-tool-paths.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/t-714-ticket-read-auth-blocker.md` | 要確認 | 401原因と後続解決は未確認 |
| `docs/report/t-722-subworker-cwd.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/t-732-subworker-checkout-attachment.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/t718-subworker-path-context.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/t724-subworker-root-binding.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/report/t724-validation.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/t729-restored-base-test-compile-blocker.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/t731-review-verdict-registration-401.md` | 維持 | 時点固定の観測・判断・検証証拠として保存。現在の解決状態は未保証 |
| `docs/report/test-code-audit-20260806.md` | 履歴化 | 当時の診断・手順・検証結果を現行判断から分離 |
| `docs/report/test-validity-20260612/README.md` | 更新 | 欠落protocol資料・対象source・履歴表示を明確化 |
| `docs/report/test-validity-20260612/agen-macros.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/agen.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/client.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/daemon.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/lint-common.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/manifest.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/memory.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/pod-registry.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/pod-store.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/pod.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/project-record.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/provider.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/secrets.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/session-analytics.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/session-metrics.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/session-store.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/ticket.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/tools.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/tui.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/test-validity-20260612/yoi.md` | 履歴化 | 旧crate別の具体的監査を保持。現在の不足・失敗一覧には使わない |
| `docs/report/workdir-subworker-delegation-recovery-denial.md` | 統合 | 共通テーマの索引へ接続し、個別観測・異なる原因は保持 |
| `docs/standalone.md` | 維持 | 固有の境界・不変条件・手順を保持。主要な不整合は今回未確認 |
