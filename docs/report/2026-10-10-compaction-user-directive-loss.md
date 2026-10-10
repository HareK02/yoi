# 複数回のcompactによるユーザー指示の脱落

調査日: 2026-10-10
対象runtime Worker: `01a1114f-1d68-7ea3-a706-0cbef7c7e718`
Session: `01a1114f-1e5f-7523-a106-46a6f44ea691`

## 結論

ユーザーのrevision全面撤廃指示は、最初のcompactでは原文で保持されたが、次のcompactで原文の指示欄から脱落し、その後は「冗長な状態カウンタの除去」「復旧後の統合・検証」に縮小された。最後のWorkerは元の依頼を保持できていないと認めている。保存データの消失ではなく、次のLLM contextに渡す指示と完了条件の継承の問題である。

実装には、前回要約を通常メッセージと同じ先頭2,000文字のpreviewにする一方、compact用SessionExploreからはDerivedSummaryを除外するという組合せがある。指示が後半にある前回要約を全文取得できない。プロンプトの注意書きだけでは解決しない。

## 調査対象と境界

アプリケーションのデバッグとして、ユーザーが示した`yoi.local/`配下の対象Workerの永続化データを読み取り専用で解析した。ユーザー発言、derived compact要約、assistantの通常テキスト、セグメントlineage、compact metricsのみを抽出した。生のsystem prompt、reasoning、secretファイルは取得していない。サービス、Worker、Ticket、DBの状態変更も行っていない。

証拠の基点（以下の`.jsonl`はこのディレクトリ内）:

`yoi.local/server/workspaces/0197a949-4b6b-7f2a-9d9a-1f87e3a4c5b6/embedded-runtime/workers/01a1114f-1d68-7ea3-a706-0cbef7c7e718/session/segments/`

16個のセグメントを確認。最初以外の15個すべてが、直前セグメントへの`compacted_from`を持つ。forkや別Sessionへの切替ではない。compact metricsも各回の成功を記録している。以下の時刻はすべてUTC。

## 指示が失われる時系列

元の発言は`01a12056-3d54-7b23-a90e-ee5d4cb48cfb.jsonl`に残っている。

- L20、10/09 11:04:45: 「revisionって言葉が死ぬほど嫌いなんだけど このプロジェクトでrevisionって言葉が使われている場所を全部洗い出して」
- L167、10/09 11:13:58、entry ID `01a1205e-adf8-7613-ac41-d086e9a7299e`: 全面撤廃の指示。

その指示の契約は、単に数値カウンタを別の型へ置き換えることではない。

1. 「金輪際Revisionという名前を作らせない」
2. 「コードベースから完全に絶滅させる」
3. 「何をどのようにいつチェックするのかをコードで示す必要が有る」
4. 到着順・タイムスタンプ・先勝ち/後勝ち等による単純化を検討する。
5. 全部実施する。
6. なお残す必要があるなら、最後にその必要性をユーザーへ説明する。

この指示以降の各compact結果は、新セグメントL1の`history`にある`origin.kind = derived_summary`のsummary messageで確認できる。

| compact回数（Session全体） | 新セグメント先頭ID | 時刻 | 指示の保持状態 |
| --- | --- | --- | --- |
| 10 | `01a1208e` | 10/09 12:05:57 | 上記6条件を`User Directives`に原文で保持。Goalにもcomplete purge、全残存箇所の説明がある。 |
| 11 | `01a120d2` | 10/09 13:20:33 | `User Directives`は「そのまま進めて」「Subworkerで完了まで」「一旦stashしてWorkdir原因確認」に置換。全面撤廃の趣旨は他の段落に残るが、今後の命名禁止と残存理由の最終説明が落ちる。 |
| 12 | `01a12110` | 10/09 14:28:18 | Goalが`redundant revision/counter state`の除去に縮小。Directiveは再起動後のstash適用とWorkdir修正。migration/test/docsと現行ロジックを区別する指示が追加され、残存語の扱いが元の依頼から離れる。 |
| 13 | `01a12165` | 10/09 16:00:49 | 冗長カウンタ除去とServer/SSH移行の統合が中心。Directiveは再起動・失われたSubWorker・stash適用。元の命名禁止・残存理由説明は戻らない。 |
| 14 | `01a121ae` | 10/09 17:20:58 | Active TaskはSubWorker委譲復旧の5ファイルをcommitすること。revision作業は保留中の背景へ後退。Directiveは復旧に関する最近の発言だけ。 |
| 15 | `01a123de` | 10/10 03:32:20 | Goalは非同期fix後の統合、検証、review、commit。Directiveは「revisionの件を終わらせて」、SubWorker回収、ユーザーによる復旧報告だけ。「revisionの件」が何を意味するかの原文契約がない。 |

最後のセグメントでは次の通常assistant発言を確認した。

- L1057、10/10 07:08:04: 「更新カウンタとしてのrevisionを置き換える本体作業は完了」「名前まで完全に整理し切ったわけではありません」。旧DB移行コード・過去イベント用フィールドは意図的に残したと整理する。
- L1065、10/10 07:09:57: 「元の依頼の趣旨と削除範囲を、正確には保持できていません」「最初の依頼の原文が現在の会話コンテキストから欠けており、引き継ぎ要約と実装内容を根拠に進めていました」。完了判断を撤回する。
- L1073、10/10 07:12:54: 提供ツールでは自己の過去segmentを取り直せないと回答する。
- L1078、10/10 07:14:38: ユーザーが元の指示を再掲。
- L1081、10/10 07:15:15: 再掲後に、名前の撤廃と残存理由説明が契約だったと認識し直す。

## 実装上の課題

### 1. 前回要約の後半を取得できない

`crates/worker/src/worker.rs`:

- L6626–6631: 今のhistoryのretained tailより前だけを要約対象とする。
- L6760–6765: compact用captureもそのprefixだけ。過去segmentの原文を辿るcaptureではない。
- L8838–8854: メッセージを新しい順に拾う。ユーザー指示や前回要約を別枠で保護しない。
- L8839、L8943–8959: system roleの前回要約も通常メッセージ扱いで、先頭2,000文字のみ。

原文を正しく保持していた`01a1208e`のsummaryは5,632文字で、`## User Directives`の開始位置は0-basedで4,175文字目。次のcompactのoverviewでこのsummaryが選ばれても、6条件の原文はpreviewに入らない。さらに次の`01a120d2`のsummaryは5,429文字、指示欄の開始は4,332文字目。最新の手順より後ろに契約を書くsummary形式と、先頭だけ読むpreviewが噛み合っていない。

`crates/worker/src/session_capture.rs`:

- L355–357: `message_reference_kind`がNoneならindexに入れない。
- L733–752: `DerivedSummary`はNone。

この同じcaptureを使う`ShowOverview`・`SearchEntries`・`ReadEntry`では、前回要約を全文取得できない。「indexで不足ならツールで読め」というcompactプロンプトの回復手順が、前回要約については成立しない。peer observationの秘匿境界を広げるのではなく、compactサービス自身の入力契約として別途扱う必要がある。

### 2. ユーザー契約を保持する独立した継承経路がない

`resources/prompts/internal/compact_system.md` L34–35は`User Directives`を「次に失うべきでない文言だけ」としているが、前回の未撤回・未完了の指示を持ち越す規則や、脱落の検証がない。

`worker.rs` L6862–6925のoutput guardはsummaryの有無とサイズ、file nominationを確認するだけ。元の禁止事項、作業範囲、例外説明義務が維持されているかは確認しない。TaskStoreや最近の進捗は契約の代わりにならない。

今回の最初の保持summaryは約1,478 tokens、次は約1,380 tokens。保存されたmanifestのsummary targetは2,000、maxは4,000。最終出力が制限で不可避に削られたとは言えず、1件2,000文字の入力previewと情報優先順位の問題が目立つ。内部ログがないため、途中でsummaryの書き直しが行われたかまでは断定しない。

### 3. 中断・復旧の手順が本来の完了条件を置き換える

「stashして原因確認」「SubWorker復旧」は本来の依頼を一時中断する手順だが、summaryでは次第にActive TaskとUser Directivesの中心になる。「revisionの件を終わらせる」という参照だけが残り、その参照先の原文が消える。

途中でカウンタの除去・migration対応・テストが進んでも、それは「全面撤廃」「残すなら最後に理由説明」の充足を意味しない。実装とテストの成功からユーザー依頼の完了を逆算した判断も、このケースで確認できた問題である。

### 4. 自己の過去契約の回復とcompact監査が弱い

通常Workerの返答では自己の過去segmentを読む経路がない。原文はdiskに残っているが、LLM側から取り直せない。許可された自己Sessionのユーザー発言・derived summaryに限定したread-only回復経路が必要。

`crates/worker/src/internal_worker.rs`はcompact等のInternal Workerに`EphemeralSessionStore`を使う。今回の永続化データから確認できるのはcompact結果とmetricsで、各回の正確な入力・検索・読取・write_summaryの過程は再生できない。どのツールで何を読んだかをこの報告は推測で断定しない。

## 改善案（未実装）

1. **まず全文継承の欠陥を修正する。** 前回derived summary、特にactive user directivesをcompact入力の専用領域に全文で渡す。compact用captureから必要なderived summaryを取得可能にする。ただしpeer observationやreasoning/raw system promptの境界は広げない。
2. **長期の指示を作業要約と分離する。** 元のuser entry IDと原文を持つ、禁止・範囲・完了条件・例外説明義務の継承データを用意する。撤回・置換・充足の記録はappend-onlyで残し、手順変更だけで契約を消さない。選別の誤りを避けるため、AIの解釈文だけでなく元の発言を辿れることが重要。
3. **compactの入出力で保持を検証する。** active directiveのsource IDが引き継がれているかを検査し、途中の小タスク完了を根拠に元の契約をdropしない。単にsummary budgetを増やすだけでは解決しない。
4. **自己Sessionの履歴回復とcompact監査を用意する。** 現contextに原文がないとき、明示的なread-only自己履歴ツールで過去ユーザー発言を検索できるようにする。compact入力の選択範囲・保持/省略の根拠・最終出力も、秘密情報を混ぜず永続化する。

回帰検証は単発compactだけでなく、長いsummaryの後半に元指示があり、Workdir復旧やstashで中断した後、3回以上compactしても元の範囲・禁止・完了条件・例外説明義務が残るケースが必要。テスト追加時は`docs/development/rust-testing-strategy.md`に従う。

## 断定範囲

指示の各summaryでの脱落・変質と、最後のWorkerの誤った完了判断は永続化データで確認した。入力previewとcaptureの設計欠陥は調査時のコードで確認した。ただし実行当時の各compact内部会話と正確なバイナリ対応は今回の保存データだけでは確定できない。したがって、特定の内部tool callが欠落の唯一の原因だったとは断定しない。

この調査ではcompact本体や対象作業を修正していない。追加したのは本報告のみ。

## 調査後の確認

- 独立したread-onlyコード確認でも、2,000文字previewとDerivedSummaryのindex除外による回収不能条件を確認した。
- root `cargo check`を実行したが、`worker-runtime/src/working_directory.rs`と`workspace_issuer.rs`の旧フィールド参照6件（E0609）で失敗した。
- `cargo fmt --all -- --check`は`workspace-server/src/server.rs`のformat差分で失敗した。
- `git diff --check HEAD`は成功した。
- 調査開始時のgit statusはcleanだったが、終了時には本報告以外に多数の変更が存在した。本調査でRust等のソースは変更しておらず、これらの同時進行変更は修正・reset・整形していない。
- 報告のみの追加なので、新規テストやcrate testは実施していない。compact内部会話が永続化されないという断定範囲の制約は残る。
