# Review: Bash ツール

## Round 1 (初回レビュー)

### 前提・要件の確認

- **コマンド実行 (`tokio::process::Command`)**: 満たされている。`crates/tools/src/bash.rs:94-103` で `bash -c <wrapped>` を起動。`stdin(null)` で stdin ブロックを防止、`kill_on_drop(true)` でタイムアウト時のリーク防止。
- **timeout (default 120s / max 600s)**: 満たされている。`bash.rs:38-39, 64-67` の `clamp(1, 600)`、`bash.rs:130-144` の `tokio::time::timeout`。`timeout_kills_long_command` で動作確認済み。
- **作業ディレクトリの永続**: 満たされている。`cd` のパースに頼らず wrapper script + tempfile で post-command の `pwd` を取得（`bash.rs:74-90`）。`cd_persists_across_calls` テストで `subdir` 移動後の `pwd` が反映されることを確認。`canonicalize` 同士で比較しており macOS の `/private/tmp` ずれにも耐性あり。
- **stdout/stderr 結合**: 満たされている。wrapper 内 `exec 2>&1` で実装、`merges_stdout_and_stderr` テストで両方含まれることを確認。子プロセス側の `stderr(Stdio::null())` も整合。
- **`ToolOutput` summary（コマンド + exit code）+ content（出力）**: 満たされている。`bash.rs:164-175` で exit 0 / 非 0 / シグナルを区別。content が空のときは `None` を返しており、`SUMMARY_THRESHOLD` を意識した良い実装。

### アーキテクチャ・スコープ

- **層分離**: `tools` クレート内に閉じており、`llm-worker` を低レベル基盤に保つ方針と整合（`bash.rs:20` で `Tool` trait のみ依存）。`builtin_tools()` のファクトリ列に追加するだけで、層を跨ぐ侵入はない。
- **クレート命名/構造**: `bash.rs` を独立モジュールに切り出し、`lib.rs` で `pub use bash::bash_tool` のみ公開。`read.rs/write.rs/...` と一貫。
- **依存追加**: `Cargo.toml` の tokio features に `process`/`time`/`io-util`/`sync` を追加（`Cargo.toml:22`）。`tempfile` は既存。`cargo add` 経由前提のフィールド追加で違和感なし。
- **Permission 層との関係**: ticket の前提通り、ScopedFs では保護せず Permission 層に委譲。`lib.rs:18-19` のドキュメントコメントで明示しており、設計意図は読み手に伝わる。
- **設計判断 1（wrapper による pwd 取得）**: `cd` パースの脆さ（サブシェル、変数展開、関数定義内 `cd` 等）を回避できるので妥当。`exec` で bash 自体が置換されると wrapper が走らないが、`bash.rs:149-155` が「ファイル読めなければ pwd 据え置き」とフェイルソフトしておりロバスト。
- **設計判断 2（wrapper の `wait`）**: `(sleep 0.05; echo bg) &` のようなジョブで stdout が EOF せずハングする問題に対する実装上必須の対処。`background_job_does_not_hang` で回帰防止済み。
- **設計判断 3（`tokio::sync::Mutex` で逐次化）**: pwd の共有可変状態と「順序のある shell セッション」の意味論を考えると正解。長時間コマンドの間 lock を握り続けるのは仕様上自然（同一セッションの bash は元々直列）。
- **設計判断 4（256KB cap）**: worker 側 `ToolOutputLimits` の手前で OOM を抑える二重防壁。truncated marker の追記後に `String::from_utf8_lossy` で UTF-8 化しており、マルチバイト切断もロスレスではないが panic はしない。妥当。
- **設計判断 5（summary/content）**: 既存ツールと API 形状が一致。`SUMMARY_THRESHOLD` の境界も意識されている。
- **設計判断 6（description のプロンプト誘導）**: Read/Write/Edit/Glob/Grep を優先させる文言は、Claude Code リファレンスとも整合し、ローカルモデルでも効きやすい簡潔さ。

### 指摘事項

#### Non-blocking / Follow-up

- **TUI 側の `render_default` 修正の同梱について** (`crates/tui/src/tool.rs:590-619`)
  - 内容としては正しいバグ修正。Bash のような汎用ツールが Detail モードでも summary しか出ない状態を解消している。
  - ただし、厳密には Bash チケットの範囲外（既存の任意の "default 経路の" ツールに同じ問題があったはず）。同梱の妥当性: Bash 投入によりバグが顕在化したこと、5 行程度の置き換えで完結すること、Bash 単体だと UX として未完であることを踏まえれば現実的な判断と言える。次回同種の状況では、TUI 表示仕様の修正として別チケットを切るほうがレビュー単位がきれいになる、というレベル。
  - フォローアップ提案: `crates/tui/` 配下に `output` を含むレンダリングが Detail/Normal で正しく出ることを確認するスナップショット/ユニットテストを 1 本追加すると、将来の `summary` フォールバック方向への意図しない退行を防げる（現状はロジックレビューのみで担保）。

- **`docs/ref/claude-code-deferred-tools.md` への追記**: Bash 実装と直接関係しない文献参照の追加（Anthropic vs OpenAI 比較への言及）。1 段落で軽微とはいえ、チケットスコープからは外れている。次回はドキュメント更新も別コミット/別チケット推奨。

- **pwd 更新の堅牢性についての観察 (`bash.rs:149-155`)**: ユーザーコマンドが `exec some-program` で bash を置換した場合や、wrapper の `pwd > tempfile` がディスクフル等で失敗した場合に pwd が据え置かれる挙動になっている。仕様上は妥当だが、ユーザー視点では「`cd foo && exec bar` 後に `cd` が消えた」ように見える可能性がある。コメントで現挙動の合理性は説明されているので blocking ではないが、将来 Permission 層導入時にエッジケースとして再考の余地あり。

#### Nits

- `BashParams` の `timeout` フィールドが `Option<u64>` で `#[serde(default)]` だが、`Option` は serde が自動的に欠落を `None` にするため `#[serde(default)]` は冗長（害はない）。
- `bash.rs:111-112` の `let mut child = child; let mut stdout = stdout;` は `async move` ブロックで mutable に再束縛しているだけ。慣用的だが `let mut` を引数側で書いてもよい。スタイル差。

### 判断

**Approve with follow-up** — チケット要件は完全に満たされており、設計判断もすべて合理的に説明されている。テストカバレッジ (8 unit + 1 integration) も妥当。同梱されている TUI 修正は実害のあるバグ修正で内容は正しいが、本来は別チケット相当のスコープ越えがあり、回帰テストの追加は次回までのフォローアップとして残しておくとよい。

---

## Round 2 (再レビュー: 2026-05-01)

### 主な変更点

1. 256KB byte-cap → **80 行 / 12 KiB を超えたら `<runtime_dir>/<pod_name>/bash-output/bash-XXX.log` に退避、tail 80 行 + パスを返す**
2. spill ファイルを Read で読めるようにするため、`Scope::deny_rules()` を `allow_rules()` の対称として追加。controller では memory 流儀（`ScopeConfig` を組み立てて `Scope::from_config` で rebuild）に統一
3. **pwd 永続を撤廃**（stateless 化）。各呼び出しは workspace root からスタート。tokio の `sync` feature 削除。`cd_persists_across_calls` → `cd_does_not_persist_across_calls` にテスト反転
4. TUI `render_default` の修正は前回と同じまま同梱（回帰テストはまだ）

### 前提・要件の再確認（仕様変更分）

- **要件「作業ディレクトリの永続」が撤回されたこと**: チケット本文 (`tickets/bash-tool.md:13`) には依然「作業ディレクトリの永続（ツール内部で `pwd` 状態を保持、`cd` で変更可能）」と書かれている。実装は stateless に切り替わっており、要件文と実装が乖離している。Claude Code 互換のために stateless が正しい判断という認識には同意するが、**チケット本文の更新がない点はチケットライフサイクルの観点で要修正**（`b. 詳細化や前提の変化` 段に該当）。
- **stdout/stderr 結合と timeout/exit handling**: 不変。`merges_stdout_and_stderr` / `nonzero_exit_is_reported` / `timeout_kills_long_command` で担保。
- **長い出力の扱い**: 新仕様（80 行 / 12 KiB 閾値、tail 返却 + spill パス通知）は description (`bash.rs:42-45`) でモデルに伝達済み。`long_output_spills_and_returns_tail` / `wide_short_output_still_spills_when_byte_budget_exceeded` で行ベース・バイトベース双方の閾値を確認。
- **spill ファイルの可視性**: `bash_spilled_file_is_readable_via_read_tool` (integration:353) で「spill 経由で書かれたファイルが Read ツールで読める」E2E が確認できている。Worker の 16 KiB cap で tail が落ちる問題への対処として筋の通った再設計。

### 設計判断の検証

#### (1) 二重防衛 cleanup（BashTool::Drop + RuntimeDir::Drop）

- **適切。** 役割分離が明快:
  - `BashTool::Drop` (`bash.rs:92-100`) — セッション終了時、その session が積んだ spill 群を削除。controller 上で worker/tools が落ちる経路に乗る。
  - `RuntimeDir::Drop` (`runtime/dir.rs:103-107`) — pod 終了時、`bash-output/` ディレクトリごと一掃。`SocketServer`、status/history 等と同列の sweeper として機能。
- 二段にする必要性も理屈が通る: BashTool だけだと「クラッシュや中断で Drop が走らなかった残骸」が残り続ける可能性があるが、RuntimeDir 全消しが最後の保険になる。逆に RuntimeDir だけだと「同一 pod 内で session を多数立ち上げ続けた場合に bash-output が肥大化する」リスクがあるが、BashTool::Drop が切り詰める。
- 設計コメント (`bash.rs:86-88`) で「session 終了時の遅延 cleanup」だと明示しているのも良い。

#### (2) `Scope::deny_rules()` 追加と memory 流儀への統一

- **対称性として自然。** `allow_rules()` (`scope.rs:148-157`) と `deny_rules()` (`scope.rs:165-174`) は完全な双子で、ResolvedRule から ScopeRule への射影がペアで揃った形。
- 当初考えていた `Scope::with_extra_read(self, PathBuf) -> Self` を撤回したのは正解。`with_extra_*` 系の単機能 API はパス追加のたびに増える危険があり、`ScopeConfig` 経由のラウンドトリップに比べて表現力で劣る。汎用 accessor を 1 本足すだけのほうが余計な API を呼ばない（YAGNI に沿う）。
- controller での組み立て方 (`controller.rs:245-255`) は memory の `build_scope_with_memory` (`pod.rs:2030-2037`) と同型で、コメント (`controller.rs:236-237`) で「memory が取っているのと同じアプローチ」と明示しているので意図が伝わりやすい。
- `Scope::summary()` (`scope.rs:198-230`) が deny を表示しないのと対称に、`deny_rules()` は明示的にプログラム的取得を許す形になっており、API 設計の意図が一貫している。

#### (3) stateless 化 + `BashTool.cwd: PathBuf` 不変フィールド

- **stateless 化自体は正しい判断。** Claude Code 仕様への寄せが理にかなっており、(a) tokio の `sync` feature が落ちて concurrent 実行が可能になる、(b) wrapper から pwd marker tempfile + `pwd > file` 行が消えてシンプル化、(c) サブシェル / 関数定義内 `cd` のような pwd 抽出のエッジケースが消える、と複数の便益がある。
- **`cwd: PathBuf` 不変フィールドの妥当性**: 現状の実装で十分。ご質問の「`fs` ごと持って `fs.pwd()` を毎回参照すべきか」については以下の理由で「**現状のスナップショットで OK**」と判断する:
  - `ScopedFs::pwd()` は pod-lifetime の不変値で、`ScopedFs::new(scope, pwd)` で構築後に変わらない。`fs` をフィールドとして持っても毎回同じ値が返る。
  - `BashTool` は子プロセスで fs を直接触らない（ScopedFs バイパス）ので、`fs` を参照する用事はない。pwd だけが必要。
  - 不要なフィールドを抱えるとライフタイムや clone コストが増える。`PathBuf` は単純で読みやすい。
  - 将来 `Bash` を ScopedFs 経由に切り替える計画があれば `fs` を持つ必要が出てくるが、ticket の前提（Permission 層に委譲）を覆さない限り発生しない。
- 一点だけ気になるとすれば、`bash.rs:78-81` のコメントで「`ScopedFs::pwd()` の snapshot」と説明している割に factory (`bash.rs:330-344`) では `fs.pwd().to_path_buf()` を呼ぶだけで、`fs` 自体は捨てているので、もし将来 Permission 層連携で `fs` が要るなら API 形状が変わる。今は気にしなくていい。

#### (4) TUI 修正の扱い

- **前回と完全に同一。回帰テスト追加もなし。** 前回 follow-up の 1 つ目「`crates/tui/` 配下に snapshot/unit test を 1 本」は未対応。
- 判断: **後段送り（別チケット）が妥当**。今回の追加変更が TUI には触れていない以上、再度 blocking 化するのは過剰。Bash チケットを閉じて、TUI の `render_default` 仕様確認を別チケットに切り出すほうが履歴が綺麗。
- ただし「別チケット化を約束する」のは現実的に flaky なので、`TODO.md` への 1 行追加（例: `- TUI tool render_default の output 表示に対する回帰テスト`）または専用チケット作成を Round 2 完了条件に組み込むことを推奨。

### 新規指摘事項

#### Blocking

なし。

#### Non-blocking / Follow-up

- **チケット本文と実装の乖離 (`tickets/bash-tool.md:13`)**: 「作業ディレクトリの永続」と書かれているが実装は stateless。Round 2 で意識的に撤回された変更なので、ticket 本文を更新してから完了に進むのが筋。代替の表記例:
  - `作業ディレクトリ: 各呼び出しは workspace root から開始（stateless、Claude Code 互換）。複数ディレクトリで作業するときは "cd <dir> && cmd" でチェイン`
- **`crates/tools/src/lib.rs:51` の broken intra-doc link**: `[\`manifest::Scope::with_extra_read\`]` を残したまま。`with_extra_read` は撤回されて存在しない。`cargo doc -p tools --no-deps` で warning が出る:
  ```
  warning: unresolved link to `manifest::Scope::with_extra_read`
   --> crates/tools/src/lib.rs:51:12
  ```
  実害は doc 生成警告のみだが、撤回した API のドキュメント参照が残るのは将来の混乱の元。`(see [\`manifest::Scope::deny_rules\`] / [\`Scope::from_config\`])` 等に書き直すか、リンクを外して文章だけ残すのが妥当。
- **TUI `render_default` 回帰テスト未追加**: Round 1 から持ち越し。前述の通り別チケット化推奨。
- **チケット範囲外の `docs/ref/claude-code-deferred-tools.md` 追記**: Round 1 と同じ。Round 2 では新たな逸脱はなし。
- **`exec` 系の堅牢性ノート (Round 1 持ち越し)**: stateless 化により pwd 更新の問題が消えたので、Round 1 で挙げた「`cd foo && exec bar` で pwd が消える」観察事項は **解消**。次回 Permission 層導入時のエッジケースとしてのみ意識すれば足りる。

#### Nits

- **`BashParams.timeout` の `#[serde(default)]`**: Round 1 と同じく冗長（害はない）。
- **`bash.rs:151-163` の child エラー早期 return パス**: `output_path` の cleanup を 3 箇所で重複 (`spawn` 失敗、`wait` 失敗、timeout でファイルサイズ 0)。1 つの helper に括れるが、エラーハンドラの分岐が異なる（`?` で抜けるかそうでないか）ので無理に統一する必要はない。Mio スタイルが統一されていれば良い、というレベル。
- **`shell_single_quote`** (`bash.rs:319-322`): 現実的にはここに来る出力パスは tempfile が生成した英数字パスで、`'` を含むことは無いに等しい。とはいえ防御として正しい実装で、コストもゼロ。

### 判断

**Approve with follow-up** — Round 2 の主要変更（spill 退避、`deny_rules()`、stateless 化、cleanup 二重化）はいずれも設計として筋が良く、テストカバレッジ（unit 10 / integration 14 / edge_cases 9 全 pass、`cargo check --workspace` クリーン、TUI 55/55 pass）で動作も担保されている。残作業は (a) チケット本文の文言を stateless に合わせて更新、(b) `lib.rs:51` の broken doc link を修正、(c) TUI の回帰テストは別チケットへ切り出し — の 3 点。いずれも軽微で、(a)(b) を Round 2 完了の条件として処理すれば本チケット自体はクローズ可能。
