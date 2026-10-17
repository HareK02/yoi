# T-745 Journal compactionの検証記録

## 対象と判断の範囲

`develop`の`10d200d9`を基点に、Hostが選ぶ保存済みJournalの読取りをcompaction境界へ接続した。始動時は前任Session、compaction時は現在Sessionを優先し、同じSubjektivのtransaction・有界projection・Subject認可を使う。Worker coreは選択や権限判定を実装せず、取得結果を一度だけrenderして実compactor入力と新segmentへ渡す。

現在の読取り結果、JSONのSHA-256、prompt source/projection由来を同じ固定文面に保存する。absent／disabled／unavailableを区別し、どれも履歴が記録済みという意味にはしない。旧Journalは通常overviewとretained tailから除き、compactor専用の明示的なhistorical資料として渡す。古い文面内の自動保持説明は当時の境界にだけ適用される。新segmentで自動保持するのは今回固定した参照だけであり、過去の保存segmentは書き換えない。この分担は、取得失敗や有界省略の際にも、前のsummaryがJournalへ委ねた継続情報をcompactorが回収できるようにするためである。

この記録は機械的なtransport・保存・認可の検証結果である。所定のsummaryを返すmockは、LLMが指示の意味を保持できることの証拠ではない。

## 9到達条件との対応

1. **今回のsnapshotを入力と結果へ渡す**：workerの`journal_three_compactions_freeze_one_snapshot_preserve_full_summary_tail_tasks_and_source`で、typed startupを持つSessionの3回compactを確認した。actual初回Requestと新seedに同じ全文・参照・checkpoint・hash・prompt由来が入り、旧注入を最新参照へ混入しない。
2. **並行更新でも固定する**：standaloneの`local_host_compaction_freezes_pending_journal_through_request_and_durable_seed_during_update`は、実LocalSubjectHost／実store／PreparedWorker／実Worker compactionを接続する。外側LLMだけを代替し、最初のcompactor要求内で実Journal adapterのUpdatePositionを実行した。storeは次版になるが入力とseedは元の同じJSON・文面のままだった。worker側でも要求境界でHost応答を更新して固定を確認した。
3. **未反映情報を補う**：open/fixedのsource_endと処理checkpointが異なるsnapshotを使い、未反映ユーザー入力が実compactor入力またはretained tailへ残ることを確認した。promptはJournalを書き写さず、不足情報・未実施・結果未確認・継続指示を補う方針を指定する。実モデルによる補完summaryの意味品質は未評価。
4. **Job／Idle完了待ちをしない**：上記実Local Host接続は未反映範囲・active generation・open entryを保持したままcompactが完了し、要求内の実Journal更新も成功する。Backendの`signed_compaction_read_does_not_contact_runtime_or_wait_for_active_generation_and_rejects_jobs`はRuntime未接続・active generation中の署名要求に成功し、Idle照会やJob完了を必要としないことを確認した。checkpoint、entry、Job状態、原資料は読取りで変更されない。既存T-734受付・保持と通常自動compactのsuiteも通過した。ただし新結合試験はOSプロセスとしてJournal Jobを走らせたものではない。
5. **fallbackとSubject境界**：workerの`journal_absent_disabled_unavailable_and_over_byte_budget_keep_conversation_compaction`で空Journal、無効、Hostなし、取得失敗、上限超過を区別し、従来の会話要約が成立した。Subjektivと両Hostで現在Session優先・同Subject補完、別Worker／Session／Subject、Jobの拒否を確認した。Backendの未署名要求は401、Job要求は403。
6. **複数compactと全文summary**：3回連鎖で長い前回summaryの末尾まで全文投入し、保持境界の両端、未反映指示、TaskStore、旧Journal非重複、元segmentの原資料不変を確認した。旧Journalは当時資料としてcompactorだけへ渡され、現在のsnapshot／処理済み範囲／新resultの自動保持とは区別される。
7. **restore・通常turnで再取得しない**：通常turnとtyped rewind restoreでHostのcompaction読取り回数が増えず、catalog変更後も保存済みbody・hash・prompt由来が復元された。既存のstartup一回注入、通常restore、behavior更新のsuiteも通過。cold Runtime再起動は新規には実行していない。
8. **由来・失効を維持する**：専用JournalContext originを共通captureではDerivedSummaryへ投影し、公開materialから除外する。session-storeの実保存試験はsummary／Journal originがuser roleでも原資料にならないことを確認した。typed startupの旧保存データはイベント型からのみin-memory分類し、本文のmarkerで認可・分類しない。Subjektivの失効writer拒否とBackendの旧Worker拒否を確認し、読取りで旧更新権限を復活させない。
9. **意味評価と機械試験を分離する**：今回のliveモデル意味評価、課金、稼働環境更新は行っていない。summaryは試験で決めた応答であり、書き写し抑制・意味的な継続品質を実証したとは扱わない。

## 実行した検証

- root `cargo check`：成功。workspace-serverの既存dead-code警告3件。
- `cargo test -p worker -p standalone -p subjektiv -p session-store`：全選択target成功。worker lib 1048件、compact_events_test 28件とその他integration targets、standalone lib 82件とhost/jobs/subjektiv targets、Subjektiv 107件、session-store lib 167件とintegration targets。standalone jobsの既存ignored 1件は未実行。
- `cargo test -p yoi-workspace-server`：lib 1191件成功／既存ignored 2件、binary 12件成功、doctest成功。署名付きHTTP・実Runtime/Controller・Backend Host・実storeの接続試験を含む。
- 変更5crateの`cargo doc --no-deps`とdoctest：成功。
- `cargo fmt --all -- --check`、`git diff --check HEAD`、状態契約命名検査：成功。
- 初回試行で期待originの旧assert、追加fixtureのtoken想定、未署名status想定を修正して再実行した。最終の成功は修正後sourceに対するもの。

新規試験は通常の既存Cargo target内に置き、機能・環境変数によるopt-inへは逃がしていない。root通常runnerはCargoの通常targetを列挙する構成である。repository内にCI workflow定義は確認できず、CIで実行済み・必須化済みとは主張しない。

## 未実行・限界

live provider／実モデルの意味品質、cold Runtime再起動、全workspace test、Web／browser／明示E2E、Nix／Dockerは未実行。token数は既存estimateであり、providerの正確なtoken数ではない。snapshotと前回summary全文はoverviewの切詰め対象にせず、実compactorのhard input limitを超えた場合は切詰め成功ではなく明示failureになることを確認した。

Ticket閉鎖、target統合、他Ticket起動、稼働環境更新、Workdir削除はこの実装作業では行っていない。
