# T-810 共通エラー契約の移行棚卸し（2026-10-31）

本記録は `develop@d0f22d871ab3403943d6bcfc71bfa5a0e1f01614`（tree `d3a114568adf8d10fb8e784870efa3537092ba4c`）を対象にした棚卸しであり、本番移行済み一覧ではない。T-810は共通utilityと契約fixtureを提供し、下記の本番接続はT-811〜815に残す。旧基点 `e8477882b` からの更新は専用work branchでfast-forwardし、他担当のclone・permissions変更・稼働Server/Runtimeを変更していない。

## 残存callsiteと所有境界

| 所有 | 観測した入口・現契約 | 残す理由と移行時に守る点 |
| --- | --- | --- |
| T-811 | `workspace-server/src/server/subject_authorization.rs::authorize_scope` と `server.rs::scoped_record_subjektiv_session` の `session.record` 拒否 | 原因発生domainでoperationとresourceを型付きにする。bind前の認可順序・policyを共通core導入で変えない。 |
| T-811 | `worker/src/worker.rs::finalize_subjektiv_session_attribution` のHTTP status/body文字列化、Host分岐の `to_string()`、`worker/src/subjektiv.rs` のHost error変換 | attributionの既存永続結果・受理不明と原因を分離する。raw bodyを共通detailsに格納しない。HTTP直結だけでなくHost経由も対象。 |
| T-811 | `worker-runtime/src/worker_backend.rs::spawn_controller` のbootstrap/Session attributionの `Result<_, String>` | 原因にcontextを加え、起動結果・cleanup結果を別に保つ。 |
| T-812 | `workspace-server/src/hosts.rs` の `embedded_worker_execution_rejected`、embeddedメッセージの語/path redaction、remote診断変換 | Warning/severity・文言から原因を再構成せず、同じtyped causeを認可済みaudienceへ投影する。 |
| T-812 | `workspace-server/src/server.rs` の `workspace_worker_create_not_accepted` / `workspace_worker_create_failed` と `sanitize_backend_error`、Runtime `http_server.rs` の手書きJSON/status・fingerprint文言分類 | 外側操作結果を保持し、内側403の単純転送も一律502も避ける。HTTP境界でtyped mappingと既存tracing sinkに接続する。 |
| T-813 | `web/workspace/src/lib/workspace/api/{http,workers}.ts` の汎用Error/既存 `error,message,diagnostics` parser | 生成型だけでは受信検証・権限を保証しない。bounded parserで未知と破損を区別する。 |
| T-813 | `sidebar/worker-launch.ts::workerLaunchOutcomeUnknown` のregex、Worker新規画面の文字列合成、`sidebar/worker-actions.ts` のseverity優先表示 | regex・診断順は原因/操作結果の正本ではない。一方restoreの既存typed state/status判定は失わせない。 |
| T-814 | `agen/src/engine.rs` の `ToolResult::error(error.to_string())`、`worker/src/wip.rs` の結果encode失敗、`feature/builtin/ticket.rs` のHTTP文言再解析 | Tool dispatchと実行失敗、実行後応答喪失、cancel/unknownを区別する。共通codecは実行/再送をしない。 |
| T-814 | Worker `LogEntry::RunErrored` / controller `Event::Error`、Job/Serviceの保存・通知・照会境界 | 履歴とlive通知の正本を一組で移行する。監査・通知・sink失敗を元結果やsourceに偽装しない。 |
| T-815 | `workdir/src/lib.rs::{WorkdirDenialReason,WorkdirFailureReason,WorkdirFailureStage}`、Repository手書きHTTP応答、`workspace-drive/src/lib.rs::Error`、`config-source/src/lib.rs::ConfigTreeError` | 既存typed reason・Unknown・補償を再実装せずadapterに接続する。configの構文診断等を機械的にすべて禁止しない。 |

`to_string()`全廃lintは追加しない。型付き数値の表示、構文診断、sourceを保持した内部Displayなどまで禁止すると、実行結果や公開可否との境界を逆に失うためである。

## transport照合

| 既存source | 観測上限・注意 | 接続時の扱い |
| --- | --- | --- |
| `api-macros/src/reqwest.rs::DEFAULT_RESPONSE_BODY_LIMIT` | HTTP応答既定1MiB、route別overrideあり | failure単体上限と外枠残budgetの小さい方。bounded body readerを経ずに全bodyを先に読む実装へ戻さない。 |
| `workspace-server/src/hosts.rs` Runtime ping client | 8KiB | 最大32KiB failureはそのまま送れない。必須fieldを切り詰めず変換失敗/fallbackを分離。 |
| `protocol/src/subscription.rs` | frame16MiB/depth64、拒否message1KiB | failure自身の32KiB/depth16も検査。message文字列を構造化failureの格納先として流用しない。 |
| `protocol/src/stream.rs` | JSONL行の明示byte上限なし | transportがfailure上限を保証するとは扱わない。 |
| `manifest/src/lib.rs` / `agen/src/tool.rs` | Tool content既定64KiB、個別overrideと後段文字列truncateあり | envelope/JSON escaping後のbyte予算を計算し、必須JSONを後段で壊さない。 |
| `worker/src/feature/mcp.rs` / `mcp/src/stdio.rs` | 結果64KiB、JSON depth12、stdout/protocol line1MiB | coreのdepth16より小さい12と、wrapper後の残depthを渡す。既存の部分置換へfailureを無条件に流さない。 |
| `plugin-pdk/src/lib.rs` / `workspace-server/src/plugin_receipt.rs` | guest error message / effect error JSONは4KiB | messageとJSON envelopeは異なる制約であり、full failureの転送可否とは別に判定。 |

coreの `Budget::new` はtransport byteと残envelope byteを必須にし、32KiBとのminを取る。`with_depth` はtransport depth/外枠残depthを16とのminにする。深さはobject/arrayを1段として数え、文字列中の括弧は数えない。JSON文字列への二重符号化は境界でescaped byteを別途予算化する。core Problem Details primitiveは外側body全体にも同じ保守的なbudgetを適用する。

## reader先行とwriter切替

1. **既存保存形式**: operation/Job/Restore等の既存結果と旧diagnostics/messageを正本のまま読む。旧文章から新reasonを捏造しない。`compatibility::read_failure_attachment(None, budget)` は `LegacyWithoutFacts` を返す。欠落とpresent-but-corruptを区別する。
2. **先行reader**: T-810が提供するversion付きenvelopeのreaderとoptional attachment readerを先に配備する。presentな値は `Known` / `Unsupported` / `Corrupt` を分け、後二者を業務上の確定拒否や未実行へ変えない。受信文面・publicラベルはauthorityではない。
3. **domainごとの移行単位**: T-811〜815の所有store/consumerが追加fieldまたは既存versioned migrationを選ぶ。古い結果を保持したままreader・照会・公開再投影を先に検証する。T-810自体はDB/本番writer/consumer切替を有効化しない。
4. **新writer**: audience型で区別した投影済み値だけを共通encoderへ渡す。domain結果、operation identity、必須detailsは黙ってtruncateしない。本件encoderはbudget不足を変換失敗として返し、static fallbackは別の公開不能表示であって新しい業務原因ではない。
5. **保存再読込み**: typed validation後も現在の認可とresource可視性で再投影する。`UnsupportedFailure` は元受信境界内にopaqueに保持し、他audienceへの無条件forward APIを提供しない。
6. **rollback**: writer切替を止めても、新形式を保存した結果が残る間はreaderを残す。旧診断へ逆変換して原因を復元したことにしない。DB移行・旧形式保持期間・実展開条件は所有storeの移行で確定する。

共通sourceは `crates/error-contract`、二domain fixtureはその `fixtures/` と `test_support`、実行例は `examples/two_domains.rs` に一元化した。schema/TSは同じRust DTOのoptional deriveから既存 `protocol --example generate_typescript --features typescript` が出力する。Worker HTTP型は引き続きcanonical OpenAPI入力の `server-api --example generate_worker_launch_api_types` が正本であり、将来の実HTTP接続は境界crateのローカルwrapperと明示 `OpenApiSchema` 実装を通す。generic schemaのdetailsは未確定のtyped引数であり、domain接続時には `schema::{failure_schema,problem_schema}::<D>()` に具体型を渡す。TS宣言・JSON Schemaだけでbyte/depth/意味検証や認可済み投影を代替しない。

本棚卸しとcore契約試験は実認可・本番HTTP/Tool/背景処理の縦断成功を主張しない。その試験とsink記録点、domain別runbookは各接続Ticketが所有する。
