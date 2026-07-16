# テスト妥当性レビュー: client

- 評価: 概ね良い

## 確認範囲

- `crates/client/Cargo.toml` と crate の全ソースファイルを確認した:
  - `src/lib.rs`
  - `src/runtime_command.rs`
  - `src/spawn.rs`
  - `src/pod_client.rs`
  - `src/ticket_role.rs`
- `runtime_command`、`spawn`、`pod_client`、`ticket_role` の既存 unit tests を確認した。
- `crates/client/tests/` 配下に integration tests がないことを確認した。
- ソースファイルや Tickets は変更していない。

## 現在のテストがよくカバーしている点

- `runtime_command` のカバレッジは焦点が絞られていて適切:
  - runtime executable には常に `pod` prefix が付与される;
  - `YOI_POD_RUNTIME_COMMAND` は executable path だけを置き換える;
  - 空の override は `current_exe` にフォールバックする。
- `spawn::runtime_args` tests は重要な引数 invariants をカバーしている:
  - workspace、pod identity、profile が分離されたままである;
  - session restore mode では profile が省略される;
  - child `cwd` は CLI argument として渡されない;
  - `ticket_role` が存在する場合は明示的に渡される。
- `pod_client` tests は mock だけでなく実際の Unix sockets を使っている:
  - client が生存している間に events を受信する;
  - `Method` JSON lines を送信する;
  - drop された clients は connections を速やかに閉じる;
  - blocked reader tasks は drop 時に abort される。
- `ticket_role` は planning について広いカバレッジがある:
  - role config 欠落と backend-only config が拒否される;
  - `inherit` profile は top-level launches で拒否される;
  - 解決不能な profile selectors は spawn 前に失敗する;
  - concrete role config と scaffold config が launch plans を生成する;
  - 設定された profile/launch prompt refs が表面化される;
  - spawn config が pod name、profile、role、workspace root、`cwd` 境界を正しく保持する;
  - prompt override precedence がテストされている;
  - role-specific launch prompts が intake、orchestrator、coder、reviewer 向けの重要な運用ガイダンスを含む。
- pre-run peer registration tests は、実際の `PodClient` framing path を通し、最初の `Run` 前の ordering を検証しているため有用。

## ギャップ / 疑問のあるテスト

- 最大のギャップは `spawn_pod` の readiness behavior。実際の subprocess launch path、stderr file polling、`YOI-READY` parsing、socket wait loop、early-exit handling、stderr tail collection、malformed ready line handling、timeout behavior はほとんど未テスト。現在の `spawn` tests は純粋な argument construction だけをカバーしている。
- `launch_ticket_role_pod_with_options` は end-to-end の host-side launch flow としてテストされていない。planning と pre-run send tests に分解されているが、`YOI-READY` を出力し、socket を bind し、最初の `Run` を受け取り、acceptance evidence を返す fake runtime process がない。
- `wait_for_run_acceptance` は重要な分岐が直接カバーされていない:
  - 一致する `Event::UserMessage` を受理する;
  - `InvokeStart(UserSend)` / `TurnStart` を受理する;
  - `Event::Error` で拒否する;
  - closed / timeout errors を返す;
  - unrelated events を安全に無視する。
- Pre-run peer registration は success と explicit error のカバレッジがあるが、timeout、connection-closed、empty peer name、multiple peer registrations、response 前の unrelated-event cases が不足している。
- Prompt tests は prompt resources から多数の exact substrings を assert している。prompt text は product contract の一部なのでこの一部は有益だが、大量の substring lists は脆く、無害な prompt wording edits が behavioral regressions に見える可能性がある。
- Default pod-name generation と sanitization は、planning cases と caller-provided name 経由の間接テストを除き、直接テストされていない。whitespace ticket ids、punctuation-heavy ids、`MAX_POD_NAME_CHARS` への truncation、空の explicit pod names のような edge cases はカバーする価値がある。
- `PodClient` tests は malformed JSON、EOF behavior、`try_next_event`、reader-channel backpressure をカバーしていない。これらは spawn/launch gaps より優先度は低いが、それでも crate の protocol boundary の一部である。
- doc tests や integration tests はない。小さな client crate としては許容できるが、この crate の責務には process/socket orchestration が含まれるため、少なくとも 1 つの lightweight fake-runtime integration-style unit test があると信頼性が実質的に向上する。

## 追加提案

- 小さな test executable または shell script を使って、`spawn_pod` 用の fake-runtime tests を追加する:
  - 通常の stderr progress lines を書く;
  - `YOI-READY\t<pod>\t<socket>` を出力する;
  - 短い delay の後に socket を bind する;
  - failure cases では早期終了する。
- readiness parsing について焦点を絞った unit tests を追加する:
  - malformed ready line;
  - socket なしの ready line;
  - ready line 後、socket が connectable になる前に child が終了する;
  - stderr tail が bounded tail だけを保持し、最後に flush された lines を含む。
- in-process Unix socket server を使って `wait_for_run_acceptance` 周辺の tests を追加する:
  - `UserMessage` exact segment match;
  - `InvokeStart(UserSend)` acceptance;
  - `TurnStart` acceptance;
  - `Event::Error` rejection;
  - connection close;
  - unrelated events がある timeout。
- pre-run peer registration の edge-case tests を追加する:
  - empty peer name は warning を出し、それでも `Run` を送信する;
  - timeout warning でも `Run` を送信する;
  - closed connection warning または send error path;
  - multiple peers が ordering を保持する。
- Pod identity stability は user-visible で runtime state paths に影響するため、`default_pod_name` / sanitization / truncation について小さな pure tests を追加する。
- exact wording が意図的に stable contract として扱われている場合を除き、prompt resource ごとに小さめの semantic sentinel assertions を維持することで、prompt の脆さを減らすことを検討する。

## 実行したコマンド

```sh
cd /home/hare/Projects/yoi && cargo test -p client
```

結果: passed。`29` unit tests が passed; doc-tests は `0` tests。このコマンドは 2 回実行され、どちらも passed。

