# Review: セッション内 Task ツール

対象: `tickets/session-todo.md` (`28fe1da` で再分割) / 実装 `6f2aca8`。

## 前提・要件の確認

- TaskCreate / TaskList / TaskGet / TaskUpdate を builtin tool として登録: 充足。`crates/tools/src/task.rs:507` の `task_tools(store)` を `crates/tools/src/lib.rs:55` の `builtin_tools` が拡張し、Controller 経由で Worker に登録される (`crates/pod/src/controller.rs:268`)。
- TaskCreate の自動採番 + 初期 status pending: 充足 (`crates/tools/src/task.rs:71`)。
- TaskUpdate の最低1フィールド + 既存チェック + 物理削除なし (`status=deleted`): 充足 (`crates/tools/src/task.rs:108`、テスト `task_update_validates_existing_and_at_least_one_field`)。
- TaskList が completed / deleted を含めて全件返す: 充足 (`crates/tools/src/task.rs:84`)。
- TaskGet が `taskid` 不在でエラー: 充足 (`crates/tools/src/task.rs:304`)。
- Resume 時の履歴 replay で TaskCreate / TaskUpdate を順に再適用: 実装あり (`replay_history`、`Pod::from_state` で `TaskStore::from_history(&state.history)`)。**ただし compact 後の resume で重複が発生する** (後述「Blocking」)。
- `tool_call.arguments` の schema 乖離は無視: 充足 (`from_str` 失敗時 `_` でスキップ)。
- compact 起動時、TaskStore 全スナップショットを compact worker context に渡す: 充足 (`build_summary_input` の `task_snapshot` 引数、`crates/pod/src/pod.rs:1278`)。
- compact worker に Task 編集権限を与えない: 充足 (compact 用 `summary_worker.register_tool` は read / mark_read_required / add_reference / write_summary のみ。Task* tool は注入されていない)。
- compact 後の新セッションで TaskStore handle を維持し全件保持: 充足 (`self.task_store` は compact 中に置換されない。Worker の history のみ入れ替え)。
- compact 後の新セッション冒頭に snapshot system message + TaskList 発火: 充足 (`crates/pod/src/pod.rs:1409-1419`)。
- 専用 LogEntry / Persistence 型を追加しない: 充足 (`replay_history` と inline snapshot text に閉じている)。
- 単体テスト: 4 ツール基本 / バリデーション / replay / compact snapshot replay の4本。**retained tail に Task* call が残るシナリオの replay テストが欠落** (後述)。

## アーキテクチャ・スコープ

- `tools` クレートの session-lifetime 状態として TaskStore を置き、Pod が handle を所有して `attach_task_store` で受け渡す方針は `Tracker` の前例と整合。レイヤー越境なし。
- 4 ツールを `crates/tools/src/task.rs` の単一モジュールにまとめている点は、現時点の規模 (~600 行) と将来の機能追加余地を考えると許容範囲。`pod.rs` への impl 流入もない。
- ツール本体は `tools` 層、配線と compact 連携は `pod` 層、と責務分離が保たれている。
- TaskStore は `Arc<Mutex<Inner>>` の thin wrapper で、永続化を tool_call replay に寄せる方針 (専用レーン無し) もチケットの要件どおり。
- 命名: `TaskStore` / `TaskEntry` / `TaskStatus` / `TaskSnapshot` の対称性も良。`insomnia-` prefix は無し (規約準拠)。
- `task_store: tools::TaskStore` を pub field 露出ではなく `attach_task_store` / `task_store()` の getter で扱っており、これも `Tracker` の前例と整合。
- compact 時の "synthetic tool_call" (`compact-tasklist` の TaskList call/result) は新規パターン。Pod 配下で Worker 履歴に注入される唯一のラインであり、再現性とプロバイダ受理性が運用上の検証対象になる (フォローアップ参照)。

## 指摘事項

### Blocking

- **compact 跨ぎの resume で TaskStore が重複/二重適用される。** `crates/pod/src/pod.rs:1398-1423` で構築する post-compact history は `[summary, ...auto-read, snapshot system message, TaskList synthetic call/result, references, ...retained_items]` の順だが、`retained_items` は compact 直前まで生きていた末尾スライスをそのまま含むため、retained 内に `TaskCreate` / `TaskUpdate` が含まれた場合、resume 時の `TaskStore::from_history` が:
  1. 先頭付近の `[Session TaskStore snapshot]` を読み `replace_with` で全件復元
  2. 後続の retained `TaskCreate` を再適用 → 同内容の重複 entry が新規 `taskid` で生える
  3. retained `TaskUpdate` も再適用 (こちらは冪等で害は薄いが意味は揺れる)

  になる。compact は token threshold 到達で起きるので、直近に Task* を呼んだケース (= 実際の主用途) で確実に発火する。

  既存テスト `replay_history_uses_compact_snapshot_and_continues_updates` (`crates/tools/src/task.rs:592`) は snapshot を history[0] に置き、後続を「compact 後の新規操作」として書いているため、本ケース (snapshot より後の retained_items に旧 Task* call が残る) を踏んでいない。

  対策案:
  - (A) `pod.rs:1423` の `new_history.extend(retained_items)` の前に retained_items から `Item::ToolCall { name == "TaskCreate" | "TaskUpdate" .. }` と対応する `ToolResult` を除去する。snapshot + TaskList 再発火がポストコンパクトの唯一の真実になる
  - (B) `replay_history` 側に anchor を入れ、snapshot を見たら**それ以前の** Task* call を捨てる + snapshot 以降は通常 replay。retained_items が snapshot より後ろにある現レイアウトでは効かないので、history 上の snapshot の位置を末尾 (retained_items の後) に移すなどレイアウト変更が必要
  - (C) compact 後 history 構築時に、snapshot の代わりに「TaskStore を空にしてから個別 TaskCreate で再現する」方針に切り替える (= snapshot の役割を tool_call 列で担う)。ただし冗長なので非推奨

  推奨は (A)。あわせて回帰テストとして「retained_items に旧 TaskCreate を含む post-compact history を `from_history` に通し、duplicate が出ないこと」のケースを追加してほしい。

### Non-blocking / Follow-up

- **snapshot text のラウンドトリップが lossy。** `render_snapshot` は `subject` / `description` を1行 inline で書き出すので、本文に改行が含まれると `parse_rendered_snapshot` が後続行を読み落とし、subject/description の途中で切れる (`crates/tools/src/task.rs:377-433`)。`description` は LLM が複数行で書く可能性が現実的なので、snapshot レンダリングを JSON ブロック (例: コードフェンスで `{"tasks":[...]}`) に切り替えるか、`description` に YAML literal block (`|`) を使うなどでロスレス化したい。replay の信頼性に直結する。
- **compact 由来の synthetic ToolCall の provider 受理性。** `Item::tool_call("compact-tasklist", "TaskList", "{}")` + 対応する `ToolResult` が、対応する assistant turn なしに history 中に挿入される (`crates/pod/src/pod.rs:1414-1419`)。Anthropic / OpenAI / Codex 各 client がこの形を受理して次リクエストを通すか、E2E で1度通しておきたい。フォーマット・ID プレフィックス的に拒否されるリスクは低いが、未検証ライン。
- **`PodInterceptor::pre_llm_request` の `&& !state.just_compacted()` 追加 (`crates/pod/src/ipc/interceptor.rs:130`) はチケット範囲外。** 内容自体は `try_post_run_compact` (`pod.rs:1079`) と同じガードを request-side にも回す mirror で、compact 直後の閾値再ヒットによる thrash 防止として妥当。とはいえ「session-todo」のスコープではないので、本来は `compact-thrash-fix` のような別チケットに切り出すか、ticket 本文に「副次修正」として明記するのが望ましい。今回はそのまま採用するなら commit message と `## 範囲外` の補記で痕跡を残してもらえると後追いが楽。
- snapshot system message に「This is the complete session task list ...」と英文で説明が入っているが、TaskList 用の synthetic tool_call の summary には説明なし。LLM 体験としては snapshot system message → TaskList result の二段で同じ情報が出るので、片方に寄せて簡潔化しても良い (任意)。

### Nits

- `crates/pod/src/pod.rs:1398` のコメント: `[summary, ...auto-read, task snapshot, TaskList result, references, ...retained]` だが実際は `task snapshot` の後ろに `TaskList tool_call` と `tool_result` の **2 件** が入る。3 件であることをコメントに反映。
- `task.rs:79` `inner.next_taskid.saturating_add(1)`: u64 で saturating は事実上不要だが、害もない。
- `lib.rs` の `pub use task::{TaskEntry, TaskSnapshot, TaskStatus, TaskStore, task_tools};` で `task::snapshot_overview` だけ `pod.rs` から `tools::task::snapshot_overview(...)` のフルパスで参照されている。`tools::snapshot_overview` 等で同レベル re-export しておくと API surface が揃う (任意)。

## 判断

**Request changes** — Resume 後の TaskStore 重複は要件「セッションを resume すると、履歴中の TaskCreate / TaskUpdate replay により TaskStore が復元される」「compact を跨いでも、TaskStore 全件が維持され」の両方に直接抵触する正解性 issue。Blocking 1点 (retained_items 二重適用) を直したうえで回帰テストを追加して再提出してほしい。アーキテクチャ・スコープ・残りの要件カバーは概ね健全で、`簡素な` 設計を保てている。

## 再レビュー (commit `420f74e` まで反映)

判断: **Approve**。

### 対応状況

- **Blocking (retained 二重適用)**: 解消。`ceafff9` で post-compact history のレイアウトを `[summary, auto-read, references, ...retained, snapshot, TaskList synthetic]` に変更。snapshot を末尾に置き `replace_with` が常に最後勝ちにする設計に切り替えた。回帰テスト `trailing_snapshot_supersedes_pre_compact_taskcreates_in_retained` で「retained に pre-compact TaskCreate × 2 + TaskUpdate × 2 → snapshot → post-compact TaskCreate × 1」のシナリオが正しく 3 件で収束することを確認。
- **Non-blocking #1 (snapshot text の lossy round-trip)**: 解消。`420f74e` で `render_snapshot` を「overview 一行 + ` ```json` フェンス内 JSON」形式に変更。`parse_compact_snapshot_text` も JSON フェンスから serde で復元する形に再実装。`snapshot_round_trips_multiline_subject_and_description` テストで改行入り subject/description + 紛らわしい `status:` プレフィックス + バッククォートが lossless で復元することを確認。`HashMap`-based 行パーサと `parse_rendered_snapshot` / `task_from_fields` ヘルパは削除。
- **Non-blocking #2 (synthetic compact-tasklist の provider 受理性)**: 構造テストで部分対応。`synthetic_compact_tasklist_pair_is_well_formed` テストで「snapshot system message → TaskList tool_call → ToolResult」3 連の構造契約 (matched call_id、tool name、content recoverability) を確認。実プロセス E2E はプロジェクト全体で未設計 (CLAUDE.md 「E2E は未設計」) のためここでは構造的健全性までしか確認できない。実 provider 通過は将来の E2E 設計時に再確認する follow-up。
- **Non-blocking #3 (`interceptor.rs:130` 範囲外修正)**: 範囲外注記として残す方針で確定。`!state.just_compacted()` ガードは `try_post_run_compact` の既存ガードを request-side に mirror する妥当な小修正で、本来は別チケットだが現規模では note 扱いとする (reviewer 提示の 2択のうち「範囲外注記」を採用)。今後 compact thrash 周りで関連修正が必要になった場合に別チケットへ切り出すフォローアップ余地は残る。

### Nit (`pod.rs:1398` コメントの 3 件挿入の不一致)

レイアウト変更に伴い同じく更新済み (`ceafff9`)。新コメント: `[summary, ...auto-read, references, ...retained, task snapshot, TaskList synthetic call/result]` + 末尾配置の理由を明記。

### 残る follow-up

- 改行入り subject/description のロスレス化は完了したが、TaskList が tool result として LLM に返す JSON フェンスのトークン効率は将来の課題 (大量 task 時)
- synthetic `compact-tasklist` の provider 受理性 E2E (上述、プロジェクト E2E 設計の中で扱う)
- `interceptor.rs` の compact thrash guard は compact 周辺の他 issue が表面化した際に専用チケット化を検討
