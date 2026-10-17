# Ticket実応答のWeb適合

`server_ticket_web_roundtrip_tests.rs` が、一時SQLiteと本番routerのHTTP bodyをUTF-8文字列のまま `roundtrip.ts` へ渡す。Webの本番parser・Answer operation・handling summaryを使い、旧done結果、caseのclose/reopen履歴、明示再依頼後の新waiting、retained release、ページ継続、現在のAnswer receiptを確認する。期待する状態と結果はfixtureの操作意図で与え、動的ID/eventは保存正本で別に観測する。応答JSONの再構成や値の補完はしない。

## 実行選択

rootまたは `web/workspace` で次を実行する。

```sh
deno task test:ticket:real-api
```

root taskはServerの通常lib targetから `ticket_web_roundtrip_tests` を `--ignored` 付きで明示選択する。DenoがPATHに必要なため通常Cargo実行ではignoredとなる。Deno以外の外部tool、npm取得、network権限、実モデル、稼働Serverは不要。自動jobからもこのtaskを選択する。リポジトリにはCI workflow定義がなく、CIでの実行完了や自動選択は保証しない。

一時DBはRAII、子Denoは `kill_on_drop` とstdin書込みを含むwatchdogで隔離・回収する。認証情報やDBパスは子へ渡さず、子のstdout/stderrを失敗診断へ残す。通信transport自体はRust router内であり、Denoは実HTTP bodyの消費者である。browserのfetch・DOMまでつなぐ製品E2Eとは称さない。

局所描画・操作は `TicketHandling.browser.spec.ts` の通常component suiteが担当する。こちらは手書きfetch fixtureで履歴選択中も現在のCASへAnswerすること等を確認するため、実Backend適合とは区別する。実session配送・復旧は既存Server delivery suiteに委ね、このbridgeではRuntime実行境界をrecordingに置き換える。
